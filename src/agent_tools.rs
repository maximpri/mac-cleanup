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
            let action = (world.suggest)(&finding.target)
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
