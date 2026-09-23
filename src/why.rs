// SPDX-License-Identifier: GPL-3.0-or-later
//! The facts behind "why is my disk full?", shared by `diskray why` and the
//! interactive Why screen so both always tell the same story.

use crate::{
    agent_tools::short_path,
    ai,
    care::{Finding, Target},
    storage::{StorageInventory, VolumeStats},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// One item worth attention, linked back to the finding that explains it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub finding_id: String,
    pub label: String,
    pub size_kb: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct Overview {
    pub capacity_kb: u64,
    pub used_kb: u64,
    pub free_kb: u64,
    /// Space the folder walk could read on this volume.
    pub measured_kb: u64,
    /// Used space the walk cannot see (snapshots, system data, unreadable).
    pub unaccounted_kb: u64,
    /// Space used by other volumes in the same APFS container.
    pub other_volumes_kb: u64,
    pub snapshots: usize,
    /// Hidden-space figures only mean something for a whole volume.
    pub whole_volume: bool,
    pub folder_walk: bool,
    pub largest: Vec<(PathBuf, u64)>,
    pub quick_wins: Vec<Item>,
    pub review: Vec<Item>,
}

impl Overview {
    pub fn hidden_kb(&self) -> u64 {
        self.unaccounted_kb + self.other_volumes_kb
    }

    pub fn quick_win_kb(&self) -> u64 {
        self.quick_wins.iter().map(|item| item.size_kb).sum()
    }
}

/// Summarize one assessment. `review` holds at most `review_limit` items.
pub fn overview(
    root: &Path,
    home: &Path,
    volume: Option<&VolumeStats>,
    inventory: Option<&StorageInventory>,
    findings: &[Finding],
    review_limit: usize,
) -> Overview {
    let whole = whole_volume(root);
    let label = |finding: &Finding| match &finding.target {
        Target::Cache(path) | Target::Folder(path) => display_path(path, root, home),
        _ => ai::display_text(&finding.title),
    };
    let quick_wins = findings
        .iter()
        .filter(|finding| finding.quick_win)
        .map(|finding| Item {
            finding_id: finding.id.clone(),
            label: ai::display_text(&finding.title),
            size_kb: finding.size_kb,
            reason: ai::display_text(&finding.consequence),
        })
        .collect();
    // The largest measured items that are not quick wins. The low-space
    // alert repeats the capacity line, and unreadable items have no size.
    let mut ranked: Vec<&Finding> = findings
        .iter()
        .filter(|finding| !finding.quick_win && finding.size_kb > 0)
        .filter(|finding| matches!(finding.target, Target::Cache(_) | Target::Folder(_)))
        .collect();
    ranked.sort_by_key(|finding| std::cmp::Reverse(finding.size_kb));
    let review = ranked
        .iter()
        .take(review_limit)
        .map(|finding| Item {
            finding_id: finding.id.clone(),
            label: if finding.id == crate::care::MACOS_FINDING_ID {
                finding.title.clone()
            } else {
                label(finding)
            },
            size_kb: finding.size_kb,
            reason: ai::display_text(&finding.consequence),
        })
        .collect();
    let largest = ranked
        .iter()
        .filter(|finding| matches!(finding.target, Target::Folder(_)))
        .filter(|finding| finding.id != crate::care::MACOS_FINDING_ID)
        .take(5)
        .filter_map(|finding| match &finding.target {
            Target::Folder(path) => Some((path.clone(), finding.size_kb)),
            _ => None,
        })
        .collect();
    Overview {
        capacity_kb: volume.map_or(0, |v| v.capacity_kb),
        used_kb: volume.map_or(0, VolumeStats::disk_used_kb),
        free_kb: volume.map_or(0, VolumeStats::disk_free_kb),
        measured_kb: inventory.map_or(0, |i| i.scanned_on_volume_kb),
        unaccounted_kb: inventory.filter(|_| whole).map_or(0, |i| i.unaccounted_kb),
        other_volumes_kb: volume
            .filter(|_| whole)
            .map_or(0, VolumeStats::other_volume_kb),
        snapshots: inventory.map_or(0, |i| i.local_snapshots.len()),
        whole_volume: whole,
        folder_walk: inventory.is_some(),
        largest,
        quick_wins,
        review,
    }
}

/// A readable path: `~/…` under the home, relative to the scanned volume
/// otherwise.
pub fn display_path(path: &Path, root: &Path, home: &Path) -> String {
    if path.starts_with(home) || root == Path::new("/") {
        return short_path(path, home);
    }
    match path.strip_prefix(root) {
        Ok(rest) => {
            let volume = root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            ai::display_text(&format!("{volume}/{}", rest.display()))
        }
        Err(_) => short_path(path, home),
    }
}

/// Hidden-space accounting compares the folder walk with the whole volume,
/// so it only means something when the scan root is the volume itself.
pub fn whole_volume(root: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    if root == Path::new("/") {
        return true;
    }
    let device = |path: &Path| fs::metadata(path).ok().map(|metadata| metadata.dev());
    match (device(root), root.parent().and_then(device)) {
        (Some(inner), Some(outer)) => inner != outer,
        _ => false,
    }
}
