// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only tools the on-device model may call during an investigation.
//!
//! Rust owns every collector. The model names folders and processes only by
//! opaque handles issued in earlier results (`n2`, `p1`), sees terse capped
//! text, and can never reach a path, command, or action it was not given.
//! Action handles (`A1`) name only actions Rust already considers eligible
//! for a suggestion; the user still adds and confirms every action.

use crate::{
    ai,
    cache::{CacheEntry, CacheStatus, format_kb},
    care::{self, Finding, Metrics, OwnerGuess, ProcessSample, Session, Target},
    investigation::{CheckObservation, EvidenceKind, EvidenceStatus},
    processes::{ProcessEntry, ProcessHealth},
    storage::{AgeProfile, StorageInventory, StorageItem, StorageItemKind, VolumeStats},
};
use serde_json::{Value, json};
use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Longest tool output returned to the model, in bytes.
pub const OUTPUT_CAP: usize = 560;
/// Handles of each kind issued per investigation (`n1`…`n99`).
pub const MAX_HANDLES: usize = 99;
const GIB_KB: u64 = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arg {
    None,
    Folder,
    Process,
    Choice(&'static str, &'static [&'static str]),
}

#[derive(Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub arg: Arg,
}

pub const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "list_children",
        description: "Largest items inside a folder, with handles for subfolders.",
        arg: Arg::Folder,
    },
    ToolSpec {
        name: "folder_age",
        description: "How much of a folder's space changed recently versus long ago.",
        arg: Arg::Folder,
    },
    ToolSpec {
        name: "open_handles",
        description: "Which running processes hold files open inside a folder.",
        arg: Arg::Folder,
    },
    ToolSpec {
        name: "growth_history",
        description: "Earlier complete measurements of a folder from saved history.",
        arg: Arg::Folder,
    },
    ToolSpec {
        name: "growth",
        description: "What grew or shrank since an earlier complete assessment; reports missing history explicitly.",
        arg: Arg::Choice("since", &["previous", "day", "week", "month"]),
    },
    ToolSpec {
        name: "cleanup_rule",
        description: "Whether an app cleanup rule covers a folder, its status, and any eligible action ID.",
        arg: Arg::Folder,
    },
    ToolSpec {
        name: "identify_owner",
        description: "Which app or tool most likely owns a folder, and when that app was last opened.",
        arg: Arg::Folder,
    },
    ToolSpec {
        name: "process_details",
        description: "Identity, state, memory, CPU, and parent of one process.",
        arg: Arg::Process,
    },
    ToolSpec {
        name: "sample_process",
        description: "Three one-second samples of one process's CPU, memory, and presence.",
        arg: Arg::Process,
    },
    ToolSpec {
        name: "memory_state",
        description: "Memory pressure, swap activity, and the largest memory users.",
        arg: Arg::None,
    },
    ToolSpec {
        name: "top_processes",
        description: "Processes using the most CPU or memory right now.",
        arg: Arg::Choice("sort", &["cpu", "memory"]),
    },
    ToolSpec {
        name: "disk_accounting",
        description: "Volume capacity, free space, snapshots, and space the folder scan could not see.",
        arg: Arg::None,
    },
    ToolSpec {
        name: "volume_context",
        description: "External and non-APFS volumes mounted right now.",
        arg: Arg::None,
    },
    ToolSpec {
        name: "request_trace",
        description: "Ask the user to approve an 8-second read-only fseventsd filesystem trace.",
        arg: Arg::None,
    },
    ToolSpec {
        name: "reference_notes",
        description: "Short Apple reference notes on one macOS topic.",
        arg: Arg::Choice(
            "topic",
            &["fseventsd", "fs_usage", "apfs_space", "memory_pressure"],
        ),
    },
    ToolSpec {
        name: "cleanup_options",
        description: "Total eligible cleanup, largest cache actions, and large folders to inspect before deciding what to remove.",
        arg: Arg::None,
    },
    ToolSpec {
        name: "list_findings",
        description: "The app's current measured findings with handles and eligible action IDs.",
        arg: Arg::None,
    },
];

pub fn spec(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// The helper-facing description of one tool.
pub fn spec_json(spec: &ToolSpec) -> Value {
    let mut value = json!({"name": spec.name, "description": spec.description});
    let argument = match spec.arg {
        Arg::None => None,
        Arg::Folder => Some(json!({
            "name": "folder",
            "description": "A folder handle such as n1 from an earlier result.",
            "kind": "handle",
            "pattern": "n[0-9]{1,2}",
        })),
        Arg::Process => Some(json!({
            "name": "process",
            "description": "A process handle such as p1 from an earlier result.",
            "kind": "handle",
            "pattern": "p[0-9]{1,2}",
        })),
        Arg::Choice(name, choices) => Some(json!({
            "name": name,
            "description": format!("One of: {}.", choices.join(", ")),
            "kind": "choice",
            "choices": choices,
        })),
    };
    if let Some(argument) = argument {
        value["argument"] = argument;
    }
    value
}

/// Each investigation family sees a small toolset so tool definitions fit
/// the on-device model's 4,096-token context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toolset {
    Storage,
    Capacity,
    Process,
    Fseventsd,
    Ask,
}

impl Toolset {
    /// Keep the six-tool context budget while exposing the in-use check for
    /// cleanup questions. General resource questions retain top_processes.
    pub fn tools_for_question(
        self,
        automatic: bool,
        question: Option<&str>,
    ) -> Vec<&'static ToolSpec> {
        let mut tools = self.tools(automatic);
        if self == Self::Ask && question.is_some_and(is_cleanup_question) {
            for tool in &mut tools {
                let replacement = match tool.name {
                    "top_processes" => "open_handles",
                    "memory_state" => "cleanup_options",
                    _ => continue,
                };
                *tool = spec(replacement).expect("known read-only tool");
            }
        }
        if self == Self::Ask
            && question.and_then(growth_period).is_some()
            && let Some(tool) = tools.iter_mut().find(|tool| {
                tool.name
                    == if question.is_some_and(is_cleanup_question) {
                        "disk_accounting"
                    } else {
                        "memory_state"
                    }
            })
        {
            *tool = spec("growth").expect("known read-only tool");
        }
        tools
    }

    pub fn fallback_for_question(
        self,
        automatic: bool,
        question: Option<&str>,
    ) -> Vec<(&'static str, Option<&'static str>)> {
        if self == Self::Ask
            && let Some(period) = question.and_then(growth_period)
        {
            return vec![("growth", Some(period))];
        }
        if self == Self::Ask && question.is_some_and(is_cleanup_question) {
            return vec![("cleanup_options", None), ("disk_accounting", None)];
        }
        self.fallback(automatic)
    }

    pub fn tools(self, automatic: bool) -> Vec<&'static ToolSpec> {
        let names: &[&str] = match self {
            Self::Storage => &[
                "list_children",
                "folder_age",
                "open_handles",
                "growth_history",
                "cleanup_rule",
                "identify_owner",
            ],
            Self::Capacity => &[
                "disk_accounting",
                "list_children",
                "volume_context",
                "cleanup_rule",
                "folder_age",
            ],
            Self::Process => &[
                "process_details",
                "sample_process",
                "memory_state",
                "top_processes",
            ],
            Self::Fseventsd => &[
                "memory_state",
                "top_processes",
                "volume_context",
                "reference_notes",
                "request_trace",
            ],
            Self::Ask => &[
                "list_findings",
                "list_children",
                "disk_accounting",
                "top_processes",
                "memory_state",
                "cleanup_rule",
            ],
        };
        names
            .iter()
            // Administrator-assisted tracing needs a person at the keyboard.
            .filter(|name| !(automatic && **name == "request_trace"))
            .filter_map(|name| spec(name))
            .collect()
    }

    /// The measured sequence used when no model is available. The subject is
    /// always the first issued handle (`n1` or `p1`).
    pub fn fallback(self, automatic: bool) -> Vec<(&'static str, Option<&'static str>)> {
        let mut steps: Vec<(&str, Option<&str>)> = match self {
            Self::Storage => vec![
                ("list_children", Some("n1")),
                ("cleanup_rule", Some("n1")),
                ("open_handles", Some("n1")),
                ("folder_age", Some("n1")),
                ("growth_history", Some("n1")),
            ],
            Self::Capacity => vec![
                ("disk_accounting", None),
                ("list_children", Some("n1")),
                ("volume_context", None),
            ],
            Self::Process => vec![
                ("process_details", Some("p1")),
                ("sample_process", Some("p1")),
                ("memory_state", None),
            ],
            Self::Fseventsd => vec![
                ("request_trace", None),
                ("memory_state", None),
                ("volume_context", None),
                ("reference_notes", Some("fseventsd")),
            ],
            Self::Ask => vec![
                ("list_findings", None),
                ("disk_accounting", None),
                ("memory_state", None),
            ],
        };
        if automatic {
            steps.retain(|(name, _)| *name != "request_trace");
        }
        steps
    }
}

pub fn is_cleanup_question(question: &str) -> bool {
    let words: Vec<_> = question
        .split(|c: char| !c.is_alphabetic())
        .map(str::to_ascii_lowercase)
        .collect();
    let has = |choices: &[&str]| words.iter().any(|word| choices.contains(&word.as_str()));
    has(&[
        "cache", "caches", "clear", "clean", "cleanup", "delete", "deleting", "remove",
    ]) || (has(&["free", "freeing", "release", "reclaim", "recover"])
        && has(&[
            "space", "disk", "storage", "gb", "gib", "mb", "mib", "tb", "tib",
        ])
        && !has(&["memory", "ram"]))
}

/// An explicit storage amount in a cleanup request. GB is decimal; GiB is binary.
pub fn cleanup_goal_kb(question: &str) -> Option<u64> {
    if !is_cleanup_question(question) {
        return None;
    }
    let text = question.to_ascii_lowercase();
    let chars: Vec<_> = text.chars().collect();
    let mut i = 0;
    let mut clause_start = 0;
    let mut goal = None;
    while i < chars.len() {
        if !chars[i].is_ascii_digit()
            && !(chars[i] == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            if matches!(chars[i], '.' | '?' | '!' | ';' | '\n') {
                clause_start = i + 1;
            }
            i += 1;
            continue;
        }
        let start = i;
        // Never reinterpret the suffix of a signed amount, range, grouped
        // number, exponent, or identifier as an independent storage goal.
        let boundary_valid = start == 0
            || !matches!(chars[start - 1], '.' | ',' | '-' | '+' | '–' | '—')
                && !chars[start - 1].is_alphanumeric();
        let prefix = chars[clause_start..start].iter().collect::<String>();
        let words: Vec<_> = prefix
            .split(|c: char| !c.is_alphabetic())
            .filter(|word| !word.is_empty())
            .collect();
        let requester = words.iter().rposition(|word| {
            matches!(
                *word,
                "free"
                    | "freeing"
                    | "release"
                    | "reclaim"
                    | "recover"
                    | "need"
                    | "want"
                    | "clear"
                    | "clean"
                    | "cleanup"
                    | "remove"
                    | "delete"
            )
        });
        let requested = requester.is_some_and(|requester| {
            !words[requester + 1..].iter().any(|word| {
                matches!(
                    *word,
                    "have" | "has" | "had" | "with" | "my" | "drive" | "capacity"
                )
            })
        });
        while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
            i += 1;
        }
        let amount = chars[start..i]
            .iter()
            .collect::<String>()
            .parse::<f64>()
            .ok();
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_alphabetic() {
            i += 1;
        }
        let unit = chars[start..i].iter().collect::<String>();
        let bytes = match unit.as_str() {
            "mb" => 1_000_000.,
            "gb" => 1_000_000_000.,
            "tb" => 1_000_000_000_000.,
            "mib" => 1_048_576.,
            "gib" => 1_073_741_824.,
            "tib" => 1_099_511_627_776.,
            _ => continue,
        };
        let suffix = chars[i..].iter().collect::<String>();
        let reported = suffix
            .split(|c: char| !c.is_alphabetic())
            .filter(|word| !word.is_empty())
            .take(2)
            .any(|word| matches!(word, "free" | "available" | "left" | "drive" | "capacity"));
        clause_start = i;
        if goal.is_some() && words.iter().any(|word| matches!(*word, "or" | "to")) && !requested {
            return None;
        }
        if let Some(kb) = amount.map(|n| n * bytes / 1024.)
            && boundary_valid
            && requested
            && !reported
            && kb.is_finite()
            && kb > 0.
            && kb < u64::MAX as f64
        {
            goal = Some(kb.ceil() as u64);
        }
    }
    goal
}

