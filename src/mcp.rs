// SPDX-License-Identifier: GPL-3.0-or-later
//! `diskray mcp`: a Model Context Protocol server over stdio.
//!
//! It gives coding agents (Claude Code, Cursor, Codex, …) the same read-only
//! tools the on-device model uses, plus `propose_cleanup`, which only saves a
//! proposal for the user to review in `diskray review`. Nothing in this module
//! can delete, move, or signal anything.
//!
//! The server is dual-era: it answers the legacy `initialize` handshake
//! (2025-11-25 and earlier) and modern per-request `_meta` versioning
//! (2026-07-28) with `server/discover`. Stdout carries protocol messages only.

use crate::{
    agent_tools::{self, Call, Dispatch, Handles, ToolWorld},
    cache::{self, format_kb},
    care::{self, Target},
    growth,
    headless::{self, Options, Snapshot},
    pending::{self, PendingPlan, ProposedAction},
};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::AtomicBool,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

pub const MODERN_VERSION: &str = "2026-07-28";
pub const LEGACY_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
const MAX_LINE_BYTES: usize = 1024 * 1024;
const MAX_TEXT_BYTES: usize = 8 * 1024;
const SNAPSHOT_WAIT: Duration = Duration::from_secs(40);
const SNAPSHOT_FRESH: Duration = Duration::from_secs(300);
const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT: &str = "io.modelcontextprotocol/clientInfo";
const META_SERVER: &str = "io.modelcontextprotocol/serverInfo";

const INSTRUCTIONS: &str = "Diskray explains what fills this Mac's disk using measured, read-only tools. \
Start with disk_overview, then drill into folders with list_children, folder_age, open_handles, identify_owner, and cleanup_rules. \
Folders can be named by absolute path, ~/path, or a handle such as n2 from an earlier result. \
Large size alone never proves that data is removable. Only cleanup_rules marks data as eligible, \
and propose_cleanup only saves a proposal: the user must review and confirm it in `diskray review`. Nothing here deletes files.";

/// Private locations: only an aggregate size is ever reported.
const PRIVATE: [&str; 8] = [
    "Library/Keychains",
    "Library/Messages",
    "Library/Mail",
    "Library/Cookies",
    "Library/Application Support/com.apple.TCC",
    "Library/Safari",
    ".ssh",
    ".gnupg",
];

enum SnapshotState {
    Idle,
    Collecting(Receiver<Snapshot>, Instant),
    Ready(Box<Snapshot>, Instant),
}

pub struct Server {
    home: PathBuf,
    root: PathBuf,
    /// Paths outside these roots are refused.
    roots: Vec<PathBuf>,
    snapshot: SnapshotState,
    handles: Handles,
    client: String,
}

impl Server {
    pub fn new(home: &Path, root: &Path) -> Self {
        let mut roots = vec![home.canonicalize().unwrap_or_else(|_| home.to_path_buf())];
        if let Ok(root) = root.canonicalize()
            && !roots.iter().any(|known| root.starts_with(known))
        {
            roots.push(root);
        }
        Self {
            home: home.to_path_buf(),
            root: root.to_path_buf(),
            roots,
            snapshot: SnapshotState::Idle,
            handles: Handles::default(),
            client: "unknown MCP client".into(),
        }
    }

    #[cfg(test)]
    fn with_snapshot(home: &Path, snapshot: Snapshot) -> Self {
        let mut server = Self::new(home, &snapshot.root.clone());
        server.snapshot = SnapshotState::Ready(Box::new(snapshot), Instant::now());
        server
    }
}

/// Serve newline-delimited JSON-RPC until input closes.
pub fn serve(input: impl BufRead, mut output: impl Write, server: &mut Server) -> io::Result<()> {
    let mut input = input;
    loop {
        let mut line = Vec::new();
        let read = (&mut input)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(());
        }
        let response = if line.len() > MAX_LINE_BYTES && !line.ends_with(b"\n") {
            // Discard the rest of the oversized line.
            let mut rest = Vec::new();
            input.read_until(b'\n', &mut rest)?;
            Some(error_response(
                &Value::Null,
                -32600,
                "Request is larger than 1 MiB",
            ))
        } else {
            let text = String::from_utf8_lossy(&line);
            if text.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(text.trim()) {
                Ok(message) => server.handle(message),
                Err(_) => Some(error_response(&Value::Null, -32700, "Parse error")),
            }
        };
        if let Some(response) = response {
            let mut bytes = serde_json::to_vec(&response)?;
            bytes.push(b'\n');
            output.write_all(&bytes)?;
            output.flush()?;
        }
    }
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn server_info() -> Value {
    json!({"name": "diskray", "title": "Diskray", "version": env!("CARGO_PKG_VERSION")})
}

