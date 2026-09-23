// SPDX-License-Identifier: GPL-3.0-or-later
//! Run the read-only assessment without the terminal interface.
//!
//! `diskray why`, `diskray ask`, and the MCP server all start from the same
//! [`Snapshot`], built by the exact assessment the interactive workspace uses.
//! Nothing here can change files.

use crate::{
    agent_tools::ToolWorld,
    cache::CacheEntry,
    care::{self, Event, Finding, Metrics, Session, Target},
    processes::ProcessEntry,
    storage::{StorageInventory, VolumeStats},
};
use std::{
    path::{Path, PathBuf},
    sync::mpsc::RecvTimeoutError,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct Options {
    pub root: PathBuf,
    /// Stop once cleanup targets are measured, before the folder walk.
    pub quick: bool,
    /// Give up waiting after this long and report what was measured.
    pub deadline: Duration,
    pub include_reinstallable: bool,
    pub retention_days: u64,
}

impl Options {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            quick: false,
            deadline: Duration::from_secs(300),
            include_reinstallable: false,
            retention_days: crate::retention::DEFAULT_RETENTION_DAYS,
        }
    }
}

/// Everything measured by one assessment.
#[derive(Debug)]
pub struct Snapshot {
    pub root: PathBuf,
    pub home: PathBuf,
    pub volume: Option<VolumeStats>,
    pub entries: Vec<CacheEntry>,
    pub processes: Vec<ProcessEntry>,
    pub metrics: Metrics,
    pub inventory: Option<StorageInventory>,
    pub findings: Vec<Finding>,
    pub history: Vec<Session>,
    /// The folder walk finished within the deadline.
    pub complete: bool,
    pub elapsed: Duration,
}

/// Collect a snapshot. `progress` receives short status lines for stderr.
pub fn collect(home: &Path, options: &Options, mut progress: impl FnMut(&str)) -> Snapshot {
    let started = Instant::now();
    let assessment = care::assess(
        options.root.clone(),
        home.to_path_buf(),
        options.include_reinstallable,
        options.retention_days,
    );
    let mut snapshot = Snapshot {
        root: options.root.clone(),
        home: home.to_path_buf(),
        volume: None,
        entries: Vec::new(),
        processes: Vec::new(),
        metrics: Metrics::default(),
        inventory: None,
        findings: Vec::new(),
        history: care::sessions(home),
        complete: false,
        elapsed: Duration::ZERO,
    };
    let mut samples = 0;
    let mut inventory_started = false;
    let mut last_report = Instant::now() - Duration::from_secs(10);
    loop {
        // CPU readings need two samples; wait for them once the scan is done.
        let ready = snapshot.complete || (options.quick && inventory_started);
        if ready && samples >= 2 {
            break;
        }
        let remaining = options.deadline.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        match assessment
            .receiver
            .recv_timeout(remaining.min(Duration::from_millis(200)))
        {
            Ok(Event::Capacity(result)) => snapshot.volume = result.ok(),
            Ok(Event::Processes(result, metrics)) => {
                snapshot.processes = result.unwrap_or_default();
                snapshot.metrics = metrics;
                samples += 1;
            }
            Ok(Event::Entry(entry)) => {
                snapshot
                    .entries
                    .retain(|existing| existing.spec.path != entry.spec.path);
                snapshot.entries.push(entry);
            }
            Ok(Event::Inventory(inventory)) => snapshot.inventory = Some(inventory),
            Ok(Event::Finished) => snapshot.complete = true,
            Ok(Event::Stage(stage)) => {
                if last_report.elapsed() > Duration::from_secs(2) {
                    progress(&stage);
                    last_report = Instant::now();
                }
            }
            Ok(Event::Progress(care::AssessmentProgress::Inventory(scan))) => {
                inventory_started = true;
                if last_report.elapsed() > Duration::from_secs(2) {
                    progress(&format!(
                        "Measuring folders · {} items · {} so far",
                        scan.items,
                        crate::cache::format_kb(scan.size_kb)
                    ));
                    last_report = Instant::now();
                }
            }
            Ok(Event::Progress(_)) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(assessment);
    let mut findings = care::findings(
        &snapshot.entries,
        &snapshot.processes,
        &snapshot.metrics,
        snapshot.inventory.as_ref(),
    );
    if let Some(finding) = snapshot.volume.as_ref().and_then(care::disk_finding) {
        findings.insert(0, finding);
    }
    snapshot.findings = findings;
    snapshot.elapsed = started.elapsed();
    snapshot
}

impl Snapshot {
    /// The action id Rust would let the model suggest for a target.
    pub fn suggestion(&self, target: &Target) -> Option<String> {
        care::suggestion_id(&self.entries, &self.processes, target)
    }

    /// Every action id currently eligible for a suggestion.
    pub fn eligible_suggestions(&self) -> Vec<String> {
        self.findings
            .iter()
            .filter_map(|finding| self.suggestion(&finding.target))
            .collect()
    }

    pub fn world<'a>(
        &'a self,
        suggest: &'a dyn Fn(&Target) -> Option<String>,
        online_research: bool,
    ) -> ToolWorld<'a> {
        ToolWorld {
            home: &self.home,
            inventory: self.inventory.as_ref(),
            entries: &self.entries,
            processes: &self.processes,
            metrics: &self.metrics,
            findings: &self.findings,
            history: &self.history,
            volume: self.volume.as_ref(),
            online_research,
            subject_pid: None,
            suggest,
        }
    }

    /// A history record of this assessment, so growth can be compared later.
    pub fn session(&self, source: &str) -> Session {
        Session {
            state: format!(
                "Assessment from `diskray {source}` · {}",
                if self.complete { "complete" } else { "partial" }
            ),
            measurements: care::measurements(&self.entries, self.inventory.as_ref()),
            after: Some(self.metrics.clone()),
            free_after_kb: self.volume.as_ref().map(VolumeStats::disk_free_kb),
            ..Session::default()
        }
    }
}