/// Select the bounded history check for common temporal storage questions.
fn growth_period(question: &str) -> Option<&'static str> {
    let words: Vec<String> = question
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .collect();
    let has = |choices: &[&str]| words.iter().any(|word| choices.contains(&word.as_str()));
    if !has(&[
        "grew",
        "grown",
        "grow",
        "growth",
        "shrunk",
        "shrank",
        "changed",
        "changes",
        "since",
        "increase",
        "increased",
        "bigger",
        "history",
    ]) {
        return None;
    }
    Some(if has(&["week", "weekly"]) {
        "week"
    } else if has(&["month", "monthly"]) {
        "month"
    } else if has(&["yesterday", "day", "daily"]) {
        "day"
    } else {
        "previous"
    })
}

/// Opaque references issued to the model for one investigation.
#[derive(Debug, Default, Clone)]
pub struct Handles {
    folders: Vec<PathBuf>,
    processes: Vec<(u32, String)>,
    actions: Vec<String>,
}

fn issue<T: PartialEq>(list: &mut Vec<T>, value: T, prefix: char) -> Option<String> {
    if let Some(index) = list.iter().position(|known| known == &value) {
        return Some(format!("{prefix}{}", index + 1));
    }
    if list.len() >= MAX_HANDLES {
        return None;
    }
    list.push(value);
    Some(format!("{prefix}{}", list.len()))
}

fn handle_index(handle: &str, prefix: char, kind: &str) -> Result<usize, String> {
    let handle = handle.trim();
    if handle.contains('/') || handle.contains('~') {
        return Err(format!(
            "Paths are not accepted. Use a {kind} handle such as {prefix}1 from an earlier result."
        ));
    }
    handle
        .strip_prefix(prefix)
        .filter(|digits| {
            (1..=2).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit())
        })
        .and_then(|digits| digits.parse::<usize>().ok())
        .filter(|number| *number >= 1)
        .map(|number| number - 1)
        .ok_or_else(|| {
            format!(
                "Unknown handle {}. Use a {kind} handle shown in an earlier result.",
                ai::display_text(handle)
                    .chars()
                    .take(16)
                    .collect::<String>()
            )
        })
}

impl Handles {
    pub fn folder(&mut self, path: &Path) -> Option<String> {
        issue(&mut self.folders, path.to_path_buf(), 'n')
    }
    pub fn process(&mut self, pid: u32, start_time: &str) -> Option<String> {
        issue(&mut self.processes, (pid, start_time.to_string()), 'p')
    }
    pub fn action(&mut self, action_id: &str) -> Option<String> {
        issue(&mut self.actions, action_id.to_string(), 'A')
    }
    pub fn folder_path(&self, handle: &str) -> Result<&Path, String> {
        let index = handle_index(handle, 'n', "folder")?;
        self.folders
            .get(index)
            .map(PathBuf::as_path)
            .ok_or_else(|| {
                format!("Unknown handle {handle}. Use a folder handle shown in an earlier result.")
            })
    }
    pub fn process_identity(&self, handle: &str) -> Result<(u32, &str), String> {
        let index = handle_index(handle, 'p', "process")?;
        self.processes
            .get(index)
            .map(|(pid, start)| (*pid, start.as_str()))
            .ok_or_else(|| {
                format!("Unknown handle {handle}. Use a process handle shown in an earlier result.")
            })
    }
    pub fn action_id(&self, handle: &str) -> Option<&str> {
        let index = handle_index(handle, 'A', "action").ok()?;
        self.actions.get(index).map(String::as_str)
    }
}

/// A call whose tool and argument shape were checked against the toolset.
#[derive(Debug, Clone)]
pub struct Call {
    pub tool: &'static ToolSpec,
    pub argument: Option<String>,
}

pub fn parse_call(
    name: &str,
    arguments: &str,
    allowed: &[&'static ToolSpec],
) -> Result<Call, String> {
    let tool = allowed
        .iter()
        .copied()
        .find(|tool| tool.name == name)
        .ok_or_else(|| {
            format!(
                "Tool {} is not available in this investigation.",
                ai::display_text(name).chars().take(40).collect::<String>()
            )
        })?;
    let value: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let field = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
            .ok_or_else(|| format!("{} needs a {key} argument.", tool.name))
    };
    let argument = match tool.arg {
        Arg::None => None,
        Arg::Folder => Some(field("folder")?),
        Arg::Process => Some(field("process")?),
        Arg::Choice(key, choices) => {
            let chosen = field(key)?;
            if !choices.contains(&chosen.as_str()) {
                return Err(format!("{key} must be one of: {}.", choices.join(", ")));
            }
            Some(chosen)
        }
    };
    Ok(Call { tool, argument })
}

/// Borrowed app state for one tool call. Nothing here can change files.
pub struct ToolWorld<'a> {
    pub home: &'a Path,
    pub inventory: Option<&'a StorageInventory>,
    pub entries: &'a [CacheEntry],
    pub processes: &'a [ProcessEntry],
    pub metrics: &'a Metrics,
    pub findings: &'a [Finding],
    pub history: &'a [Session],
    /// The current assessment, including its root, volume identity and coverage.
    pub current: Option<&'a Session>,
    pub volume: Option<&'a VolumeStats>,
    pub online_research: bool,
    pub subject_pid: Option<u32>,
    /// The action ID Rust would allow the model to suggest for a target.
    pub suggest: &'a dyn Fn(&Target) -> Option<String>,
}

/// One tool result: model-facing text plus typed evidence metadata.
#[derive(Debug)]
pub struct Outcome {
    pub text: String,
    pub status: EvidenceStatus,
    pub kind: EvidenceKind,
    pub supports: Vec<String>,
    pub contradicts: Vec<String>,
    /// Newly measured folders to merge into the explorer inventory.
    pub inventory: Option<Box<StorageInventory>>,
    /// Non-overlapping eligible cache sizes, never the sizes of review folders.
    pub cleanup_total_kb: Option<u64>,
}

impl Outcome {
    fn complete(kind: EvidenceKind, text: String) -> Self {
        Self {
            text,
            status: EvidenceStatus::Complete,
            kind,
            supports: vec![],
            contradicts: vec![],
            inventory: None,
            cleanup_total_kb: None,
        }
    }
    fn unavailable(kind: EvidenceKind, status: EvidenceStatus, text: String) -> Self {
        Self {
            status,
            ..Self::complete(kind, text)
        }
    }
    fn supporting(mut self, ids: &[&str]) -> Self {
        self.supports.extend(ids.iter().map(|id| id.to_string()));
        self
    }
    fn contradicting(mut self, ids: &[&str]) -> Self {
        self.contradicts.extend(ids.iter().map(|id| id.to_string()));
        self
    }
    fn partial_if(mut self, partial: bool) -> Self {
        if partial && self.status == EvidenceStatus::Complete {
            self.status = EvidenceStatus::Partial;
        }
        self
    }
}

pub type Job = Box<dyn FnOnce(&AtomicBool) -> Slow + Send>;

pub enum Dispatch {
    Ready(Outcome),
    Slow(Job),
    NeedsApproval(Job),
    /// The call referenced something the model was never given.
    Rejected(String),
}

/// Raw results from collectors that run off the interface thread.
pub enum Slow {
    Children(PathBuf, Box<StorageInventory>),
    Age(PathBuf, io::Result<AgeProfile>),
    Owners(PathBuf, io::Result<Vec<(u32, String)>>),
    Owner(PathBuf, OwnerGuess),
    Samples(String, String, Result<Vec<ProcessSample>, String>),
    Observation(CheckObservation),
    Notes(Result<String, String>),
}

fn truncate_middle(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let head = (max - 1) / 2;
    let tail = max - 1 - head;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!("{start}…{end}")
}

pub fn short_path(path: &Path, home: &Path) -> String {
    let text = match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    };
    truncate_middle(&ai::display_text(&text), 60)
}

/// Cap model-facing text at a character boundary.
pub fn cap(text: &str) -> String {
    let text = ai::display_text(text);
    if text.len() <= OUTPUT_CAP {
        return text;
    }
    let mut end = OUTPUT_CAP - '…'.len_utf8();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn percent(part: u64, whole: u64) -> u64 {
    if whole == 0 {
        0
    } else {
        (part as f64 * 100. / whole as f64).round() as u64
    }
}

fn duration_text(seconds: u64) -> String {
    match seconds {
        0..=119 => format!("{seconds} seconds"),
        120..=7_199 => format!("{} minutes", seconds / 60),
        7_200..=172_799 => format!("{} hours", seconds / 3_600),
        _ => format!("{} days", seconds / 86_400),
    }
}

fn process_name(command: &str) -> String {
    ai::display_text(
        Path::new(command)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(command),
    )
}

/// A short timeline label, for example `folder_age(~/Library/Caches)`.
pub fn label(call: &Call, handles: &Handles, world: &ToolWorld<'_>) -> String {
    let argument = call
        .argument
        .as_deref()
        .map(|argument| match call.tool.arg {
            Arg::Folder => handles
                .folder_path(argument)
                .map(|path| short_path(path, world.home))
                .unwrap_or_else(|_| ai::display_text(argument)),
            Arg::Process => handles
                .process_identity(argument)
                .map(|(pid, start)| {
                    world
                        .processes
                        .iter()
                        .find(|process| process.pid == pid && process.start_time == start)
                        .map(|process| format!("{} · PID {pid}", process_name(&process.command)))
                        .unwrap_or_else(|| format!("PID {pid}"))
                })
                .unwrap_or_else(|_| ai::display_text(argument)),
            _ => ai::display_text(argument),
        });
    format!("{}({})", call.tool.name, argument.unwrap_or_default())
}

pub fn dispatch(call: &Call, world: &ToolWorld<'_>, handles: &mut Handles) -> Dispatch {
    let argument = call.argument.as_deref().unwrap_or_default();
    match call.tool.arg {
        Arg::Folder => {
            let path = match handles.folder_path(argument) {
                Ok(path) => path.to_path_buf(),
                Err(error) => return Dispatch::Rejected(error),
            };
            folder_tool(call.tool.name, path, world, handles)
        }
        Arg::Process => {
            let (pid, start) = match handles.process_identity(argument) {
                Ok((pid, start)) => (pid, start.to_string()),
                Err(error) => return Dispatch::Rejected(error),
            };
            process_tool(call.tool.name, argument, pid, start, world)
        }
        Arg::None | Arg::Choice(..) => system_tool(call.tool.name, argument, world, handles),
    }
}

fn folder_tool(
    name: &str,
    path: PathBuf,
    world: &ToolWorld<'_>,
    handles: &mut Handles,
) -> Dispatch {
    let home = world.home.to_path_buf();
    match name {
        "list_children" => {
            if let Some(items) = world
                .inventory
                .and_then(|inventory| inventory.children.get(&path))
            {
                let complete = world.inventory.is_some_and(|inventory| inventory.complete);
                return Dispatch::Ready(children_outcome(&path, items, complete, world, handles));
            }
            Dispatch::Slow(Box::new(move |cancel| {
                let inventory = StorageInventory::scan_with_cancel(&path, &home, cancel);
                Slow::Children(path, Box::new(inventory))
            }))
        }
        "folder_age" => Dispatch::Slow(Box::new(move |cancel| {
            let profile = crate::storage::folder_age(&path, 20_000, Duration::from_secs(2), cancel);
            Slow::Age(path, profile)
        })),
        "open_handles" => Dispatch::Slow(Box::new(move |_| {
            let owners = care::open_handle_owners(&path, Duration::from_secs(6));
            Slow::Owners(path, owners)
        })),
        "identify_owner" => Dispatch::Slow(Box::new(move |_| {
            let guess = care::identify_owner(&path, &home, Duration::from_secs(3));
            Slow::Owner(path, guess)
        })),
        "growth_history" => Dispatch::Ready(growth_outcome(&path, world)),
        "cleanup_rule" => Dispatch::Ready(cleanup_rule_outcome(&path, world, handles)),
        _ => Dispatch::Rejected("Unsupported folder tool.".into()),
    }
}

fn process_tool(
    name: &str,
    handle: &str,
    pid: u32,
    start: String,
    world: &ToolWorld<'_>,
) -> Dispatch {
    let current = world
        .processes
        .iter()
        .find(|process| process.pid == pid && process.start_time == start);
    match name {
        "process_details" => Dispatch::Ready(match current {
            Some(process) => process_outcome(handle, process, world),
            None => Outcome::complete(
                EvidenceKind::ProcessSample,
                format!("{handle} (PID {pid}) is no longer in the latest sample. A new process with the same name is not assumed to be the same work."),
            )
            .contradicting(&["process_persists"]),
        }),
        "sample_process" => {
            let handle = handle.to_string();
            let name = current
                .map(|process| process_name(&process.command))
                .unwrap_or_else(|| format!("PID {pid}"));
            Dispatch::Slow(Box::new(move |cancel| {
                let samples = care::sample_process(pid, &start, 3, Duration::from_secs(1), cancel);
                Slow::Samples(handle, name, samples)
            }))
        }
        _ => Dispatch::Rejected("Unsupported process tool.".into()),
    }
}

fn system_tool(
    name: &str,
    argument: &str,
    world: &ToolWorld<'_>,
    handles: &mut Handles,
) -> Dispatch {
    match name {
        "memory_state" => Dispatch::Ready(memory_outcome(world, handles)),
        "top_processes" => {
            Dispatch::Ready(top_processes_outcome(argument == "cpu", world, handles))
        }
        "disk_accounting" => Dispatch::Ready(disk_outcome(world)),
        "growth" => Dispatch::Ready(growth_comparison_outcome(argument, world, handles)),
        "volume_context" => Dispatch::Slow(Box::new(|cancel| {
            Slow::Observation(care::observe_volume_context(cancel))
        })),
        "request_trace" => Dispatch::NeedsApproval(Box::new(|cancel| {
            Slow::Observation(care::observe_fseventsd(cancel))
        })),
        "reference_notes" => {
            if world.online_research && matches!(argument, "fseventsd" | "fs_usage") {
                Dispatch::Slow(Box::new(|cancel| {
                    Slow::Notes(care::research_sources(cancel, true))
                }))
            } else {
                Dispatch::Ready(Outcome::complete(
                    EvidenceKind::Research,
                    format!("Bundled reference · {}", bundled_note(argument)),
                ))
            }
        }
        "list_findings" => Dispatch::Ready(findings_outcome(world, handles)),
        "cleanup_options" => Dispatch::Ready(cleanup_options_outcome(world, handles)),
        _ => Dispatch::Rejected("Unsupported tool.".into()),
    }
}

fn bundled_note(topic: &str) -> &'static str {
    match topic {
        "fseventsd" => {
            "FSEvents keeps per-volume event logs so apps can learn what changed. Heavy filesystem activity or an event backlog can raise fseventsd memory. It is a macOS system daemon and should not be terminated."
        }
        "fs_usage" => {
            "fs_usage reports filesystem system calls and page faults. PgIn/PgOut rows are paging observations, and a wide-mode suffix is a thread identifier, not a process ID."
        }
        "apfs_space" => {
            "APFS volumes in one container share free space. Local Time Machine snapshots and purgeable data occupy space that folder totals do not show; macOS can reclaim purgeable space when it is needed."
        }
        _ => {
            "macOS keeps memory busy with caches and compression, so high used memory or RSS alone is not a problem. Memory pressure (normal, warning, critical) and sustained swap-out are better signals of a shortage."
        }
    }
}