impl Server {
    /// Handle one message. Notifications and client responses produce nothing.
    fn handle(&mut self, message: Value) -> Option<Value> {
        let Some(object) = message.as_object() else {
            return Some(error_response(&Value::Null, -32600, "Invalid request"));
        };
        let id = object.get("id").cloned();
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            // A response to something we never sent, or malformed.
            return id.map(|id| error_response(&id, -32600, "Invalid request"));
        };
        let params = object.get("params").cloned().unwrap_or(json!({}));
        let meta = params.get("_meta");
        let modern_version = meta
            .and_then(|meta| meta.get(META_VERSION))
            .and_then(Value::as_str);
        if let Some(client) = meta
            .and_then(|meta| meta.get(META_CLIENT))
            .and_then(|info| info.get("name"))
            .and_then(Value::as_str)
        {
            self.client = crate::ai::display_text(client).chars().take(80).collect();
        }
        let Some(id) = id else {
            // Notifications: initialized, cancelled, and anything else.
            return None;
        };
        if let Some(version) = modern_version
            && version != MODERN_VERSION
        {
            let mut supported = vec![MODERN_VERSION];
            supported.extend(LEGACY_VERSIONS);
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32022,
                    "message": "Unsupported protocol version",
                    "data": {"supported": supported, "requested": version},
                },
            }));
        }
        let modern = modern_version.is_some();
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "server/discover" => Ok(json!({
                "supportedVersions": [MODERN_VERSION, LEGACY_VERSIONS[0], LEGACY_VERSIONS[1], LEGACY_VERSIONS[2]],
                "capabilities": {"tools": {"listChanged": false}},
                "instructions": INSTRUCTIONS,
                "ttlMs": 3_600_000,
                "cacheScope": "private",
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({
                "tools": tool_definitions(),
                "ttlMs": 3_600_000,
                "cacheScope": "private",
            })),
            "tools/call" => self.call_tool(&params),
            _ => Err((-32601, format!("Method not found: {method}"))),
        };
        Some(match result {
            Ok(mut result) => {
                if modern {
                    result["resultType"] = json!("complete");
                    result["_meta"] = json!({ META_SERVER: server_info() });
                }
                json!({"jsonrpc": "2.0", "id": id, "result": result})
            }
            Err((code, message)) => error_response(&id, code, &message),
        })
    }

    fn initialize(&mut self, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(LEGACY_VERSIONS[0]);
        if let Some(client) = params
            .get("clientInfo")
            .and_then(|info| info.get("name"))
            .and_then(Value::as_str)
        {
            self.client = crate::ai::display_text(client).chars().take(80).collect();
        }
        let version = if LEGACY_VERSIONS.contains(&requested) {
            requested
        } else {
            LEGACY_VERSIONS[0]
        };
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": server_info(),
            "instructions": INSTRUCTIONS,
        })
    }

    fn call_tool(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or((-32602, "tools/call needs a tool name".to_string()))?;
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
        if !tool_definitions()
            .iter()
            .any(|tool| tool["name"].as_str() == Some(name))
        {
            return Err((-32602, format!("Unknown tool: {name}")));
        }
        // The project search does not need the disk assessment.
        let outcome = if name == "stale_artifacts" {
            Ok(self.stale_artifacts(&arguments))
        } else {
            match self.snapshot_ready() {
                Err(waiting) => Ok(ToolText::plain(waiting)),
                Ok(()) => self.run_tool(name, &arguments),
            }
        };
        Ok(match outcome {
            Ok(text) => {
                let mut result = json!({
                    "content": [{"type": "text", "text": cap(&text.text)}],
                    "isError": false,
                });
                if let Some(structured) = text.structured {
                    result["structuredContent"] = structured;
                }
                result
            }
            Err(message) => json!({
                "content": [{"type": "text", "text": cap(&message)}],
                "isError": true,
            }),
        })
    }

    /// Start or wait for the background assessment. Returns a friendly
    /// message when it is still running.
    fn snapshot_ready(&mut self) -> Result<(), String> {
        if let SnapshotState::Ready(_, at) = &self.snapshot
            && at.elapsed() < SNAPSHOT_FRESH
        {
            return Ok(());
        }
        if matches!(self.snapshot, SnapshotState::Idle) {
            self.start_collection();
        }
        if let SnapshotState::Ready(..) = self.snapshot {
            // Stale but usable; refresh in the background next time.
            return Ok(());
        }
        let started = Instant::now();
        loop {
            let SnapshotState::Collecting(receiver, since) = &self.snapshot else {
                return Ok(());
            };
            match receiver.try_recv() {
                Ok(snapshot) => {
                    self.snapshot = SnapshotState::Ready(Box::new(snapshot), Instant::now());
                    return Ok(());
                }
                Err(TryRecvError::Disconnected) => {
                    self.snapshot = SnapshotState::Idle;
                    return Err("The disk assessment stopped unexpectedly; try again.".into());
                }
                Err(TryRecvError::Empty) if started.elapsed() > SNAPSHOT_WAIT => {
                    return Err(format!(
                        "Diskray is still measuring the disk (started {}s ago). Nothing is wrong; call the tool again in about a minute.",
                        since.elapsed().as_secs()
                    ));
                }
                Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(100)),
            }
        }
    }

    fn start_collection(&mut self) {
        let (sender, receiver) = mpsc::channel();
        let home = self.home.clone();
        let mut options = Options::new(self.root.clone());
        options.deadline = Duration::from_secs(300);
        thread::spawn(move || {
            let _ = sender.send(headless::collect(&home, &options, |_| {}));
        });
        self.snapshot = SnapshotState::Collecting(receiver, Instant::now());
    }

    fn snapshot(&self) -> &Snapshot {
        match &self.snapshot {
            SnapshotState::Ready(snapshot, _) => snapshot,
            _ => unreachable!("tools run only after snapshot_ready"),
        }
    }

    /// Resolve an agent-supplied folder: a handle from an earlier result, an
    /// absolute path, or `~/path`. It must stay inside the allowed roots and
    /// never pass through a symlink.
    fn resolve_folder(&mut self, argument: &str) -> Result<PathBuf, String> {
        let argument = argument.trim();
        if argument.len() > 4096 || argument.contains('\0') {
            return Err("Path is too long or malformed.".into());
        }
        if let Ok(path) = self.handles.folder_path(argument) {
            return Ok(path.to_path_buf());
        }
        let raw = if argument == "~" {
            self.home.clone()
        } else if let Some(rest) = argument.strip_prefix("~/") {
            self.home.join(rest)
        } else if Path::new(argument).is_absolute() {
            PathBuf::from(argument)
        } else {
            return Err(format!(
                "Use an absolute path, ~/path, or a handle like n2 (got {}).",
                crate::ai::display_text(argument)
                    .chars()
                    .take(60)
                    .collect::<String>()
            ));
        };
        if raw
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err("Paths with `..` are not accepted.".into());
        }
        let resolved = raw
            .canonicalize()
            .map_err(|_| format!("{} does not exist or cannot be read.", raw.display()))?;
        let Some(root) = self.roots.iter().find(|root| resolved.starts_with(root)) else {
            return Err("That path is outside the folders Diskray was started for.".into());
        };
        if cache::has_symlink_component_below(&raw, root)
            && cache::has_symlink_component_below(&resolved, root)
        {
            return Err("Paths through symlinks are not accepted.".into());
        }
        Ok(resolved)
    }

    fn is_private(&self, path: &Path) -> bool {
        let home = self
            .home
            .canonicalize()
            .unwrap_or_else(|_| self.home.clone());
        PRIVATE
            .iter()
            .any(|private| path.starts_with(home.join(private)))
    }

    fn run_tool(&mut self, name: &str, arguments: &Value) -> Result<ToolText, String> {
        let text = |key: &str| {
            arguments
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
        };
        match name {
            "disk_overview" => {
                if arguments.get("refresh").and_then(Value::as_bool) == Some(true) {
                    self.start_collection();
                    return Ok(ToolText::plain(
                        "Started a fresh assessment. Call disk_overview again in about a minute."
                            .into(),
                    ));
                }
                Ok(self.overview())
            }
            "findings" => Ok(self.findings()),
            "growth" => Ok(self.growth(arguments.get("since_days").and_then(Value::as_u64))),
            "list_children" | "folder_age" | "open_handles" | "identify_owner"
            | "cleanup_rules" => {
                let path = self.resolve_folder(text("path"))?;
                if self.is_private(&path) {
                    let size = self
                        .snapshot()
                        .inventory
                        .as_ref()
                        .and_then(|inventory| {
                            inventory
                                .children
                                .values()
                                .flatten()
                                .find(|item| item.path == path)
                        })
                        .map(|item| format_kb(item.size_kb))
                        .unwrap_or_else(|| "unmeasured".into());
                    return Ok(ToolText::plain(format!(
                        "{} is a private location; Diskray reports only its total size: {size}.",
                        agent_tools::short_path(&path, &self.home)
                    )));
                }
                let tool = if name == "cleanup_rules" {
                    "cleanup_rule"
                } else {
                    name
                };
                let handle = self
                    .handles
                    .folder(&path)
                    .ok_or("Too many folders in this session; restart the server.")?;
                self.dispatch(tool, Some(handle))
            }
            "top_processes" => {
                let sort = if text("sort") == "cpu" {
                    "cpu"
                } else {
                    "memory"
                };
                self.dispatch("top_processes", Some(sort.into()))
            }
            "memory_state" => self.dispatch("memory_state", None),
            "process_details" => {
                let pid = arguments
                    .get("pid")
                    .and_then(Value::as_u64)
                    .ok_or("process_details needs a numeric pid.")?
                    as u32;
                let start = self
                    .snapshot()
                    .processes
                    .iter()
                    .find(|process| process.pid == pid)
                    .map(|process| process.start_time.clone())
                    .ok_or(format!(
                        "No current process with PID {pid} is in the latest sample."
                    ))?;
                let handle = self
                    .handles
                    .process(pid, &start)
                    .ok_or("Too many processes in this session.")?;
                self.dispatch("process_details", Some(handle))
            }
            "propose_cleanup" => self.propose(arguments),
            _ => Err(format!("Unknown tool: {name}")),
        }
    }

    /// Run one shared read-only tool exactly as the on-device agent does.
    fn dispatch(&mut self, tool: &str, argument: Option<String>) -> Result<ToolText, String> {
        let spec = agent_tools::spec(tool).ok_or("Unsupported tool.")?;
        let snapshot = match &self.snapshot {
            SnapshotState::Ready(snapshot, _) => snapshot,
            _ => return Err("The disk assessment is not ready yet.".into()),
        };
        let suggest = |target: &Target| snapshot.suggestion(target);
        let world: ToolWorld<'_> = snapshot.world(&suggest, false);
        let call = Call {
            tool: spec,
            argument,
        };
        let outcome = match agent_tools::dispatch(&call, &world, &mut self.handles) {
            Dispatch::Ready(outcome) => outcome,
            Dispatch::Slow(job) => {
                let slow = job(&AtomicBool::new(false));
                agent_tools::finish(slow, &world, &mut self.handles)
            }
            Dispatch::NeedsApproval(_) => {
                return Err("That diagnostic needs approval in the interactive app.".into());
            }
            Dispatch::Rejected(reason) => return Err(reason),
        };
        Ok(ToolText::plain(format!(
            "{} ({})",
            outcome.text,
            outcome.status.label()
        )))
    }

    fn overview(&self) -> ToolText {
        let snapshot = self.snapshot();
        let volume = snapshot.volume.as_ref();
        let inventory = snapshot.inventory.as_ref();
        let quick: Vec<_> = snapshot.findings.iter().filter(|f| f.quick_win).collect();
        let structured = json!({
            "root": snapshot.root,
            "complete": snapshot.complete,
            "capacity_kb": volume.map(|v| v.capacity_kb),
            "used_kb": volume.map(|v| v.disk_used_kb()),
            "free_kb": volume.map(|v| v.disk_free_kb()),
            "measured_kb": inventory.map(|i| i.scanned_on_volume_kb),
            "not_visible_to_scan_kb": inventory.map(|i| i.unaccounted_kb),
            "other_volumes_kb": volume.map(|v| v.other_volume_kb()),
            "local_snapshots": inventory.map(|i| i.local_snapshots.len()),
            "largest": inventory.map(|inventory| inventory.top_level.iter().take(8).map(|item| json!({
                "path": item.path, "size_kb": item.size_kb
            })).collect::<Vec<_>>()),
            "quick_wins": quick.iter().map(|finding| json!({
                "title": finding.title, "size_kb": finding.size_kb,
                "path": match &finding.target { Target::Cache(path) => Some(path), _ => None },
            })).collect::<Vec<_>>(),
        });
        let text = format!(
            "{} used of {} · {} free. Folder walk measured {}{}; {} not visible to the walk (snapshots, system data, or protected folders); {} local snapshot(s). Largest: {}. Ready quick wins: {} ({}).",
            volume.map_or("?".into(), |v| format_kb(v.disk_used_kb())),
            volume.map_or("?".into(), |v| format_kb(v.capacity_kb)),
            volume.map_or("?".into(), |v| format_kb(v.disk_free_kb())),
            inventory.map_or("nothing".into(), |i| format_kb(i.scanned_on_volume_kb)),
            if snapshot.complete { "" } else { " (partial)" },
            inventory.map_or("unknown".into(), |i| format_kb(i.unaccounted_kb)),
            inventory.map_or(0, |i| i.local_snapshots.len()),
            inventory
                .map(|inventory| inventory
                    .top_level
                    .iter()
                    .take(5)
                    .map(|item| format!(
                        "{} {}",
                        agent_tools::short_path(&item.path, &self.home),
                        format_kb(item.size_kb)
                    ))
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default(),
            format_kb(quick.iter().map(|finding| finding.size_kb).sum()),
            quick
                .iter()
                .map(|finding| crate::ai::display_text(&finding.title))
                .collect::<Vec<_>>()
                .join(", "),
        );
        ToolText {
            text,
            structured: Some(structured),
        }
    }

    fn findings(&self) -> ToolText {
        let snapshot = self.snapshot();
        let items: Vec<Value> = snapshot
            .findings
            .iter()
            .take(30)
            .map(|finding| {
                let (kind, path) = match &finding.target {
                    Target::Cache(path) => ("cleanup_rule", Some(path.display().to_string())),
                    Target::Folder(path) => ("folder", Some(path.display().to_string())),
                    Target::Process(..) => ("process", None),
                    Target::System => ("system", None),
                };
                json!({
                    "title": crate::ai::display_text(&finding.title),
                    "kind": kind,
                    "path": path,
                    "size_kb": finding.size_kb,
                    "observation": crate::ai::display_text(&finding.observation),
                    "tradeoff": crate::ai::display_text(&finding.consequence),
                    "eligible_for_proposal": snapshot.suggestion(&finding.target).is_some()
                        && matches!(finding.target, Target::Cache(_)),
                })
            })
            .collect();
        let text = items
            .iter()
            .map(|item| {
                format!(
                    "- {} · {} · {}{}",
                    item["title"].as_str().unwrap_or_default(),
                    item["kind"].as_str().unwrap_or_default(),
                    format_kb(item["size_kb"].as_u64().unwrap_or_default()),
                    if item["eligible_for_proposal"] == true {
                        " · eligible for propose_cleanup"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        ToolText {
            text: if text.is_empty() {
                "No findings in this assessment.".into()
            } else {
                text
            },
            structured: Some(json!({ "findings": items })),
        }
    }

    fn growth(&self, since_days: Option<u64>) -> ToolText {
        let snapshot = self.snapshot();
        let mut current = snapshot.session("mcp");
        current.root = Some(snapshot.root.display().to_string());
        current.volume_id = care::volume_id(&snapshot.root);
        current.complete = snapshot.complete;
        let base = match since_days {
            Some(days) => growth::baseline(&snapshot.history, &current, days),
            None => growth::previous(&snapshot.history, &current),
        };
        let Some(base) = base else {
            return ToolText::plain(
                "No earlier complete assessment of this volume to compare with yet. Run `diskray why` periodically to build history.".into(),
            );
        };
        let deltas = growth::diff(base, &current);
        let structured = json!({
            "since": base.updated,
            "changes": deltas.iter().map(|delta| json!({
                "path": delta.path, "before_kb": delta.before_kb, "after_kb": delta.after_kb,
            })).collect::<Vec<_>>(),
        });
        let text = if deltas.is_empty() {
            "Nothing changed significantly (at least 500 MB and 5%) since the baseline.".into()
        } else {
            deltas
                .iter()
                .take(10)
                .map(|delta| {
                    format!(
                        "{} {}{} → {}",
                        agent_tools::short_path(Path::new(&delta.path), &self.home),
                        if delta.change_kb() >= 0 { "+" } else { "−" },
                        format_kb(delta.change_kb().unsigned_abs()),
                        format_kb(delta.after_kb)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        ToolText {
            text,
            structured: Some(structured),
        }
    }

    fn stale_artifacts(&self, arguments: &Value) -> ToolText {
        let mut config = crate::artifacts::Config::load(&self.home);
        if let Some(days) = arguments.get("min_age_days").and_then(Value::as_u64) {
            config.min_age_days = days.min(3650);
        }
        let report = crate::artifacts::find(&self.home, &config, &AtomicBool::new(false));
        let mut lines = if report.artifacts.is_empty() {
            vec![format!(
                "No build output over 10 MB in projects untouched for {}+ days.",
                config.min_age_days
            )]
        } else {
            let mut lines = vec![format!(
                "{} of build output in projects untouched for {}+ days (report only; cannot be proposed for cleanup):",
                format_kb(report.total_kb()),
                config.min_age_days
            )];
            lines.extend(report.lines(|path| agent_tools::short_path(path, &self.home), 25));
            lines
        };
        if report.roots.is_empty() {
            lines.push("No project folders found; the user can set `roots` in ~/.config/diskray/projects.toml.".into());
        }
        if !report.complete {
            lines.push("Partial: the search stopped at its time or size limit.".into());
        }
        ToolText {
            text: lines.join("\n"),
            structured: Some(report.to_json()),
        }
    }

    /// Save a proposal of eligible cleanup targets for the user to review.
    fn propose(&mut self, arguments: &Value) -> Result<ToolText, String> {
        let paths: Vec<&str> = arguments
            .get("paths")
            .and_then(Value::as_array)
            .map(|paths| paths.iter().filter_map(Value::as_str).take(10).collect())
            .unwrap_or_default();
        if paths.is_empty() {
            return Err(
                "propose_cleanup needs `paths`: cleanup-rule paths from findings or cleanup_rules."
                    .into(),
            );
        }
        let reason: String = crate::ai::display_text(
            arguments
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or(""),
        )
        .chars()
        .take(300)
        .collect();
        let mut actions = Vec::new();
        let mut refused = Vec::new();
        for raw in paths {
            let resolved = self.resolve_folder(raw);
            let snapshot = self.snapshot();
            let entry = resolved.as_ref().ok().and_then(|path| {
                snapshot
                    .entries
                    .iter()
                    .find(|entry| entry.spec.path.canonicalize().ok().as_ref() == Some(path))
            });
            match entry.and_then(|entry| {
                snapshot
                    .suggestion(&Target::Cache(entry.spec.path.clone()))
                    .map(|id| (entry, id))
            }) {
                Some((entry, id)) => actions.push(ProposedAction {
                    id,
                    path: entry.spec.path.display().to_string(),
                    label: entry.spec.label.into(),
                    size_kb: entry.size_kb,
                }),
                None => refused.push(crate::ai::display_text(raw)),
            }
        }
        if actions.is_empty() {
            return Err(format!(
                "None of these are eligible: {}. Only cleanup-rule targets that are ready now can be proposed.",
                refused.join(", ")
            ));
        }
        let total: u64 = actions.iter().map(|action| action.size_kb).sum();
        let plan = PendingPlan {
            schema: "diskray.pending/1".into(),
            created: care::timestamp(),
            client: self.client.clone(),
            reason,
            actions,
        };
        pending::save(&self.home, &plan)
            .map_err(|error| format!("Could not save the proposal: {error}"))?;
        Ok(ToolText::plain(format!(
            "Saved a proposal with {} action(s) totalling {}. Nothing was deleted. Ask the user to run `diskray review` to inspect and confirm it.{}",
            plan.actions.len(),
            format_kb(total),
            if refused.is_empty() {
                String::new()
            } else {
                format!(" Not eligible and left out: {}.", refused.join(", "))
            }
        )))
    }
}

struct ToolText {
    text: String,
    structured: Option<Value>,
}

impl ToolText {
    fn plain(text: String) -> Self {
        Self {
            text,
            structured: None,
        }
    }
}

fn cap(text: &str) -> String {
    let text = crate::ai::display_text(text);
    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    let mut end = MAX_TEXT_BYTES - 3;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn read_only(title: &str) -> Value {
    json!({"title": title, "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false})
}

fn path_input(description: &str) -> Value {
    json!({
        "type": "object",
        "properties": {"path": {"type": "string", "description": description}},
        "required": ["path"],
        "additionalProperties": false,
    })
}

/// Tool definitions in a fixed order, so clients can cache them.
pub fn tool_definitions() -> Vec<Value> {
    let folder = "An absolute path, ~/path, or a folder handle such as n2 from an earlier result.";
    let empty = json!({"type": "object", "properties": {}, "additionalProperties": false});
    vec![
        json!({
            "name": "disk_overview",
            "description": "Capacity, used and free space, space the folder walk cannot see (snapshots, system data), the largest folders, and ready quick wins. Start here.",
            "inputSchema": {"type": "object", "properties": {"refresh": {"type": "boolean", "description": "Start a fresh assessment instead of using the cached one."}}, "additionalProperties": false},
            "annotations": read_only("Disk overview"),
        }),
        json!({
            "name": "findings",
            "description": "Measured findings: cleanup-rule targets with their status and tradeoff, large folders, busy processes, and system readings.",
            "inputSchema": empty,
            "annotations": read_only("Findings"),
        }),
        json!({
            "name": "list_children",
            "description": "The largest items inside a folder, with handles for subfolders. Size alone does not mean waste.",
            "inputSchema": path_input(folder),
            "annotations": read_only("List folder contents"),
        }),
        json!({
            "name": "folder_age",
            "description": "How much of a folder's space changed recently versus long ago, and when it last changed.",
            "inputSchema": path_input(folder),
            "annotations": read_only("Folder age"),
        }),
        json!({
            "name": "open_handles",
            "description": "Which running processes hold files open inside a folder right now.",
            "inputSchema": path_input(folder),
            "annotations": read_only("Open files"),
        }),
        json!({
            "name": "identify_owner",
            "description": "Which app or tool most likely owns a folder, and when that app was last opened. A heuristic, not proof.",
            "inputSchema": path_input(folder),
            "annotations": read_only("Likely owner"),
        }),
        json!({
            "name": "cleanup_rules",
            "description": "Whether a Diskray cleanup rule covers a folder, its current status, and its tradeoff. Only rule targets can be proposed for cleanup.",
            "inputSchema": path_input(folder),
            "annotations": read_only("Cleanup rules"),
        }),
        json!({
            "name": "growth",
            "description": "What grew or shrank significantly since an earlier complete assessment of this volume.",
            "inputSchema": {"type": "object", "properties": {"since_days": {"type": "integer", "minimum": 0, "maximum": 365, "description": "Compare with the newest assessment at least this many days old; omit for the previous one."}}, "additionalProperties": false},
            "annotations": read_only("Growth"),
        }),
        json!({
            "name": "top_processes",
            "description": "Processes using the most CPU or memory right now. High usage alone does not mean a process is stuck.",
            "inputSchema": {"type": "object", "properties": {"sort": {"type": "string", "enum": ["cpu", "memory"]}}, "required": ["sort"], "additionalProperties": false},
            "annotations": read_only("Top processes"),
        }),
        json!({
            "name": "memory_state",
            "description": "Memory pressure, swap activity, and the largest memory users.",
            "inputSchema": empty,
            "annotations": read_only("Memory state"),
        }),
        json!({
            "name": "process_details",
            "description": "Identity, state, memory, CPU, and parent of one process by PID. Shows the executable, never its arguments.",
            "inputSchema": {"type": "object", "properties": {"pid": {"type": "integer", "minimum": 1}}, "required": ["pid"], "additionalProperties": false},
            "annotations": read_only("Process details"),
        }),
        json!({
            "name": "stale_artifacts",
            "description": "Build output (node_modules, target, .venv, .build, .gradle, .next) in projects nobody has touched for a while, with how to regenerate each. Report only; these cannot be proposed for cleanup.",
            "inputSchema": {"type": "object", "properties": {"min_age_days": {"type": "integer", "minimum": 1, "maximum": 3650, "description": "Report projects untouched for at least this many days (default 60)."}}, "additionalProperties": false},
            "annotations": read_only("Stale build output"),
        }),
        json!({
            "name": "propose_cleanup",
            "description": "Save a proposal to clear up to 10 cleanup-rule targets that are ready now. Nothing is deleted: the user reviews and confirms it with `diskray review`.",
            "inputSchema": {"type": "object", "properties": {
                "paths": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 10, "description": "Cleanup-rule target paths from findings or cleanup_rules."},
                "reason": {"type": "string", "maxLength": 300, "description": "Why these targets, in one or two sentences, shown to the user."}
            }, "required": ["paths"], "additionalProperties": false},
            "annotations": {"title": "Propose cleanup for review", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false},
        }),
    ]
}

/// Run the server on stdin/stdout.
pub fn run(home: &Path, root: &Path) -> Result<i32, String> {
    let mut server = Server::new(home, root);
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve(stdin.lock(), stdout.lock(), &mut server).map_err(|error| error.to_string())?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cache::{CacheEntry, CacheSpec, CacheStatus, CacheTarget, CacheTier, PathIdentity},
        care::Metrics,
        storage::{StorageCategory, StorageInventory, StorageItem, StorageItemKind},
    };
    use std::fs;

    fn fixture() -> (tempfile::TempDir, Server) {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().canonicalize().unwrap();
        let cache = root.join("Library/Caches/pip");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("wheel"), vec![1; 4096]).unwrap();
        fs::create_dir_all(root.join(".ssh")).unwrap();
        fs::create_dir_all(root.join("Library/Caches/Review")).unwrap();
        let entry = |path: &Path, status| CacheEntry {
            spec: CacheSpec {
                home: root.clone(),
                path: path.to_path_buf(),
                label: "pip cache",
                tier: CacheTier::Routine,
                process_pattern: "",
                note: "downloaded Python packages",
                target: CacheTarget::DirectoryContents,
            },
            status,
            size_kb: 4,
            outcome: None,
            identity: PathIdentity::capture(path),
        };
        let children = vec![StorageItem {
            path: cache.clone(),
            size_kb: 4,
            kind: StorageItemKind::Directory,
            category: StorageCategory::DeveloperData,
        }];
        let inventory = StorageInventory {
            volume: None,
            volume_error: None,
            roots: vec![],
            scanned_kb: 4,
            scanned_on_volume_kb: 4,
            unaccounted_kb: 0,
            inventory_overage_kb: 0,
            local_snapshots: vec![],
            scanned_items: 1,
            scan_errors: 0,
            scan_error_paths: vec![],
            complete: true,
            top_level: children.clone(),
            largest: vec![],
            children: [(root.join("Library/Caches"), children)]
                .into_iter()
                .collect(),
        };
        let entries = vec![
            entry(&cache, CacheStatus::Ready),
            entry(&root.join("Library/Caches/Review"), CacheStatus::Review),
        ];
        let findings = care::findings(&entries, &[], &Metrics::default(), Some(&inventory));
        let snapshot = Snapshot {
            root: root.clone(),
            home: root.clone(),
            volume: None,
            entries,
            processes: vec![],
            metrics: Metrics::default(),
            inventory: Some(inventory),
            findings,
            history: vec![],
            complete: true,
            elapsed: Duration::ZERO,
        };
        let server = Server::with_snapshot(&root, snapshot);
        (home, server)
    }

    fn exchange(server: &mut Server, lines: &[Value]) -> Vec<Value> {
        let input: String = lines.iter().map(|line| format!("{line}\n")).collect();
        let mut output = Vec::new();
        serve(input.as_bytes(), &mut output, server).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn call(id: u64, name: &str, arguments: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
    }

    #[test]
    fn speaks_both_legacy_and_modern_protocols() {
        let (_home, mut server) = fixture();
        let replies = exchange(
            &mut server,
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "clientInfo": {"name": "Claude Code"}}}),
                json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
                json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
                json!({"jsonrpc": "2.0", "id": 3, "method": "server/discover", "params": {"_meta": {META_VERSION: MODERN_VERSION}}}),
                json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list", "params": {"_meta": {META_VERSION: "1900-01-01"}}}),
                json!({"jsonrpc": "2.0", "id": 5, "method": "nope"}),
            ],
        );
        assert_eq!(replies.len(), 5, "notifications get no reply");
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
        assert!(
            replies[0]["result"].get("resultType").is_none(),
            "legacy replies stay legacy"
        );
        assert_eq!(replies[1]["result"], json!({}));
        assert_eq!(replies[2]["result"]["resultType"], "complete");
        assert_eq!(replies[2]["result"]["supportedVersions"][0], MODERN_VERSION);
        assert_eq!(
            replies[2]["result"]["_meta"][META_SERVER]["name"],
            "diskray"
        );
        assert_eq!(replies[3]["error"]["code"], -32022);
        assert_eq!(replies[3]["error"]["data"]["requested"], "1900-01-01");
        assert_eq!(replies[4]["error"]["code"], -32601);
        assert_eq!(server.client, "Claude Code");
    }

    #[test]
    fn malformed_and_oversized_input_gets_protocol_errors() {
        let (_home, mut server) = fixture();
        let mut input = String::from("not json\n");
        input.push_str(&"x".repeat(MAX_LINE_BYTES + 10));
        input.push('\n');
        input.push_str(&format!(
            "{}\n",
            json!({"jsonrpc": "2.0", "id": 9, "method": "ping"})
        ));
        let mut output = Vec::new();
        serve(input.as_bytes(), &mut output, &mut server).unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies[0]["error"]["code"], -32700);
        assert_eq!(replies[1]["error"]["code"], -32600);
        assert_eq!(replies[2]["id"], 9, "the server keeps serving");
    }

    #[test]
    fn tools_list_is_stable_and_read_only_except_the_proposal() {
        let tools = tool_definitions();
        let names: Vec<_> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.first(), Some(&"disk_overview"));
        for tool in &tools {
            let read_only = tool["annotations"]["readOnlyHint"] == true;
            assert_eq!(
                read_only,
                tool["name"] != "propose_cleanup",
                "{}",
                tool["name"]
            );
            assert_eq!(tool["annotations"]["destructiveHint"], false);
        }
    }

    #[test]
    fn path_escapes_are_refused_and_private_folders_show_only_a_size() {
        let (home, mut server) = fixture();
        let root = home.path().canonicalize().unwrap();
        std::os::unix::fs::symlink("/etc", root.join("etc-link")).unwrap();
        let replies = exchange(
            &mut server,
            &[
                call(1, "list_children", json!({"path": "~/Library/../../"})),
                call(2, "list_children", json!({"path": "/etc"})),
                call(3, "list_children", json!({"path": "~/etc-link"})),
                call(4, "list_children", json!({"path": "relative/path"})),
                call(5, "folder_age", json!({"path": "~/.ssh"})),
                call(6, "list_children", json!({"path": "~/Library/Caches"})),
            ],
        );
        for reply in &replies[..4] {
            assert_eq!(reply["result"]["isError"], true, "{reply}");
        }
        let private = replies[4]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(private.contains("private location"), "{private}");
        let listing = replies[5]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(listing.contains("pip"), "{listing}");
        assert!(listing.contains("n2"), "subfolders get handles");
        let replies = exchange(&mut server, &[call(7, "folder_age", json!({"path": "n2"}))]);
        assert_eq!(
            replies[0]["result"]["isError"], false,
            "handles from earlier results work"
        );
    }

    #[test]
    fn proposals_only_save_eligible_targets_and_never_delete() {
        let (home, mut server) = fixture();
        let root = home.path().canonicalize().unwrap();
        let replies = exchange(
            &mut server,
            &[
                call(
                    1,
                    "propose_cleanup",
                    json!({"paths": ["~/Library/Caches/Review"], "reason": "space"}),
                ),
                call(
                    2,
                    "propose_cleanup",
                    json!({"paths": ["~/Library/Caches/pip", "~/Library/Caches/Review"], "reason": "rebuildable"}),
                ),
            ],
        );
        assert_eq!(
            replies[0]["result"]["isError"], true,
            "review data cannot be proposed"
        );
        let saved = replies[1]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(saved.contains("Nothing was deleted"), "{saved}");
        assert!(saved.contains("Not eligible"));
        let (_, plan) = pending::newest(&root, care::timestamp()).unwrap();
        assert_eq!(plan.actions.len(), 1);
        assert!(plan.actions[0].id.starts_with("clean:"));
        assert!(root.join("Library/Caches/pip/wheel").exists());
    }

    #[test]
    fn the_module_contains_no_deletion_calls() {
        let source = include_str!("mcp.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        for forbidden in [
            "clean_cache",
            "remove_file",
            "remove_dir",
            "signal_process",
            "relocation::execute",
        ] {
            assert!(
                !code.contains(forbidden),
                "mcp.rs must not call {forbidden}"
            );
        }
    }
}