/// Turn a finished collector result into model-facing evidence.
pub fn finish(slow: Slow, world: &ToolWorld<'_>, handles: &mut Handles) -> Outcome {
    match slow {
        Slow::Children(path, inventory) => {
            let listed = inventory
                .roots
                .first()
                .and_then(|root| inventory.children.get(&root.path))
                .or_else(|| inventory.children.get(&path))
                .cloned()
                .unwrap_or_default();
            let mut outcome = children_outcome(&path, &listed, inventory.complete, world, handles);
            outcome.inventory = Some(inventory);
            outcome
        }
        Slow::Age(path, profile) => age_outcome(&path, profile, world),
        Slow::Owners(path, owners) => owners_outcome(&path, owners, world, handles),
        Slow::Owner(path, guess) => {
            let place = short_path(&path, world.home);
            Outcome::complete(
                EvidenceKind::VolumeContext,
                match guess.owner {
                    Some(owner) => format!(
                        "Likely owner of {place}: {owner} (from {}){}{}. A path heuristic, not proof.",
                        guess.basis,
                        guess
                            .app_path
                            .map(|app| format!(" · app at {app}"))
                            .unwrap_or_default(),
                        guess
                            .last_used
                            .map(|date| format!(" · last opened {date}"))
                            .unwrap_or_default()
                    ),
                    None => format!("No likely owner for {place}: {}.", guess.basis),
                },
            )
        }
        Slow::Samples(handle, name, samples) => samples_outcome(&handle, &name, samples),
        Slow::Observation(observation) => {
            let mut outcome = Outcome::unavailable(
                observation.kind,
                observation.status,
                if observation.kind == EvidenceKind::VolumeContext {
                    compress_mounts(&observation.summary)
                } else {
                    observation.summary.clone()
                },
            );
            outcome.supports = observation.supports.clone();
            outcome.contradicts = observation.contradicts.clone();
            if outcome.kind == EvidenceKind::VolumeContext && outcome.supports.is_empty() {
                let external = outcome.text.starts_with("Mounted now");
                if external {
                    outcome.supports.push("volume_specific".into());
                }
            }
            outcome
        }
        Slow::Notes(result) => match result {
            Ok(text) => Outcome::complete(EvidenceKind::Research, text),
            Err(error) => Outcome::unavailable(
                EvidenceKind::Research,
                EvidenceStatus::Failed,
                format!("Reference research unavailable: {error}"),
            ),
        },
    }
}

fn children_outcome(
    path: &Path,
    items: &[StorageItem],
    complete: bool,
    world: &ToolWorld<'_>,
    handles: &mut Handles,
) -> Outcome {
    let place = short_path(path, world.home);
    if items.is_empty() {
        return Outcome::complete(
            EvidenceKind::VolumeContext,
            format!("{place} has no measured children."),
        )
        .partial_if(!complete);
    }
    let total: u64 = items.iter().map(|item| item.size_kb).sum();
    let mut parts = Vec::new();
    for item in items.iter().take(6) {
        let name = truncate_middle(
            &ai::display_text(
                &item
                    .path
                    .file_name()
                    .unwrap_or(item.path.as_os_str())
                    .to_string_lossy(),
            ),
            32,
        );
        let handle = (item.kind == StorageItemKind::Directory)
            .then(|| handles.folder(&item.path))
            .flatten()
            .map(|handle| format!("{handle} "))
            .unwrap_or_default();
        let suffix = if item.kind == StorageItemKind::Directory {
            ""
        } else {
            " (file)"
        };
        parts.push(format!(
            "{handle}{name}{suffix} {} {}%",
            format_kb(item.size_kb),
            percent(item.size_kb, total)
        ));
    }
    if items.len() > 6 {
        let rest: u64 = items.iter().skip(6).map(|item| item.size_kb).sum();
        parts.push(format!("+{} more {}", items.len() - 6, format_kb(rest)));
    }
    Outcome::complete(
        EvidenceKind::VolumeContext,
        format!(
            "{place} holds {} in {} measured items{}: {}",
            format_kb(total),
            items.len(),
            if complete { "" } else { " (partial scan)" },
            parts.join(" · ")
        ),
    )
    .supporting(&["measured_children", "measured_consumers"])
    .partial_if(!complete)
}

fn age_outcome(path: &Path, profile: io::Result<AgeProfile>, world: &ToolWorld<'_>) -> Outcome {
    let place = short_path(path, world.home);
    let profile = match profile {
        Ok(profile) => profile,
        Err(error) => {
            return Outcome::unavailable(
                EvidenceKind::VolumeContext,
                EvidenceStatus::Failed,
                format!(
                    "Age check for {place} unavailable: {}",
                    ai::display_text(&error.to_string())
                ),
            );
        }
    };
    if profile.files == 0 {
        return Outcome::complete(
            EvidenceKind::VolumeContext,
            format!("{place} contains no readable files."),
        )
        .partial_if(!profile.complete);
    }
    let [recent, months, year, older] = profile.buckets_kb;
    let total = profile.total_kb;
    let newest = profile.newest_age_secs;
    let mut outcome = Outcome::complete(
        EvidenceKind::VolumeContext,
        format!(
            "{place}: {} files, {}. Changed within 7 days {}% · 7–90 days {}% · 90–365 days {}% · older {}%.{}{}",
            profile.files,
            format_kb(total),
            percent(recent, total),
            percent(months, total),
            percent(year, total),
            percent(older, total),
            newest
                .map(|age| format!(" Newest change {} ago.", duration_text(age)))
                .unwrap_or_default(),
            if profile.complete {
                String::new()
            } else {
                " Walk stopped early; totals are partial.".into()
            }
        ),
    )
    .partial_if(!profile.complete);
    if newest.is_some_and(|age| age <= 86_400) {
        outcome = outcome.supporting(&["active_writer"]);
    } else if percent(year + older, total) >= 50 && newest.is_some_and(|age| age > 7 * 86_400) {
        outcome = outcome.contradicting(&["active_writer"]);
    }
    outcome
}

fn owners_outcome(
    path: &Path,
    owners: io::Result<Vec<(u32, String)>>,
    world: &ToolWorld<'_>,
    handles: &mut Handles,
) -> Outcome {
    let place = short_path(path, world.home);
    match owners {
        Ok(owners) if owners.is_empty() => Outcome::complete(
            EvidenceKind::ProcessSample,
            format!("No process has files open under {place} right now (point-in-time check)."),
        )
        .contradicting(&["active_writer"]),
        Ok(owners) => {
            let listed = owners
                .iter()
                .take(5)
                .map(|(pid, command)| {
                    let handle = world
                        .processes
                        .iter()
                        .find(|process| process.pid == *pid)
                        .and_then(|process| handles.process(process.pid, &process.start_time))
                        .map(|handle| format!("{handle} "))
                        .unwrap_or_default();
                    format!("{handle}{} (PID {pid})", process_name(command))
                })
                .collect::<Vec<_>>()
                .join(", ");
            Outcome::complete(
                EvidenceKind::ProcessSample,
                format!(
                    "{} process{} hold files open under {place}: {listed}.",
                    owners.len(),
                    if owners.len() == 1 { "" } else { "es" }
                ),
            )
            .supporting(&["active_writer"])
        }
        Err(error) if error.kind() == io::ErrorKind::TimedOut => Outcome::unavailable(
            EvidenceKind::ProcessSample,
            EvidenceStatus::TimedOut,
            format!("Open-file check for {place} timed out; the folder may be very large."),
        ),
        Err(error) => Outcome::unavailable(
            EvidenceKind::ProcessSample,
            EvidenceStatus::Failed,
            format!(
                "Open-file check for {place} unavailable: {}",
                ai::display_text(&error.to_string())
            ),
        ),
    }
}

fn growth_comparison_outcome(
    period: &str,
    world: &ToolWorld<'_>,
    handles: &mut Handles,
) -> Outcome {
    let unavailable = |text: String| {
        Outcome::unavailable(
            EvidenceKind::VolumeContext,
            EvidenceStatus::Unsupported,
            text,
        )
    };
    let Some(current) = world.current.filter(|current| current.complete) else {
        return unavailable("Growth cannot be measured yet: the current assessment is incomplete. Finish a full assessment, then compare it with saved history.".into());
    };
    let days = match period {
        "day" => 1,
        "week" => 7,
        "month" => 30,
        _ => 0,
    };
    let Some(base) = crate::growth::baseline(world.history, current, days) else {
        let interval = if days == 0 {
            "earlier".into()
        } else {
            format!("at least {days} days old")
        };
        return unavailable(format!(
            "Growth cannot be measured: no comparable complete assessment of this volume {interval} is saved. Current folder sizes do not establish growth. Keep complete assessments to build history."
        ));
    };
    let deltas = crate::growth::diff(base, current);
    let since = crate::history::format_timestamp(UNIX_EPOCH + Duration::from_secs(base.updated));
    let text = if deltas.is_empty() {
        format!(
            "No significant growth or shrinkage in folders measured in both assessments since {since} (threshold: 500 MiB and 5%)."
        )
    } else {
        let changes = deltas
            .iter()
            .take(3)
            .map(|delta| {
                let path = Path::new(&delta.path);
                let handle = handles.folder(path).unwrap_or_default();
                format!(
                    "{handle} {} {}{} ({} → {})",
                    short_path(path, world.home),
                    if delta.change_kb() >= 0 { "+" } else { "−" },
                    format_kb(delta.change_kb().unsigned_abs()),
                    format_kb(delta.before_kb),
                    format_kb(delta.after_kb)
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "Changes since {since}: {changes}. Only folders measured in both complete assessments are compared."
        )
    };
    Outcome::complete(EvidenceKind::VolumeContext, cap(&text))
}

fn growth_outcome(path: &Path, world: &ToolWorld<'_>) -> Outcome {
    let place = short_path(path, world.home);
    let key = path.display().to_string();
    let values: Vec<(u64, u64)> = world
        .history
        .iter()
        .filter_map(|session| {
            session
                .measurements
                .get(&key)
                .map(|kb| (session.updated, *kb))
        })
        .take(3)
        .collect();
    if values.len() < 2 {
        return Outcome::unavailable(
            EvidenceKind::VolumeContext,
            EvidenceStatus::Unsupported,
            format!("No comparable complete history for {place} yet."),
        );
    }
    let listed = values
        .iter()
        .map(|(updated, kb)| {
            format!(
                "{} ({})",
                format_kb(*kb),
                crate::history::format_timestamp(UNIX_EPOCH + Duration::from_secs(*updated))
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let newest = values[0].1;
    let oldest = values[values.len() - 1].1;
    let outcome = Outcome::complete(
        EvidenceKind::VolumeContext,
        format!(
            "Saved complete measurements of {place}, newest first: {listed}. History does not identify the writer."
        ),
    );
    if newest > oldest.saturating_add(oldest / 10) {
        outcome.supporting(&["active_writer"])
    } else {
        outcome
    }
}

fn cleanup_rule_outcome(path: &Path, world: &ToolWorld<'_>, handles: &mut Handles) -> Outcome {
    let place = short_path(path, world.home);
    let mut matches: Vec<&CacheEntry> = world
        .entries
        .iter()
        .filter(|entry| entry.status != CacheStatus::Missing)
        .filter(|entry| entry.spec.path.starts_with(path) || path.starts_with(&entry.spec.path))
        .collect();
    matches.sort_by_key(|entry| std::cmp::Reverse(entry.size_kb));
    if matches.is_empty() {
        return Outcome::complete(
            EvidenceKind::Policy,
            format!("No cleanup rule covers {place}, its contents, or a parent. Size alone does not make data removable."),
        )
        .contradicting(&["rebuildable_data"]);
    }
    let mut supports = Vec::new();
    let mut contradicts = Vec::new();
    let described = matches
        .iter()
        .take(3)
        .map(|entry| {
            let relation = if entry.spec.path == path {
                "covers this folder"
            } else if entry.spec.path.starts_with(path) {
                "applies inside it"
            } else {
                "covers a parent"
            };
            let covering = entry.spec.path == path || path.starts_with(&entry.spec.path);
            match entry.status {
                CacheStatus::Ready | CacheStatus::Optional if covering => {
                    supports.push("rebuildable_data")
                }
                CacheStatus::InUse => supports.push("active_writer"),
                CacheStatus::Review | CacheStatus::Whitelisted if covering => {
                    contradicts.push("rebuildable_data")
                }
                _ => {}
            }
            let action = (world.suggest)(&Target::Cache(entry.spec.path.clone()))
                .and_then(|id| handles.action(&id))
                .map(|handle| format!(" · eligible action {handle}"))
                .unwrap_or_default();
            format!(
                "{} {relation} · {} · {} · {}{action}",
                entry.spec.label,
                entry.status.label(),
                format_kb(entry.size_kb),
                entry.spec.note
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    Outcome::complete(
        EvidenceKind::Policy,
        format!("Rules for {place}: {described}"),
    )
    .supporting(&supports)
    .contradicting(&contradicts)
}

fn process_outcome(handle: &str, process: &ProcessEntry, world: &ToolWorld<'_>) -> Outcome {
    let rss = world
        .metrics
        .rss_kb
        .get(&process.pid)
        .copied()
        .or(process.rss_kb)
        .map(format_kb)
        .unwrap_or_else(|| "unknown".into());
    let cpu = world
        .metrics
        .cpu
        .get(&process.pid)
        .map(|cpu| format!("{cpu:.1}% over {:.1}s", world.metrics.interval_secs))
        .unwrap_or_else(|| "not yet measured".into());
    let parent = world
        .processes
        .iter()
        .find(|candidate| candidate.pid == process.parent_pid)
        .map(|parent| process_name(&parent.command))
        .unwrap_or_else(|| "not in sample".into());
    let outcome = Outcome::complete(
        EvidenceKind::ProcessSample,
        format!(
            "{handle} {} · PID {} · {} · RSS {rss} · CPU {cpu} · running {} · parent {parent} (PID {}) · {}",
            process_name(&process.command),
            process.pid,
            process.health.label(),
            ai::display_text(&process.elapsed),
            process.parent_pid,
            if process.system_owned {
                "system-owned, read-only".to_string()
            } else if process.signalable {
                "current account, can be stopped only from a reviewed plan".to_string()
            } else {
                format!(
                    "protected: {}",
                    process.signal_block_reason.as_deref().unwrap_or("not signalable")
                )
            }
        ),
    )
    .supporting(&["process_persists"]);
    if process.health != ProcessHealth::Running {
        outcome.supporting(&["abnormal_process"])
    } else {
        outcome
    }
}

fn samples_outcome(
    handle: &str,
    name: &str,
    samples: Result<Vec<ProcessSample>, String>,
) -> Outcome {
    let samples = match samples {
        Ok(samples) => samples,
        Err(error) => {
            return Outcome::unavailable(
                EvidenceKind::ProcessSample,
                EvidenceStatus::Failed,
                format!(
                    "Sampling {handle} {name} unavailable: {}",
                    ai::display_text(&error)
                ),
            );
        }
    };
    let present = samples.iter().filter(|sample| sample.present).count();
    let cpu = samples
        .windows(2)
        .filter(|pair| pair[0].present && pair[1].present)
        .map(|pair| {
            format!(
                "{:.1}%",
                (pair[1].cpu_seconds - pair[0].cpu_seconds).max(0.) * 100.
            )
        })
        .collect::<Vec<_>>();
    let rss: Vec<u64> = samples
        .iter()
        .filter(|sample| sample.present)
        .map(|sample| sample.rss_kb)
        .collect();
    let state = samples
        .iter()
        .rev()
        .find(|sample| sample.present)
        .map(|sample| sample.state.clone())
        .unwrap_or_default();
    let text = format!(
        "{handle} {name}: present in {present}/{} one-second samples{}{}{}",
        samples.len(),
        if cpu.is_empty() {
            String::new()
        } else {
            format!(" · CPU {}", cpu.join(", "))
        },
        match (rss.first(), rss.last()) {
            (Some(first), Some(last)) =>
                format!(" · RSS {} → {}", format_kb(*first), format_kb(*last)),
            _ => String::new(),
        },
        if state.is_empty() {
            String::new()
        } else {
            format!(" · state {state}")
        }
    );
    let outcome = Outcome::complete(EvidenceKind::ProcessSample, text);
    if present == samples.len() && present > 0 {
        outcome.supporting(&["process_persists"])
    } else if present < samples.len() {
        outcome.contradicting(&["process_persists"])
    } else {
        outcome
    }
}

fn ranked_processes<'a>(
    world: &'a ToolWorld<'_>,
    cpu: bool,
    count: usize,
) -> Vec<(&'a ProcessEntry, f64)> {
    let mut ranked: Vec<(&ProcessEntry, f64)> = world
        .processes
        .iter()
        .filter_map(|process| {
            if cpu {
                world
                    .metrics
                    .cpu
                    .get(&process.pid)
                    .map(|value| (process, *value))
            } else {
                world
                    .metrics
                    .rss_kb
                    .get(&process.pid)
                    .copied()
                    .or(process.rss_kb)
                    .map(|kb| (process, kb as f64))
            }
        })
        .collect();
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1));
    ranked.truncate(count);
    ranked
}

fn memory_outcome(world: &ToolWorld<'_>, handles: &mut Handles) -> Outcome {
    let rate = |value: Option<f64>| {
        value
            .map(|value| format!("{value:.1} KiB/s"))
            .unwrap_or_else(|| "unmeasured".into())
    };
    let top = ranked_processes(world, false, 3);
    let listed = top
        .iter()
        .map(|(process, kb)| {
            format!(
                "{}{} {}",
                handles
                    .process(process.pid, &process.start_time)
                    .map(|handle| format!("{handle} "))
                    .unwrap_or_default(),
                process_name(&process.command),
                format_kb(*kb as u64)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let outcome = Outcome::complete(
        EvidenceKind::ProcessSample,
        format!(
            "Memory pressure {} · swap {} · swap in {} / out {} · largest memory users: {}. RSS is not reclaimable memory.",
            world.metrics.pressure_label(),
            ai::display_text(&world.metrics.swap),
            rate(world.metrics.swap_in_kb_s),
            rate(world.metrics.swap_out_kb_s),
            if listed.is_empty() { "unmeasured".into() } else { listed }
        ),
    )
    .partial_if(world.metrics.pressure.is_none());
    let subject_in_top = world
        .subject_pid
        .is_some_and(|pid| top.iter().any(|(process, _)| process.pid == pid));
    match world.metrics.pressure {
        Some(level) if level >= 2 && subject_in_top => {
            outcome.supporting(&["system_pressure_correlation"])
        }
        Some(1) => outcome.contradicting(&["system_pressure_correlation"]),
        _ => outcome,
    }
}

fn top_processes_outcome(cpu: bool, world: &ToolWorld<'_>, handles: &mut Handles) -> Outcome {
    let ranked = ranked_processes(world, cpu, 5);
    if ranked.is_empty() {
        return Outcome::unavailable(
            EvidenceKind::ProcessSample,
            EvidenceStatus::Partial,
            if cpu {
                "CPU has not been measured over an interval yet.".into()
            } else {
                "Memory readings are not available yet.".into()
            },
        );
    }
    let listed = ranked
        .iter()
        .map(|(process, value)| {
            format!(
                "{}{} {}",
                handles
                    .process(process.pid, &process.start_time)
                    .map(|handle| format!("{handle} "))
                    .unwrap_or_default(),
                process_name(&process.command),
                if cpu {
                    format!("{value:.1}%")
                } else {
                    format_kb(*value as u64)
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Outcome::complete(
        EvidenceKind::ProcessSample,
        if cpu {
            format!(
                "Top CPU over {:.1}s (100% = one core): {listed}. High CPU alone does not mean a process is stuck.",
                world.metrics.interval_secs
            )
        } else {
            format!("Top resident memory: {listed}. RSS is not reclaimable memory.")
        },
    )
}

fn disk_outcome(world: &ToolWorld<'_>) -> Outcome {
    let mut parts = Vec::new();
    if let Some(volume) = world.volume {
        parts.push(format!(
            "Volume {} · {} used · {} free{}",
            format_kb(volume.capacity_kb),
            format_kb(volume.disk_used_kb()),
            format_kb(volume.disk_free_kb()),
            if volume.container_free_kb.is_some() {
                " (shared APFS container)"
            } else {
                ""
            }
        ));
    }
    let Some(inventory) = world.inventory else {
        return if parts.is_empty() {
            Outcome::unavailable(
                EvidenceKind::VolumeContext,
                EvidenceStatus::Unsupported,
                "Disk accounting is not available yet.".into(),
            )
        } else {
            Outcome::unavailable(
                EvidenceKind::VolumeContext,
                EvidenceStatus::Partial,
                format!("{}. The folder scan has not finished.", parts.join(" · ")),
            )
        };
    };
    parts.push(format!(
        "folder scan measured {} ({}) · {} not visible to the scan · {} local Time Machine snapshots · {} unreadable entries",
        format_kb(inventory.scanned_kb),
        if inventory.complete { "complete" } else { "partial" },
        format_kb(inventory.unaccounted_kb),
        inventory.local_snapshots.len(),
        inventory.scan_errors
    ));
    let mut outcome = Outcome::complete(
        EvidenceKind::VolumeContext,
        format!("{}.", parts.join(" · ")),
    )
    .partial_if(!inventory.complete);
    if inventory.unaccounted_kb >= GIB_KB
        || !inventory.local_snapshots.is_empty()
        || inventory.scan_errors > 0
    {
        outcome = outcome.supporting(&["missing_coverage"]);
    } else if inventory.complete {
        outcome = outcome
            .contradicting(&["missing_coverage"])
            .supporting(&["measured_consumers"]);
    }
    outcome
}

/// Reduce raw `mount` output to the volumes that could matter to an investigation.
fn compress_mounts(summary: &str) -> String {
    let interesting: Vec<String> = summary
        .lines()
        .filter(|line| line.contains(" on /"))
        .filter(|line| {
            line.contains(" on /Volumes/")
                || !(line.contains("(apfs")
                    || line.contains("(devfs")
                    || line.contains("(autofs")
                    || line.starts_with("map "))
        })
        .filter_map(|line| {
            let (_, rest) = line.split_once(" on ")?;
            let (place, kind) = rest.split_once(" (")?;
            let kind = kind.split([',', ')']).next().unwrap_or_default();
            Some(format!(
                "{} ({kind})",
                truncate_middle(&ai::display_text(place), 40)
            ))
        })
        .take(6)
        .collect();
    if interesting.is_empty() {
        "Only internal APFS system volumes are mounted.".into()
    } else {
        format!(
            "Mounted now: {}. Presence alone does not establish activity.",
            interesting.join(", ")
        )
    }
}

/// The Data-volume walk and the explorer can name the same home folder through
/// different macOS mount aliases. Compare them without probing the filesystem.
fn logical_path(path: &Path) -> PathBuf {
    path.strip_prefix("/System/Volumes/Data")
        .map(|suffix| Path::new("/").join(suffix))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn measured_children<'a>(
    inventory: &'a StorageInventory,
    path: &Path,
) -> Option<&'a [StorageItem]> {
    inventory
        .children
        .get(path)
        .or_else(|| inventory.children.get(&logical_path(path)))
        .or_else(|| {
            let logical = logical_path(path);
            let alias = Path::new("/System/Volumes/Data").join(logical.strip_prefix("/").ok()?);
            inventory.children.get(&alias)
        })
        .map(Vec::as_slice)
}

fn cleanup_options_outcome(world: &ToolWorld<'_>, handles: &mut Handles) -> Outcome {
    let on_volume = |path: &Path| {
        world.inventory.is_none_or(|inventory| {
            let Some(volume) = &inventory.volume else {
                return true;
            };
            let path = logical_path(path);
            inventory
                .roots
                .iter()
                .filter(|root| path.starts_with(logical_path(&root.path)))
                .max_by_key(|root| {
                    (
                        logical_path(&root.path).components().count(),
                        root.path == Path::new("/System/Volumes/Data"),
                    )
                })
                // An unscanned path has no device evidence either way.
                .is_none_or(|root| root.device == volume.device)
        })
    };
    let mut eligible: Vec<_> = world
        .entries
        .iter()
        .filter_map(|entry| {
            if entry.size_kb == 0
                || !matches!(entry.status, CacheStatus::Ready | CacheStatus::Optional)
                || !on_volume(&entry.spec.path)
            {
                return None;
            }
            (world.suggest)(&Target::Cache(entry.spec.path.clone()))
                .filter(|id| id.starts_with("clean:"))
                .map(|id| (entry, id))
        })
        .collect();
    // Keep outermost paths so nested rules and duplicate findings cannot inflate
    // the estimate. Sort the remaining rules by size for the model's next check.
    eligible.sort_by_key(|(entry, _)| logical_path(&entry.spec.path).components().count());
    let mut unique: Vec<(&CacheEntry, String)> = Vec::new();
    for (entry, id) in eligible {
        if !unique.iter().any(|(parent, _)| {
            logical_path(&entry.spec.path).starts_with(logical_path(&parent.spec.path))
        }) {
            unique.push((entry, id));
        }
    }
    unique.sort_by_key(|(entry, _)| std::cmp::Reverse(entry.size_kb));
    let total = unique
        .iter()
        .fold(0u64, |sum, (entry, _)| sum.saturating_add(entry.size_kb));
    let mut text = format!(
        "Eligible cleanup: {} across {} rules; estimated, not guaranteed free space.",
        format_kb(total),
        unique.len()
    );

    // Findings may be ranked behind dozens of caches and processes. Inspect the
    // measured folder index as well so a multi-GiB project remains reachable.
    let mut candidates: Vec<(PathBuf, u64)> = world
        .findings
        .iter()
        .filter_map(|finding| match &finding.target {
            Target::Folder(path)
                if world
                    .inventory
                    .is_none_or(|inventory| inventory.children.contains_key(path)) =>
            {
                Some((path.clone(), finding.size_kb))
            }
            _ => None,
        })
        .collect();
    if let Some(inventory) = world.inventory {
        let targets: Vec<_> = world
            .entries
            .iter()
            .map(|entry| entry.spec.path.clone())
            .collect();
        let breakdown = crate::storage::breakdown(inventory, &targets, 1);
        candidates.extend(breakdown.items.into_iter().filter_map(|item| {
            if item.kind == StorageItemKind::Directory {
                Some((item.path, item.size_kb))
            } else {
                let parent = item.path.parent()?;
                let children = inventory.children.get(parent)?;
                Some((
                    parent.to_path_buf(),
                    children
                        .iter()
                        .fold(0u64, |sum, child| sum.saturating_add(child.size_kb)),
                ))
            }
        }));
        // Keep the explorer's current folder even if the overview opens it into
        // more specific descendants or it came from a later on-demand scan.
        if let Some(path) = handles.folders.first()
            && let Some(children) = measured_children(inventory, path)
        {
            candidates.push((
                path.clone(),
                children
                    .iter()
                    .fold(0u64, |sum, item| sum.saturating_add(item.size_kb)),
            ));
        }
    }
    candidates.retain(|(path, size)| {
        let path = logical_path(path);
        *size > 0
            && path.starts_with(logical_path(world.home))
            && path != logical_path(world.home)
            && on_volume(&path)
            && !world
                .entries
                .iter()
                .any(|entry| path.starts_with(logical_path(&entry.spec.path)))
    });
    let selected = handles.folders.first().map(|path| logical_path(path));
    candidates.sort_by(|a, b| {
        let a_selected = selected.as_ref() == Some(&logical_path(&a.0));
        let b_selected = selected.as_ref() == Some(&logical_path(&b.0));
        b_selected
            .cmp(&a_selected)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut review: Vec<PathBuf> = Vec::new();
    let coverage = if world
        .inventory
        .is_some_and(|inventory| inventory.complete && inventory.scan_errors == 0)
    {
        ""
    } else {
        " (partial scan)"
    };
    text.push_str(&format!(
        " Inspect folders{coverage}; size is not approved cleanup:"
    ));
    for (path, size) in candidates {
        if review.len() == 2 {
            break;
        }
        if review.iter().any(|known| {
            logical_path(&path).starts_with(logical_path(known))
                || logical_path(known).starts_with(logical_path(&path))
        }) {
            continue;
        }
        let mut issued = handles.clone();
        if let Some(handle) = issued.folder(&path) {
            let mut part = format!(
                " {handle} {} {}",
                truncate_middle(
                    &short_path(&logical_path(&path), &logical_path(world.home)),
                    38
                ),
                format_kb(size)
            );
            if let Some(child) = world
                .inventory
                .and_then(|inventory| measured_children(inventory, &path))
                .and_then(|children| {
                    children
                        .iter()
                        .filter(|child| child.size_kb > 0)
                        .max_by_key(|child| child.size_kb)
                })
            {
                let name = truncate_middle(
                    &ai::display_text(
                        &child
                            .path
                            .file_name()
                            .unwrap_or(child.path.as_os_str())
                            .to_string_lossy(),
                    ),
                    24,
                );
                let hint = format!(" (largest: {name} {})", format_kb(child.size_kb));
                if text.len() + part.len() + hint.len() < OUTPUT_CAP - 145 {
                    part.push_str(&hint);
                }
            }
            part.push(';');
            // Leave room for at least one cache and the tool-reply envelope.
            if text.len() + part.len() <= OUTPUT_CAP - 145 {
                text.push_str(&part);
                review.push(path);
                *handles = issued;
            }
        }
    }
    if review.is_empty() {
        text.push_str(" none measured.");
    }
    text.push_str(" Eligible caches:");
    for (entry, id) in unique.iter().take(3) {
        let mut issued = handles.clone();
        let Some(folder) = issued.folder(&entry.spec.path) else {
            continue;
        };
        let Some(action) = issued.action(id) else {
            continue;
        };
        let part = format!(
            " {folder} {} {} {action};",
            truncate_middle(&ai::display_text(entry.spec.label), 24),
            format_kb(entry.size_kb)
        );
        if text.len() + part.len() <= OUTPUT_CAP - 45 {
            text.push_str(&part);
            *handles = issued;
        }
    }
    if unique.is_empty() {
        text.push_str(" none.");
    }
    let mut outcome =
        Outcome::complete(EvidenceKind::Policy, text).partial_if(!coverage.is_empty());
    outcome.cleanup_total_kb = Some(total);
    outcome
}

fn findings_outcome(world: &ToolWorld<'_>, handles: &mut Handles) -> Outcome {
    let listed = world
        .findings
        .iter()
        .take(8)
        .map(|finding| {
            let handle = match &finding.target {
                Target::Cache(path) | Target::Folder(path) => handles.folder(path),
                Target::Process(pid, start) => handles.process(*pid, start),
                Target::System => None,
            }
            .map(|handle| format!("{handle} "))
            .unwrap_or_default();
            let kind = match &finding.target {
                _ if finding.quick_win => "quick win",
                Target::Cache(_) => "cleanup rule",
                Target::Folder(_) => "folder",
                Target::Process(..) => "process",
                Target::System => "system reading",
            };
            // An unscoped findings list cannot establish which process the
            // user intends to act on. Targeted investigations issue those IDs.
            let action = (!matches!(finding.target, Target::Process(..)))
                .then(|| (world.suggest)(&finding.target))
                .flatten()
                .and_then(|id| handles.action(&id))
                .map(|handle| format!(" · eligible action {handle}"))
                .unwrap_or_default();
            format!(
                "{handle}{}{} · {kind}{action}",
                truncate_middle(&ai::display_text(&finding.title), 40),
                if finding.size_kb > 0 {
                    format!(" {}", format_kb(finding.size_kb))
                } else {
                    String::new()
                }
            )
        })
        .collect::<Vec<_>>();
    if listed.is_empty() {
        return Outcome::complete(
            EvidenceKind::Policy,
            "No measured findings yet; the assessment may still be running.".into(),
        );
    }
    Outcome::complete(
        EvidenceKind::Policy,
        format!("Current findings: {}", listed.join("; ")),
    )
}

/// Seconds since the Unix epoch, for evidence timestamps.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cache::{CacheSpec, CacheTier, PathIdentity},
        storage::StorageCategory,
    };

    fn world<'a>(
        home: &'a Path,
        inventory: Option<&'a StorageInventory>,
        entries: &'a [CacheEntry],
        processes: &'a [ProcessEntry],
        metrics: &'a Metrics,
        suggest: &'a dyn Fn(&Target) -> Option<String>,
    ) -> ToolWorld<'a> {
        ToolWorld {
            home,
            inventory,
            entries,
            processes,
            metrics,
            findings: &[],
            history: &[],
            current: None,
            volume: None,
            online_research: false,
            subject_pid: None,
            suggest,
        }
    }

    fn item(path: &Path, size_kb: u64, kind: StorageItemKind) -> StorageItem {
        StorageItem {
            path: path.to_path_buf(),
            size_kb,
            kind,
            category: StorageCategory::DeveloperData,
        }
    }

    fn inventory(children: Vec<(PathBuf, Vec<StorageItem>)>) -> StorageInventory {
        StorageInventory {
            volume: None,
            volume_error: None,
            roots: vec![],
            scanned_kb: 0,
            scanned_on_volume_kb: 0,
            unaccounted_kb: 0,
            inventory_overage_kb: 0,
            local_snapshots: vec![],
            scanned_items: 0,
            scan_errors: 0,
            scan_error_paths: vec![],
            complete: true,
            top_level: vec![],
            largest: vec![],
            children: children.into_iter().collect(),
        }
    }

    fn cache_entry(path: &Path, size_kb: u64, status: CacheStatus) -> CacheEntry {
        CacheEntry {
            spec: CacheSpec {
                label: "fixture cache",
                path: path.to_path_buf(),
                ..crate::cache::scan_specs(Path::new("/Users/demo"), Path::new("/Users/demo"))[0]
                    .clone()
            },
            status,
            size_kb,
            outcome: None,
            identity: None,
        }
    }

    fn folder_finding(path: &Path, size_kb: u64) -> Finding {
        Finding {
            id: format!("folder:{}", path.display()),
            title: path.display().to_string(),
            observation: String::new(),
            consequence: String::new(),
            size_kb,
            quick_win: false,
            target: Target::Folder(path.to_path_buf()),
            related_pids: vec![],
        }
    }

    #[test]
    fn toolsets_fit_the_context_budget_and_hide_tracing_from_automation() {
        for set in [
            Toolset::Storage,
            Toolset::Capacity,
            Toolset::Process,
            Toolset::Fseventsd,
            Toolset::Ask,
        ] {
            let tools = set.tools(false);
            assert!(tools.len() <= 6, "{set:?} has too many tools");
            let bytes: usize = tools
                .iter()
                .map(|tool| spec_json(tool).to_string().len())
                .sum();
            assert!(bytes < 1_600, "{set:?} tool definitions use {bytes} bytes");
            assert!(
                !set.tools(true)
                    .iter()
                    .any(|tool| tool.name == "request_trace")
            );
            assert!(
                !set.fallback(true)
                    .iter()
                    .any(|(name, _)| *name == "request_trace")
            );
            for (name, _) in set.fallback(false) {
                assert!(
                    tools.iter().any(|tool| tool.name == name),
                    "{set:?} fallback uses {name}"
                );
            }
        }
        assert!(
            TOOLS
                .iter()
                .all(|tool| spec_json(tool)["description"].as_str().unwrap().len() <= 110)
        );
    }

    #[test]
    fn cleanup_requests_include_totals_and_safety_tools_within_budget() {
        for question in [
            "how can i safely and quickly release 10gb of disk space?",
            "Free up 10 GiB",
            "Reclaim storage",
            "Recover disk space",
            "Which caches can I clear safely?",
        ] {
            let tools = Toolset::Ask.tools_for_question(false, Some(question));
            assert_eq!(tools.len(), 6);
            for name in [
                "cleanup_options",
                "list_children",
                "list_findings",
                "open_handles",
                "cleanup_rule",
            ] {
                assert!(
                    tools.iter().any(|tool| tool.name == name),
                    "{question}: {name}"
                );
            }
            assert!(
                tools
                    .iter()
                    .map(|tool| spec_json(tool).to_string().len())
                    .sum::<usize>()
                    < 1600
            );
            assert_eq!(
                Toolset::Ask.fallback_for_question(false, Some(question))[0].0,
                "cleanup_options"
            );
        }
        for question in ["Free up memory", "Release 10 GB of RAM"] {
            assert!(!is_cleanup_question(question));
        }
        assert_eq!(
            cleanup_goal_kb("release 10gb of disk space"),
            Some(9_765_625)
        );
        assert_eq!(cleanup_goal_kb("free 1.5 GiB"), Some(1_572_864));
        assert_eq!(cleanup_goal_kb("free 500 MB"), Some(488_282));
        assert_eq!(cleanup_goal_kb("free 0 GB"), None);
        assert_eq!(cleanup_goal_kb("free disk space"), None);
        assert_eq!(cleanup_goal_kb("release 10 GB of RAM"), None);
        assert_eq!(
            cleanup_goal_kb("I have 1.9GB free and need 10GB of disk space"),
            Some(9_765_625)
        );
        assert_eq!(
            cleanup_goal_kb("Free 10GB of disk space; only 1.9GB is left"),
            Some(9_765_625)
        );
    }

    #[test]
    fn cleanup_goal_converts_decimal_and_binary_units_and_rounds_up() {
        for (question, expected) in [
            ("Free 1 MB of disk space", 977),
            ("Free 1 MiB of disk space", 1024),
            ("Free 1 GB of disk space", 976_563),
            ("Free 1 GiB of disk space", GIB_KB),
            ("Free 1 TB of disk space", 976_562_500),
            ("Free 1 TiB of disk space", 1024 * GIB_KB),
            ("RECLAIM 0.5\tGiB", GIB_KB / 2),
            ("Free .5 GiB of disk space", GIB_KB / 2),
            ("Free 0.000001 GB of disk space", 1),
        ] {
            assert_eq!(cleanup_goal_kb(question), Some(expected), "{question}");
        }
    }

    #[test]
    fn cleanup_goal_rejects_malformed_signed_and_ambiguous_amounts() {
        for question in [
            "Free -10 GB of disk space",
            "Free +10 GB of disk space",
            "Free 1..5 GB of disk space",
            "Free 1,5 GB of disk space",
            "Free 5-10 GB of disk space",
            "Free 5–10 GB of disk space",
            "Free 1e3 GB of disk space",
            "Free x10 GB of disk space",
            "Free 10 GB or 20 GB of disk space",
            "Free 999999999999999999999999999 GB of disk space",
            "Free 10 GBs of disk space",
            "Free 10 KB of disk space",
            "Free 0 GB of disk space",
        ] {
            assert_eq!(cleanup_goal_kb(question), None, "{question}");
        }
    }

    #[test]
    fn cleanup_goal_distinguishes_reported_free_space_from_the_requested_amount() {
        for question in [
            "Clean caches; I have 1.9 GB free",
            "How can I free disk space? I have 1.9 GB free",
            "Free disk space on my 500 GB drive",
            "Clear caches with 1.9 GB available",
        ] {
            assert_eq!(cleanup_goal_kb(question), None, "{question}");
        }
        for question in [
            "I have 1.9GB free and need 10GB of disk space",
            "Free 10GB of disk space; only 1.9GB is left",
            "My 500 GB drive has 1.9 GB available; I want to reclaim 10 GB",
        ] {
            assert_eq!(cleanup_goal_kb(question), Some(9_765_625), "{question}");
        }
    }

    #[test]
    fn cleanup_overview_requires_status_size_and_callback_eligibility() {
        let home = Path::new("/Users/demo");
        let mut entries: Vec<_> = [
            CacheStatus::Ready,
            CacheStatus::Optional,
            CacheStatus::Review,
            CacheStatus::InUse,
            CacheStatus::ScanError,
            CacheStatus::Symlink,
            CacheStatus::Invalid,
            CacheStatus::Missing,
            CacheStatus::Whitelisted,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, status)| {
            cache_entry(
                &home.join(format!("Library/Caches/rule-{index}")),
                1024,
                status,
            )
        })
        .collect();
        for name in ["denied", "foreign", "empty"] {
            entries.push(cache_entry(
                &home.join(format!("Library/Caches/{name}")),
                if name == "empty" { 0 } else { 100 * GIB_KB },
                CacheStatus::Ready,
            ));
        }
        let suggest = |target: &Target| match target {
            Target::Cache(path) if path.ends_with("denied") => None,
            Target::Cache(path) if path.ends_with("foreign") => Some("signal:42:start:TERM".into()),
            Target::Cache(path) => Some(format!("clean:{}", path.display())),
            _ => None,
        };
        let metrics = Metrics::default();
        let world = world(home, None, &entries, &[], &metrics, &suggest);
        let mut handles = Handles::default();
        let outcome = cleanup_options_outcome(&world, &mut handles);
        assert_eq!(outcome.cleanup_total_kb, Some(2048));
        assert_eq!(handles.actions.len(), 2);
        assert_eq!(handles.folders.len(), 2);
        assert!(outcome.text.contains("across 2 rules"), "{}", outcome.text);
        for (index, path) in handles.folders.iter().enumerate() {
            assert!(path.ends_with(format!("rule-{index}")));
            assert!(outcome.text.contains(&format!("n{} ", index + 1)));
            assert!(outcome.text.contains(&format!(" A{};", index + 1)));
        }
    }

    #[test]
    fn cleanup_totals_deduplicate_aliases_and_nested_rules_in_any_order() {
        let home = Path::new("/Users/demo");
        let entries = [
            cache_entry(
                &home.join("Library/Caches/pip/http"),
                80,
                CacheStatus::Ready,
            ),
            cache_entry(
                Path::new("/System/Volumes/Data/Users/demo/Library/Caches/pip"),
                100,
                CacheStatus::Ready,
            ),
            cache_entry(&home.join("Library/Caches/pip"), 100, CacheStatus::Ready),
            cache_entry(&home.join("Library/Caches/pip2"), 40, CacheStatus::Ready),
        ];
        let metrics = Metrics::default();
        for order in [
            vec![0, 1, 2, 3],
            vec![3, 2, 1, 0],
            vec![1, 0, 3, 2],
            vec![0, 1, 3],
        ] {
            let ordered: Vec<_> = order
                .into_iter()
                .map(|index| entries[index].clone())
                .collect();
            let suggest = |target: &Target| care::suggestion_id(&ordered, &[], target);
            let world = world(home, None, &ordered, &[], &metrics, &suggest);
            let mut handles = Handles::default();
            let outcome = cleanup_options_outcome(&world, &mut handles);
            assert_eq!(outcome.cleanup_total_kb, Some(140), "{}", outcome.text);
            assert_eq!(handles.actions.len(), 2);
            assert!(!handles.actions.iter().any(|id| id.ends_with("/http")));
        }
    }

    #[test]
    fn cleanup_totals_saturate_and_only_expose_the_largest_three_actions() {
        let home = Path::new("/Users/demo");
        let entries: Vec<_> = (0..5)
            .map(|index| {
                cache_entry(
                    &home.join(format!("Library/Caches/rule-{index}")),
                    u64::MAX - index,
                    CacheStatus::Ready,
                )
            })
            .collect();
        let metrics = Metrics::default();
        let suggest = |target: &Target| care::suggestion_id(&entries, &[], target);
        let world = world(home, None, &entries, &[], &metrics, &suggest);
        let mut handles = Handles::default();
        let outcome = cleanup_options_outcome(&world, &mut handles);
        assert_eq!(outcome.cleanup_total_kb, Some(u64::MAX));
        assert_eq!(handles.actions.len(), 3);
        for (index, action) in handles.actions.iter().enumerate() {
            assert!(action.ends_with(&format!("/rule-{index}")));
        }
        assert!(outcome.text.len() <= OUTPUT_CAP - 45);
    }

    #[test]
    fn cleanup_overview_keeps_large_debug_output_reachable_behind_many_findings() {
        let home = Path::new("/Users/demo");
        let debug = home.join("code/project/target/debug");
        let metrics = Metrics::default();
        let none = |_: &Target| None;
        let mut findings: Vec<_> = (0..40)
            .map(|index| Finding {
                target: Target::Process(index + 1, "start".into()),
                ..folder_finding(&home.join(format!("process-{index}")), 100)
            })
            .collect();
        findings.push(folder_finding(&debug, 14 * GIB_KB));
        let inventory = inventory(vec![(debug.clone(), vec![])]);
        let mut world = world(home, Some(&inventory), &[], &[], &metrics, &none);
        world.findings = &findings;
        let mut handles = Handles::default();
        let outcome = cleanup_options_outcome(&world, &mut handles);
        assert_eq!(outcome.cleanup_total_kb, Some(0));
        assert_eq!(handles.folder_path("n1").unwrap(), debug);
        assert!(outcome.text.contains("debug 14.0 GiB"), "{}", outcome.text);
        assert!(outcome.text.contains("size is not approved cleanup"));
        assert!(handles.actions.is_empty());
        assert!(handles.processes.is_empty());
    }

    #[test]
    fn cleanup_overview_keeps_measured_target_context_in_project_summary() {
        let home = Path::new("/Users/demo");
        let project = home.join("code/project");
        let metrics = Metrics::default();
        let none = |_: &Target| None;
        let findings = [folder_finding(&project, 120 * 1024 + 4)];
        for (alias, target_name) in [
            (false, "target"),
            (true, "target"),
            (true, "target\u{202e}\u{1b}"),
        ] {
            let measured = if alias {
                Path::new("/System/Volumes/Data").join(project.strip_prefix("/").unwrap())
            } else {
                project.clone()
            };
            let target = measured.join(target_name);
            let debug = target.join("debug");
            let mut inventory = inventory(vec![
                (
                    measured.clone(),
                    vec![
                        item(&measured.join("Cargo.toml"), 4, StorageItemKind::File),
                        item(&target, 120 * 1024, StorageItemKind::Directory),
                    ],
                ),
                (
                    target.clone(),
                    vec![item(&debug, 120 * 1024, StorageItemKind::Directory)],
                ),
                (
                    debug,
                    vec![item(
                        &target.join("debug/output.bin"),
                        120 * 1024,
                        StorageItemKind::File,
                    )],
                ),
            ]);
            inventory.top_level = vec![item(&project, 120 * 1024 + 4, StorageItemKind::Directory)];
            let mut world = world(home, Some(&inventory), &[], &[], &metrics, &none);
            world.findings = &findings;
            let mut handles = Handles::default();
            let outcome = cleanup_options_outcome(&world, &mut handles);
            assert!(
                outcome.text.contains("~/code/project 120.0 MiB"),
                "{}",
                outcome.text
            );
            assert!(
                outcome.text.contains("largest: target 120.0 MiB"),
                "{}",
                outcome.text
            );
            assert!(outcome.text.contains("size is not approved cleanup"));
            assert!(!outcome.text.contains(['\u{202e}', '\u{1b}']));
            assert_eq!(outcome.cleanup_total_kb, Some(0));
            assert_eq!(handles.folders, vec![project.clone()]);
            assert!(handles.actions.is_empty());
            assert!(outcome.text.len() <= OUTPUT_CAP - 45);
        }
    }

    #[test]
    fn cleanup_overview_finds_selected_folder_through_either_data_volume_alias() {
        let home = Path::new("/Users/demo");
        let canonical = home.join("code/project/target");
        let alias = Path::new("/System/Volumes/Data").join(canonical.strip_prefix("/").unwrap());
        let metrics = Metrics::default();
        let none = |_: &Target| None;
        for (selected, measured) in [(&canonical, &alias), (&alias, &canonical)] {
            let inventory = inventory(vec![(
                measured.clone(),
                vec![item(
                    &measured.join("debug"),
                    14 * GIB_KB,
                    StorageItemKind::Directory,
                )],
            )]);
            let world = world(home, Some(&inventory), &[], &[], &metrics, &none);
            let mut handles = Handles::default();
            handles.folder(selected);
            let outcome = cleanup_options_outcome(&world, &mut handles);
            assert!(
                outcome
                    .text
                    .contains(" n1 ~/code/project/target 14.0 GiB (largest: debug 14.0 GiB);"),
                "{}",
                outcome.text
            );
            assert_eq!(handles.folders, vec![selected.clone()]);
            assert_eq!(outcome.cleanup_total_kb, Some(0));
            assert!(handles.actions.is_empty());
        }
    }

    #[test]
    fn cleanup_overview_reports_partial_measurements_and_skips_overlapping_review_folders() {
        let home = Path::new("/Users/demo");
        let project = home.join("code/project");
        let parent = home.join("code");
        let other = home.join("Movies");
        let cache = home.join("Library/Caches/pip");
        let entries = [cache_entry(&cache, 1024, CacheStatus::Review)];
        let findings = [
            folder_finding(&parent, 30 * GIB_KB),
            folder_finding(&project, 14 * GIB_KB),
            folder_finding(&other, 20 * GIB_KB),
            folder_finding(&cache.join("http"), 50 * GIB_KB),
            folder_finding(home, 100 * GIB_KB),
            folder_finding(Path::new("/Users/other/Documents"), 100 * GIB_KB),
        ];
        let metrics = Metrics::default();
        let none = |_: &Target| None;
        for (complete, scan_errors, expected) in [
            (true, 0, EvidenceStatus::Complete),
            (false, 0, EvidenceStatus::Partial),
            (true, 1, EvidenceStatus::Partial),
        ] {
            let mut inventory = inventory(
                findings
                    .iter()
                    .filter_map(|finding| match &finding.target {
                        Target::Folder(path) => Some((path.clone(), vec![])),
                        _ => None,
                    })
                    .collect(),
            );
            inventory.complete = complete;
            inventory.scan_errors = scan_errors;
            let mut world = world(home, Some(&inventory), &entries, &[], &metrics, &none);
            world.findings = &findings;
            let mut handles = Handles::default();
            let outcome = cleanup_options_outcome(&world, &mut handles);
            assert_eq!(outcome.status, expected);
            assert_eq!(handles.folders, vec![parent.clone(), other.clone()]);
            assert_eq!(outcome.cleanup_total_kb, Some(0));
            assert!(handles.actions.is_empty());
            assert_eq!(
                outcome.text.contains("partial scan"),
                expected == EvidenceStatus::Partial
            );
        }
    }

    #[test]
    fn cleanup_overview_unicode_budget_issues_only_visible_handles() {
        let home = Path::new("/Users/demo");
        let entries: Vec<_> = (0..8)
            .map(|index| CacheEntry {
                spec: CacheSpec {
                    label: "🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹🧹",
                    ..cache_entry(
                        &home.join(format!("cache-{index}")),
                        1024,
                        CacheStatus::Ready,
                    )
                    .spec
                },
                ..cache_entry(
                    &home.join(format!("cache-{index}")),
                    1024,
                    CacheStatus::Ready,
                )
            })
            .collect();
        let findings: Vec<_> = (0..4)
            .map(|index| {
                folder_finding(
                    &home.join(format!("{index}-{}", "🧰".repeat(40))),
                    14 * GIB_KB,
                )
            })
            .collect();
        let metrics = Metrics::default();
        let suggest = |target: &Target| care::suggestion_id(&entries, &[], target);
        let mut world = world(home, None, &entries, &[], &metrics, &suggest);
        world.findings = &findings;
        let mut handles = Handles::default();
        let outcome = cleanup_options_outcome(&world, &mut handles);
        let model_text = cap(&outcome.text);
        assert_eq!(model_text, outcome.text);
        assert!(model_text.len() <= OUTPUT_CAP - 45);
        assert_eq!(outcome.cleanup_total_kb, Some(8 * 1024));
        assert!(!handles.actions.is_empty());
        assert!(
            handles.actions.len() < 3,
            "fixture must exceed the cache display budget"
        );
        for index in 1..=handles.folders.len() {
            assert!(model_text.contains(&format!(" n{index} ")), "{model_text}");
        }
        for index in 1..=handles.actions.len() {
            assert!(model_text.contains(&format!(" A{index};")), "{model_text}");
        }
    }

    #[test]
    fn cleanup_overview_handle_exhaustion_does_not_issue_invisible_references() {
        let home = Path::new("/Users/demo");
        let entries = [cache_entry(
            &home.join("Library/Caches/pip"),
            1024,
            CacheStatus::Ready,
        )];
        let metrics = Metrics::default();
        let suggest = |target: &Target| care::suggestion_id(&entries, &[], target);
        let world = world(home, None, &entries, &[], &metrics, &suggest);
        for exhausted_kind in ["folder", "action"] {
            let mut handles = Handles::default();
            for index in 0..MAX_HANDLES {
                if exhausted_kind == "folder" {
                    handles.folder(&home.join(format!("existing-{index}")));
                } else {
                    handles.action(&format!("clean:/existing-{index}"));
                }
            }
            let before_folders = handles.folders.clone();
            let before_actions = handles.actions.clone();
            let outcome = cleanup_options_outcome(&world, &mut handles);
            assert_eq!(handles.folders, before_folders, "{exhausted_kind}");
            assert_eq!(handles.actions, before_actions, "{exhausted_kind}");
            assert_eq!(outcome.cleanup_total_kb, Some(1024));
            assert!(!outcome.text.contains(" A"));
        }
    }

    #[test]
    fn cleanup_totals_exclude_blocked_overlapping_and_review_data() {
        let home = Path::new("/Users/demo");
        let entry = |name: &str, size_kb, status| CacheEntry {
            spec: CacheSpec {
                label: "fixture cache",
                path: home.join(name),
                ..crate::cache::scan_specs(home, home)[0].clone()
            },
            status,
            size_kb,
            outcome: None,
            identity: None,
        };
        let entries = vec![
            entry("Library/Caches/pip", 125 * 1024, CacheStatus::Ready),
            entry("Library/Caches/pip/http", 100 * 1024, CacheStatus::Ready),
            entry("Library/Caches/brew", 250 * 1024, CacheStatus::Optional),
            entry("Library/Caches/busy", 20 * GIB_KB, CacheStatus::InUse),
            entry("Library/Caches/kept", 20 * GIB_KB, CacheStatus::Whitelisted),
            entry("Library/Archives", 20 * GIB_KB, CacheStatus::Review),
            entry("Library/Caches/failed", 20 * GIB_KB, CacheStatus::ScanError),
        ];
        let project = home.join("code/project/target");
        let mut inventory = inventory(vec![(
            project.clone(),
            vec![item(
                &project.join("debug"),
                14 * GIB_KB,
                StorageItemKind::Directory,
            )],
        )]);
        inventory.complete = false;
        let metrics = Metrics::default();
        let suggest = |target: &Target| care::suggestion_id(&entries, &[], target);
        let world = world(home, Some(&inventory), &entries, &[], &metrics, &suggest);
        let mut handles = Handles::default();
        handles.folder(&project);
        let outcome = cleanup_options_outcome(&world, &mut handles);
        assert_eq!(outcome.cleanup_total_kb, Some(375 * 1024));
        assert_eq!(outcome.status, EvidenceStatus::Partial);
        assert!(outcome.text.contains("375.0 MiB"), "{}", outcome.text);
        assert!(outcome.text.contains("target 14.0 GiB"), "{}", outcome.text);
        assert!(outcome.text.contains("not approved cleanup"));
        assert!(outcome.text.contains("partial scan"));
        assert!(outcome.text.len() <= OUTPUT_CAP - 45);
        assert_eq!(handles.actions.len(), 2);
        assert!(!handles.actions.iter().any(|id| id.contains("target")));
        assert_eq!(
            handles.action_id("A1"),
            Some("clean:/Users/demo/Library/Caches/brew")
        );
    }

    #[test]
    fn cleanup_overview_sees_data_volume_aliases_and_excludes_other_volumes() {
        let home = Path::new("/Users/demo");
        let project = Path::new("/System/Volumes/Data/Users/demo/code/project/target");
        let mut inventory = inventory(vec![(
            project.to_path_buf(),
            vec![item(
                &project.join("debug"),
                14 * GIB_KB,
                StorageItemKind::Directory,
            )],
        )]);
        inventory.top_level = vec![item(project, 14 * GIB_KB, StorageItemKind::Directory)];
        inventory.volume = Some(VolumeStats {
            accounting_path: "/System/Volumes/Data".into(),
            filesystem: "apfs".into(),
            capacity_kb: 100 * GIB_KB,
            used_kb: 99 * GIB_KB,
            free_kb: GIB_KB,
            container_free_kb: None,
            device: 2,
        });
        inventory.roots = [
            ("/", 1),
            ("/System/Volumes/Data", 2),
            ("/Volumes/External", 3),
        ]
        .map(|(path, device)| crate::storage::StorageRoot {
            path: path.into(),
            size_kb: 0,
            device,
            scan_errors: 0,
        })
        .to_vec();
        let entries =
            ["/Users/demo/Library/Caches/pip", "/Volumes/External/cache"].map(|path| CacheEntry {
                spec: CacheSpec {
                    path: path.into(),
                    label: "fixture",
                    ..crate::cache::scan_specs(home, home)[0].clone()
                },
                size_kb: 125 * 1024,
                status: CacheStatus::Ready,
                identity: None,
                outcome: None,
            });
        let metrics = Metrics::default();
        let suggest = |target: &Target| care::suggestion_id(&entries, &[], target);
        let world = world(home, Some(&inventory), &entries, &[], &metrics, &suggest);
        for selected in [false, true] {
            let mut handles = Handles::default();
            if selected {
                handles.folder(&home.join("code/project/target"));
            }
            let result = cleanup_options_outcome(&world, &mut handles);
            assert_eq!(result.cleanup_total_kb, Some(125 * 1024));
            assert!(result.text.contains("target"), "{}", result.text);
            assert!(result.text.contains("14.0 GiB"));
            assert!(!result.text.contains("/System/Volumes/Data"));
            assert!(
                !handles
                    .actions
                    .iter()
                    .any(|action| action.contains("External"))
            );
        }
    }

    #[test]
    fn growth_questions_get_history_tools_within_the_six_tool_limit() {
        for (question, period) in [
            ("What grew since last week?", "week"),
            ("What changed since yesterday?", "day"),
            ("What shrank since last month?", "month"),
            ("What grew since last time?", "previous"),
            (
                "Which caches grew since last week and can I clean them?",
                "week",
            ),
        ] {
            let tools = Toolset::Ask.tools_for_question(false, Some(question));
            assert!(tools.iter().any(|tool| tool.name == "growth"));
            assert_eq!(tools.len(), 6);
            assert!(
                tools
                    .iter()
                    .map(|tool| spec_json(tool).to_string().len())
                    .sum::<usize>()
                    < 1600
            );
            assert_eq!(
                Toolset::Ask.fallback_for_question(false, Some(question)),
                vec![("growth", Some(period))]
            );
        }
        assert!(
            Toolset::Ask
                .tools_for_question(false, Some("What is using memory?"))
                .iter()
                .any(|tool| tool.name == "memory_state")
        );
    }

    #[test]
    fn growth_compares_the_requested_period_and_matching_volume() {
        let home = Path::new("/Users/demo");
        let metrics = Metrics::default();
        let none = |_: &Target| None;
        let mut world = world(home, None, &[], &[], &metrics, &none);
        let session = |id, days: u64, kb, volume: &str, complete| Session {
            id,
            updated: days * 86_400,
            complete,
            root: Some("/".into()),
            volume_id: Some(volume.into()),
            measurements: [("/Users/demo/Movies".into(), kb)].into_iter().collect(),
            ..Session::default()
        };
        let current = session(5, 30, 4 * GIB_KB, "disk1", true);
        let history = vec![
            session(4, 29, 3 * GIB_KB, "disk1", true),
            session(3, 23, GIB_KB, "other-disk", true),
            session(2, 23, GIB_KB, "disk1", false),
            session(1, 22, 2 * GIB_KB, "disk1", true),
        ];
        world.current = Some(&current);
        world.history = &history;
        let mut handles = Handles::default();
        let result = growth_comparison_outcome("week", &world, &mut handles);
        assert_eq!(result.status, EvidenceStatus::Complete);
        assert!(result.text.contains("+2.0 GiB"), "{}", result.text);
        assert!(result.text.contains("~/Movies"));
        assert_eq!(handles.folder_path("n1").unwrap(), home.join("Movies"));
        let previous = growth_comparison_outcome("previous", &world, &mut handles);
        assert!(previous.text.contains("+1.0 GiB"), "{}", previous.text);
        let missing = growth_comparison_outcome("month", &world, &mut handles);
        assert_eq!(missing.status, EvidenceStatus::Unsupported);
        assert!(missing.text.contains("at least 30 days old"));
        assert!(!missing.text.contains("No significant growth"));
        world.current = None;
        assert_eq!(
            growth_comparison_outcome("week", &world, &mut handles).status,
            EvidenceStatus::Unsupported
        );
    }

    #[test]
    fn handles_reject_paths_unknown_and_foreign_references() {
        let mut handles = Handles::default();
        assert_eq!(
            handles.folder(Path::new("/Users/demo/a")).as_deref(),
            Some("n1")
        );
        assert_eq!(
            handles.folder(Path::new("/Users/demo/a")).as_deref(),
            Some("n1")
        );
        assert_eq!(
            handles.folder(Path::new("/Users/demo/b")).as_deref(),
            Some("n2")
        );
        assert_eq!(handles.process(7, "start").as_deref(), Some("p1"));
        assert_eq!(handles.action("clean:/x").as_deref(), Some("A1"));
        assert_eq!(
            handles.folder_path("n2").unwrap(),
            Path::new("/Users/demo/b")
        );
        assert!(
            handles
                .folder_path("/Users/demo/a")
                .unwrap_err()
                .contains("Paths are not accepted")
        );
        assert!(
            handles
                .folder_path("~/Library")
                .unwrap_err()
                .contains("Paths are not accepted")
        );
        assert!(
            handles
                .folder_path("n9")
                .unwrap_err()
                .contains("Unknown handle")
        );
        assert!(handles.folder_path("p1").is_err());
        assert!(handles.folder_path("n0").is_err());
        assert!(handles.folder_path("n001").is_err());
        assert_eq!(handles.process_identity("p1").unwrap(), (7, "start"));
        assert_eq!(handles.action_id("A1"), Some("clean:/x"));
        assert_eq!(handles.action_id("A2"), None);
        let mut full = Handles::default();
        for index in 0..MAX_HANDLES {
            assert!(full.folder(&PathBuf::from(format!("/f{index}"))).is_some());
        }
        assert!(full.folder(Path::new("/overflow")).is_none());
    }

    #[test]
    fn calls_are_limited_to_the_toolset_and_declared_arguments() {
        let allowed = Toolset::Storage.tools(false);
        assert!(parse_call("list_children", r#"{"folder":"n1"}"#, &allowed).is_ok());
        assert!(
            parse_call("list_children", "{}", &allowed)
                .unwrap_err()
                .contains("folder")
        );
        assert!(
            parse_call("run_shell", r#"{"folder":"n1"}"#, &allowed)
                .unwrap_err()
                .contains("not available")
        );
        assert!(parse_call("memory_state", "{}", &allowed).is_err());
        let ask = Toolset::Ask.tools(false);
        assert!(parse_call("top_processes", r#"{"sort":"cpu"}"#, &ask).is_ok());
        assert!(parse_call("top_processes", r#"{"sort":"disk"}"#, &ask).is_err());
        let cleanup =
            Toolset::Ask.tools_for_question(false, Some("Is it safe to clear my pip CACHE?"));
        assert_eq!(cleanup.len(), 6);
        assert!(parse_call("open_handles", r#"{"folder":"n1"}"#, &cleanup).is_ok());
        assert!(parse_call("top_processes", r#"{"sort":"cpu"}"#, &cleanup).is_err());
        let cpu = Toolset::Ask.tools_for_question(false, Some("Which process uses CPU?"));
        assert!(parse_call("top_processes", r#"{"sort":"cpu"}"#, &cpu).is_ok());
    }

    #[test]
    fn list_children_issues_handles_only_for_directories_and_caps_output() {
        let home = Path::new("/Users/demo");
        let root = home.join("Library/Caches");
        let mut children: Vec<StorageItem> = (0..9)
            .map(|index| {
                item(
                    &root.join(format!("folder-{index}-{}", "x".repeat(60))),
                    100 - index,
                    StorageItemKind::Directory,
                )
            })
            .collect();
        children.push(item(&root.join("big.bin"), 500, StorageItemKind::File));
        children.sort_by_key(|item| std::cmp::Reverse(item.size_kb));
        let inventory = inventory(vec![(root.clone(), children)]);
        let metrics = Metrics::default();
        let none = |_: &Target| None;
        let world = world(home, Some(&inventory), &[], &[], &metrics, &none);
        let mut handles = Handles::default();
        handles.folder(&root);
        let call = parse_call(
            "list_children",
            r#"{"folder":"n1"}"#,
            &Toolset::Storage.tools(false),
        )
        .unwrap();
        let Dispatch::Ready(outcome) = dispatch(&call, &world, &mut handles) else {
            panic!("inventory lookups are immediate");
        };
        let text = cap(&outcome.text);
        assert!(text.len() <= OUTPUT_CAP);
        assert!(text.contains("big.bin (file)"));
        assert!(text.contains("n2 "));
        assert!(outcome.supports.contains(&"measured_children".to_string()));
        assert!(
            !text.contains("/Users/demo"),
            "paths are shown relative to home"
        );
        assert_eq!(
            label(&call, &handles, &world),
            "list_children(~/Library/Caches)"
        );
    }

    #[test]
    fn cleanup_rule_offers_actions_only_through_the_rust_eligibility_callback() {
        let home = Path::new("/Users/demo");
        let path = home.join("Library/Caches/pip");
        let entry = |status| CacheEntry {
            spec: CacheSpec {
                label: "pip cache",
                path: path.clone(),
                tier: CacheTier::Routine,
                note: "Packages download again when needed.",
                process_pattern: "",
                ..crate::cache::scan_specs(home, home)[0].clone()
            },
            status,
            size_kb: 2048,
            outcome: None,
            identity: PathIdentity::capture(home),
        };
        let metrics = Metrics::default();
        let eligible = |target: &Target| match target {
            Target::Cache(path) => Some(format!("clean:{}", path.display())),
            _ => None,
        };
        for (status, expect_action, support) in [
            (CacheStatus::Ready, true, Some("rebuildable_data")),
            (CacheStatus::Review, false, None),
            (CacheStatus::InUse, false, Some("active_writer")),
        ] {
            let entries = [entry(status)];
            let callback = |target: &Target| {
                (status == CacheStatus::Ready)
                    .then(|| eligible(target))
                    .flatten()
            };
            let world = world(home, None, &entries, &[], &metrics, &callback);
            let mut handles = Handles::default();
            handles.folder(&path);
            let outcome = cleanup_rule_outcome(&path, &world, &mut handles);
            assert_eq!(
                outcome.text.contains("eligible action A1"),
                expect_action,
                "{status:?}"
            );
            assert_eq!(handles.action_id("A1").is_some(), expect_action);
            if let Some(id) = support {
                assert!(outcome.supports.contains(&id.to_string()), "{status:?}");
            }
            if status == CacheStatus::Review {
                assert!(
                    outcome
                        .contradicts
                        .contains(&"rebuildable_data".to_string())
                );
            }
        }
        let world = world(home, None, &[], &[], &metrics, &eligible);
        let outcome =
            cleanup_rule_outcome(&home.join("Documents"), &world, &mut Handles::default());
        assert!(
            outcome
                .contradicts
                .contains(&"rebuildable_data".to_string())
        );
    }

    #[test]
    fn failed_and_timed_out_collectors_never_support_hypotheses() {
        let home = Path::new("/Users/demo");
        let metrics = Metrics::default();
        let none = |_: &Target| None;
        let world = world(home, None, &[], &[], &metrics, &none);
        let mut handles = Handles::default();
        for outcome in [
            owners_outcome(
                home,
                Err(io::Error::new(io::ErrorKind::TimedOut, "slow")),
                &world,
                &mut handles,
            ),
            owners_outcome(home, Err(io::Error::other("denied")), &world, &mut handles),
            age_outcome(home, Err(io::Error::other("gone")), &world),
            samples_outcome("p1", "x", Err("ps failed".into())),
            growth_outcome(home, &world),
        ] {
            assert!(!outcome.status.can_support_hypothesis(), "{}", outcome.text);
            assert!(outcome.supports.is_empty());
        }
        let busy = owners_outcome(home, Ok(vec![(4, "Code".into())]), &world, &mut handles);
        assert!(busy.supports.contains(&"active_writer".to_string()));
        let idle = owners_outcome(home, Ok(vec![]), &world, &mut handles);
        assert!(idle.contradicts.contains(&"active_writer".to_string()));
    }

    #[test]
    fn folder_age_classification_is_conservative() {
        let home = Path::new("/Users/demo");
        let metrics = Metrics::default();
        let none = |_: &Target| None;
        let world = world(home, None, &[], &[], &metrics, &none);
        let profile = |buckets, newest| AgeProfile {
            files: 4,
            total_kb: 400,
            buckets_kb: buckets,
            newest_age_secs: Some(newest),
            complete: true,
            errors: 0,
        };
        let busy = age_outcome(home, Ok(profile([100, 100, 100, 100], 60)), &world);
        assert!(busy.supports.contains(&"active_writer".to_string()));
        let stale = age_outcome(home, Ok(profile([0, 100, 100, 200], 40 * 86_400)), &world);
        assert!(stale.contradicts.contains(&"active_writer".to_string()));
        let mixed = age_outcome(home, Ok(profile([0, 300, 50, 50], 3 * 86_400)), &world);
        assert!(mixed.supports.is_empty() && mixed.contradicts.is_empty());
    }

    #[test]
    fn mounts_are_compressed_to_external_and_custom_volumes() {
        assert_eq!(
            compress_mounts(
                "/dev/disk3s1 on / (apfs, local, read-only)\ndevfs on /dev (devfs, local)\n"
            ),
            "Only internal APFS system volumes are mounted."
        );
        let text = compress_mounts(
            "/dev/disk3s1 on / (apfs, local)\n//nas/share on /Volumes/share (smbfs, nodev)\n/dev/disk5s1 on /Volumes/Backup (apfs, local)\n",
        );
        assert!(text.contains("/Volumes/share (smbfs)"));
        assert!(text.contains("/Volumes/Backup (apfs)"));
    }

    #[test]
    fn output_cap_respects_character_boundaries() {
        let long = "é".repeat(OUTPUT_CAP);
        let capped = cap(&long);
        assert!(capped.len() <= OUTPUT_CAP);
        assert!(capped.ends_with('…'));
        assert_eq!(cap("a\u{1b}b"), "ab");
    }
}
