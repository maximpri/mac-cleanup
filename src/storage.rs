// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only inventory of the storage behind a scan location.
//!
//! The cleanup candidates are intentionally narrow, but a useful storage
//! report must account for the rest of the volume too. This module walks the
//! selected filesystem without following symlinks or crossing mount points,
//! records allocated blocks (the space that matters to the filesystem), and
//! keeps the largest useful paths for review. It never deletes anything.

use std::{
    collections::{BTreeMap, HashSet},
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use serde::Serialize;

const DF_COMMAND: &str = "/bin/df";
const DISKUTIL_COMMAND: &str = "/usr/sbin/diskutil";
const TMUTIL_COMMAND: &str = "/usr/bin/tmutil";
const MAX_TOP_LEVEL_ITEMS: usize = 20;
const MAX_LARGEST_ITEMS: usize = 20;
const MAX_CANDIDATES: usize = 512;
const MAX_ERROR_PATHS: usize = 20;

/// Filesystem capacity as reported by `df` for the volume containing the
/// selected location.
#[derive(Debug, Clone, Serialize)]
pub struct VolumeStats {
    pub accounting_path: PathBuf,
    pub filesystem: String,
    pub capacity_kb: u64,
    pub used_kb: u64,
    pub free_kb: u64,
    /// Free space in the whole APFS container when `diskutil` reports it.
    /// The container pool is shared across the volume group, so this explains
    /// space that the per-volume `df` numbers cannot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_free_kb: Option<u64>,
    #[serde(skip_serializing)]
    pub device: u64,
}

impl VolumeStats {
    /// APFS volumes share a container. Keep per-volume usage for reconciling
    /// the walk, but use the shared pool for the disk fullness indicator.
    pub fn disk_used_kb(&self) -> u64 {
        self.container_free_kb
            .map_or(self.used_kb, |free| self.capacity_kb.saturating_sub(free))
    }

    pub fn disk_free_kb(&self) -> u64 {
        self.container_free_kb.unwrap_or(self.free_kb)
    }

    pub fn other_volume_kb(&self) -> u64 {
        self.disk_used_kb().saturating_sub(self.used_kb)
    }
}

/// A path found during the read-only inventory.
#[derive(Debug, Clone, Serialize)]
pub struct StorageItem {
    pub path: PathBuf,
    pub size_kb: u64,
    pub kind: StorageItemKind,
    pub category: StorageCategory,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageItemKind {
    Directory,
    File,
    Symlink,
}

/// A conservative display classification. It is a navigation aid, not a
/// deletion recommendation; all inventory items remain review-only.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageCategory {
    PersonalData,
    ApplicationData,
    DeveloperData,
    Applications,
    SystemData,
    TemporaryData,
    Other,
}

impl StorageCategory {
    /// One plain sentence about what this kind of folder usually holds.
    pub fn description(self) -> &'static str {
        match self {
            Self::PersonalData => "your own files: documents, media, projects",
            Self::ApplicationData => "data that apps keep for you",
            Self::DeveloperData => "developer tools, SDKs, simulators, and build data",
            Self::Applications => "installed apps",
            Self::SystemData => "macOS-managed data",
            Self::TemporaryData => "temporary files",
            Self::Other => "files outside the usual locations",
        }
    }

    /// What to do about a large folder of this kind.
    pub fn advice(self) -> &'static str {
        match self {
            Self::PersonalData => {
                "These are your files. Open the folder to see what is large; move or archive what you no longer need."
            }
            Self::ApplicationData => {
                "Reduce it from inside the app that owns it. Deleting app data directly can lose settings or content."
            }
            Self::DeveloperData => {
                "Usually reduced from the tool that created it, for example Xcode or a package manager."
            }
            Self::Applications => "Uninstall apps you no longer use.",
            Self::SystemData => "macOS manages this space. Diskray never changes it.",
            Self::TemporaryData => "macOS and apps usually clean this up themselves.",
            Self::Other => "Open the folder to see what is large before deciding anything.",
        }
    }

    /// A short verdict for lists.
    pub fn verdict(self) -> &'static str {
        match self {
            Self::PersonalData => "Your files",
            Self::ApplicationData => "App data",
            Self::DeveloperData => "Developer data",
            Self::Applications => "Apps",
            Self::SystemData => "macOS",
            Self::TemporaryData => "Temporary",
            Self::Other => "Other",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PersonalData => "PERSONAL DATA",
            Self::ApplicationData => "APP DATA",
            Self::DeveloperData => "DEVELOPER DATA",
            Self::Applications => "APPLICATIONS",
            Self::SystemData => "SYSTEM DATA",
            Self::TemporaryData => "TEMPORARY DATA",
            Self::Other => "OTHER",
        }
    }
}

/// The result of a storage inventory. `used_kb` comes from filesystem
/// accounting, while `scanned_on_volume_kb` is what the directory walk could
/// observe. Their difference is useful: it can point to protected paths,
/// APFS snapshots, purgeable/system-managed data, or a scan that was not
/// complete.
#[derive(Debug, Clone, Serialize)]
pub struct StorageInventory {
    pub volume: Option<VolumeStats>,
    pub volume_error: Option<String>,
    pub roots: Vec<StorageRoot>,
    pub scanned_kb: u64,
    pub scanned_on_volume_kb: u64,
    pub unaccounted_kb: u64,
    pub inventory_overage_kb: u64,
    /// Dates of the local APFS Time Machine snapshots on the accounted
    /// volume, report-only. Snapshots hold deleted data and are a common
    /// reason a directory walk finds less space than `df` reports as used.
    pub local_snapshots: Vec<String>,
    pub scanned_items: u64,
    pub scan_errors: u64,
    pub scan_error_paths: Vec<PathBuf>,
    pub complete: bool,
    pub top_level: Vec<StorageItem>,
    pub largest: Vec<StorageItem>,
    /// Complete direct-child index for interactive exploration; omitted from reports.
    #[serde(skip_serializing)]
    pub children: BTreeMap<PathBuf, Vec<StorageItem>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageRoot {
    pub path: PathBuf,
    pub size_kb: u64,
    #[serde(skip_serializing)]
    pub device: u64,
    pub scan_errors: u64,
}

/// A measured walk checkpoint, not an estimate of the work remaining.
#[derive(Debug, Clone, Default)]
pub struct ScanProgress {
    pub items: u64,
    pub size_kb: u64,
    pub errors: u64,
    pub path: PathBuf,
}

impl StorageInventory {
    /// Run a complete inventory without a cancellation request.
    pub fn scan(scan_root: &Path, account_home: &Path) -> Self {
        let never_cancel = AtomicBool::new(false);
        Self::scan_with_cancel(scan_root, account_home, &never_cancel)
    }

    /// Run an inventory while allowing the interactive UI to cancel it.
    pub fn scan_with_cancel(
        scan_root: &Path,
        account_home: &Path,
        cancel_requested: &AtomicBool,
    ) -> Self {
        Self::scan_with_progress(scan_root, account_home, cancel_requested, &mut |_| {})
    }

    pub fn scan_with_progress(
        scan_root: &Path,
        account_home: &Path,
        cancel_requested: &AtomicBool,
        progress: &mut dyn FnMut(ScanProgress),
    ) -> Self {
        let roots = inventory_roots(scan_root, account_home);
        let accounting_path = accounting_path(scan_root, account_home);
        let (volume, volume_error) = match read_volume_stats(&accounting_path) {
            Ok(stats) => (Some(stats), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let accounting_device = volume
            .as_ref()
            .map(|stats| stats.device)
            .or_else(|| device_for(&accounting_path));

        let mut scanner = InventoryScanner::new(cancel_requested, progress);
        let mut root_reports = Vec::with_capacity(roots.len());
        for root in roots {
            if cancel_requested.load(Ordering::Relaxed) {
                scanner.cancelled = true;
                break;
            }
            let report = scanner.scan_root(&root);
            root_reports.push(report);
        }
        scanner.report_progress(scan_root);

        let scanned_on_volume_kb = accounting_device.map_or(0, |device| {
            root_reports
                .iter()
                .filter(|root| root.device == device)
                .map(|root| root.size_kb)
                .sum()
        });
        let (unaccounted_kb, inventory_overage_kb) = volume.as_ref().map_or((0, 0), |volume| {
            (
                volume.used_kb.saturating_sub(scanned_on_volume_kb),
                scanned_on_volume_kb.saturating_sub(volume.used_kb),
            )
        });

        let scan_errors = scanner.errors;
        let scan_error_paths = scanner.error_paths;
        let mut top_level = scanner.top_level;
        top_level.sort_by(|left, right| {
            right
                .size_kb
                .cmp(&left.size_kb)
                .then_with(|| left.path.cmp(&right.path))
        });
        top_level.truncate(MAX_TOP_LEVEL_ITEMS);

        scanner.candidates.sort_by(|left, right| {
            right
                .size_kb
                .cmp(&left.size_kb)
                .then_with(|| left.path.cmp(&right.path))
        });
        let largest = select_largest_candidates(scanner.candidates);
        let complete = !scanner.cancelled && scan_errors == 0;
        let local_snapshots = if volume.is_some() {
            local_snapshot_dates(&accounting_path)
        } else {
            Vec::new()
        };

        for children in scanner.children.values_mut() {
            children.sort_by(|a, b| b.size_kb.cmp(&a.size_kb).then_with(|| a.path.cmp(&b.path)));
        }

        StorageInventory {
            volume,
            volume_error,
            roots: root_reports,
            scanned_kb: scanner.scanned_kb,
            scanned_on_volume_kb,
            unaccounted_kb,
            inventory_overage_kb,
            local_snapshots,
            scanned_items: scanner.scanned_items,
            scan_errors,
            scan_error_paths,
            complete,
            top_level,
            largest,
            children: scanner.children,
        }
    }

    /// Construct a report for an unexpected worker failure. This keeps the
    /// TUI and JSON output honest instead of presenting an empty volume as
    /// though it had been scanned successfully.
    pub fn unavailable(scan_root: &Path, error: impl Into<String>) -> Self {
        Self {
            volume: None,
            volume_error: Some(error.into()),
            roots: vec![StorageRoot {
                path: scan_root.to_path_buf(),
                size_kb: 0,
                device: 0,
                scan_errors: 1,
            }],
            scanned_kb: 0,
            scanned_on_volume_kb: 0,
            unaccounted_kb: 0,
            inventory_overage_kb: 0,
            local_snapshots: Vec::new(),
            scanned_items: 0,
            scan_errors: 1,
            scan_error_paths: vec![scan_root.to_path_buf()],
            complete: false,
            top_level: Vec::new(),
            largest: Vec::new(),
            children: BTreeMap::new(),
        }
    }

    pub fn used_kb(&self) -> u64 {
        self.volume.as_ref().map_or(0, |volume| volume.used_kb)
    }

    pub fn free_kb(&self) -> u64 {
        self.volume.as_ref().map_or(0, |volume| volume.free_kb)
    }

    pub fn capacity_kb(&self) -> u64 {
        self.volume.as_ref().map_or(0, |volume| volume.capacity_kb)
    }
}

struct InventoryScanner<'a> {
    cancel_requested: &'a AtomicBool,
    progress: &'a mut dyn FnMut(ScanProgress),
    last_report: Instant,
    seen: HashSet<(u64, u64)>,
    top_level: Vec<StorageItem>,
    candidates: Vec<StorageItem>,
    children: BTreeMap<PathBuf, Vec<StorageItem>>,
    scanned_kb: u64,
    scanned_items: u64,
    errors: u64,
    error_paths: Vec<PathBuf>,
    cancelled: bool,
}

impl<'a> InventoryScanner<'a> {
    fn new(cancel_requested: &'a AtomicBool, progress: &'a mut dyn FnMut(ScanProgress)) -> Self {
        Self {
            cancel_requested,
            progress,
            last_report: Instant::now(),
            seen: HashSet::new(),
            top_level: Vec::new(),
            candidates: Vec::new(),
            children: BTreeMap::new(),
            scanned_kb: 0,
            scanned_items: 0,
            errors: 0,
            error_paths: Vec::new(),
            cancelled: false,
        }
    }

    fn report_progress(&mut self, path: &Path) {
        (self.progress)(ScanProgress {
            items: self.scanned_items,
            size_kb: self.scanned_kb,
            errors: self.errors,
            path: path.to_path_buf(),
        });
        self.last_report = Instant::now();
    }

    fn scan_root(&mut self, root: &Path) -> StorageRoot {
        let metadata = match fs::symlink_metadata(root) {
            Ok(metadata) => metadata,
            Err(_) => {
                self.record_error(root);
                return StorageRoot {
                    path: root.to_path_buf(),
                    size_kb: 0,
                    device: 0,
                    scan_errors: 1,
                };
            }
        };
        let device = metadata.dev();
        let before_errors = self.errors;
        let root_size_kb = self.visit_directory(root, root, device, 0, true);
        StorageRoot {
            path: root.to_path_buf(),
            size_kb: root_size_kb,
            device,
            scan_errors: self.errors.saturating_sub(before_errors),
        }
    }

    fn visit_directory(
        &mut self,
        path: &Path,
        scan_root: &Path,
        device: u64,
        depth: usize,
        is_root: bool,
    ) -> u64 {
        if self.cancel_requested.load(Ordering::Relaxed) {
            self.cancelled = true;
            return 0;
        }

        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => {
                self.record_error(path);
                return 0;
            }
        };
        if metadata.dev() != device {
            return 0;
        }

        let kind = if metadata.file_type().is_symlink() {
            StorageItemKind::Symlink
        } else if metadata.is_dir() {
            StorageItemKind::Directory
        } else {
            StorageItemKind::File
        };
        if !self.seen.insert((metadata.dev(), metadata.ino())) {
            return 0;
        }

        self.scanned_items += 1;
        let own_kb = blocks_to_kb(metadata.blocks());
        self.scanned_kb = self.scanned_kb.saturating_add(own_kb);
        if self.scanned_items == 1 || self.last_report.elapsed() >= Duration::from_millis(200) {
            self.report_progress(path);
        }

        if kind != StorageItemKind::Directory {
            if own_kb > 0 && !is_root {
                self.add_candidate(StorageItem::new(path, own_kb, kind));
            }
            return own_kb;
        }

        self.children.entry(path.to_path_buf()).or_default();
        let mut total_kb = own_kb;
        match fs::read_dir(path) {
            Ok(reader) => {
                for item in reader {
                    if self.cancel_requested.load(Ordering::Relaxed) {
                        self.cancelled = true;
                        break;
                    }
                    let item = match item {
                        Ok(item) => item,
                        Err(_) => {
                            self.record_error(path);
                            continue;
                        }
                    };
                    let child = item.path();
                    if should_skip(&child, scan_root, depth) {
                        continue;
                    }
                    let child_metadata = match fs::symlink_metadata(&child) {
                        Ok(metadata) => metadata,
                        Err(_) => {
                            self.record_error(&child);
                            continue;
                        }
                    };
                    if child_metadata.dev() != device {
                        continue;
                    }
                    let child_kind = if child_metadata.file_type().is_symlink() {
                        StorageItemKind::Symlink
                    } else if child_metadata.is_dir() {
                        StorageItemKind::Directory
                    } else {
                        StorageItemKind::File
                    };
                    let child_size =
                        self.visit_directory(&child, scan_root, device, depth + 1, false);
                    total_kb = total_kb.saturating_add(child_size);
                    self.children
                        .entry(path.to_path_buf())
                        .or_default()
                        .push(StorageItem::new(&child, child_size, child_kind));
                    if depth == 0 && child_size > 0 {
                        self.top_level
                            .push(StorageItem::new(&child, child_size, child_kind));
                    }
                }
            }
            Err(_) => self.record_error(path),
        }

        if !is_root && total_kb > 0 && depth >= 2 {
            self.add_candidate(StorageItem::new(path, total_kb, kind));
        }
        total_kb
    }

    fn add_candidate(&mut self, item: StorageItem) {
        self.candidates.push(item);
        if self.candidates.len() >= MAX_CANDIDATES * 2 {
            self.candidates.sort_by(|left, right| {
                right
                    .size_kb
                    .cmp(&left.size_kb)
                    .then_with(|| left.path.cmp(&right.path))
            });
            self.candidates.truncate(MAX_CANDIDATES);
        }
    }

    fn record_error(&mut self, path: &Path) {
        self.errors += 1;
        if self.error_paths.len() < MAX_ERROR_PATHS {
            self.error_paths.push(path.to_path_buf());
        }
    }
}

impl StorageItem {
    fn new(path: &Path, size_kb: u64, kind: StorageItemKind) -> Self {
        Self {
            path: path.to_path_buf(),
            size_kb,
            kind,
            category: classify_path(path),
        }
    }
}

/// Folders whose names say nothing about what fills them. The breakdown
/// always looks inside these instead of reporting them.
const CONTAINER_NAMES: [&str; 16] = [
    "Users",
    "Library",
    "Application Support",
    "Containers",
    "Group Containers",
    "Caches",
    "Developer",
    "Xcode",
    "CoreSimulator",
    ".cache",
    ".local",
    "share",
    "Data",
    "private",
    "var",
    "opt",
];

/// macOS-managed locations, reported together as one row.
fn is_macos_system(path: &Path) -> bool {
    [
        "/System",
        "/usr",
        "/bin",
        "/sbin",
        "/cores",
        "/private/var/vm",
    ]
    .iter()
    .any(|root| path.starts_with(root))
        || path.ends_with("macOS Install Data")
}

/// Where the measured space went, as folders that mean something: folders
/// are opened while their name is generic (`Library`, `Caches`), while one
/// child holds most of their space, or while they contain a cleanup target,
/// and never past a cleanup target. Items are non-overlapping and sorted
/// largest first; macOS-managed space is summed into `macos_kb`.
#[derive(Debug, Clone, Default)]
pub struct Breakdown {
    pub items: Vec<StorageItem>,
    pub macos_kb: u64,
}

pub fn breakdown(inventory: &StorageInventory, targets: &[PathBuf], min_kb: u64) -> Breakdown {
    let mut result = Breakdown::default();
    let mut pending: Vec<(StorageItem, usize)> = inventory
        .top_level
        .iter()
        .map(|item| (item.clone(), 0))
        .collect();
    while let Some((item, depth)) = pending.pop() {
        if is_macos_system(&item.path) {
            result.macos_kb += item.size_kb;
            continue;
        }
        if item.size_kb < min_kb {
            continue;
        }
        let children = inventory
            .children
            .get(&item.path)
            .filter(|children| !children.is_empty());
        let is_target = targets.iter().any(|target| target == &item.path);
        let open = !is_target
            && depth < 10
            && item.kind == StorageItemKind::Directory
            && children.is_some_and(|children| {
                let generic = item
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| CONTAINER_NAMES.contains(&name))
                    || item.path.parent() == Some(Path::new("/Users"));
                let dominant = children
                    .iter()
                    .map(|child| child.size_kb)
                    .max()
                    .is_some_and(|largest| largest * 10 >= item.size_kb * 6);
                let holds_target = targets
                    .iter()
                    .any(|target| target != &item.path && target.starts_with(&item.path));
                generic || dominant || holds_target
            });
        match children.filter(|_| open) {
            Some(children) => {
                pending.extend(children.iter().map(|child| (child.clone(), depth + 1)))
            }
            None => result.items.push(item),
        }
    }
    result.items.sort_by(|left, right| {
        right
            .size_kb
            .cmp(&left.size_kb)
            .then_with(|| left.path.cmp(&right.path))
    });
    result
}

fn blocks_to_kb(blocks: u64) -> u64 {
    blocks.saturating_add(1) / 2
}

/// Allocated space under a folder grouped by each file's last modification.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgeProfile {
    pub files: u64,
    pub total_kb: u64,
    /// Allocated KB changed within 7 days, 7–90 days, 90–365 days, and earlier.
    pub buckets_kb: [u64; 4],
    /// Seconds since the most recent file modification.
    pub newest_age_secs: Option<u64>,
    /// False when the entry or time limit stopped the walk early.
    pub complete: bool,
    pub errors: u64,
}

/// Bounded, read-only walk that never follows symlinks or leaves the device.
pub fn folder_age(
    root: &Path,
    max_entries: u64,
    limit: Duration,
    cancel: &AtomicBool,
) -> io::Result<AgeProfile> {
    const DAY: u64 = 86_400;
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.is_dir() {
        return Err(io::Error::other("not a real directory"));
    }
    let device = metadata.dev();
    let now = std::time::SystemTime::now();
    let started = Instant::now();
    let mut profile = AgeProfile {
        complete: true,
        ..Default::default()
    };
    let mut entries = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    'walk: while let Some(directory) = pending.pop() {
        let Ok(reader) = fs::read_dir(&directory) else {
            profile.errors += 1;
            continue;
        };
        for item in reader {
            entries += 1;
            if entries > max_entries || started.elapsed() > limit || cancel.load(Ordering::Relaxed)
            {
                profile.complete = false;
                break 'walk;
            }
            let Ok(item) = item else {
                profile.errors += 1;
                continue;
            };
            let Ok(metadata) = fs::symlink_metadata(item.path()) else {
                profile.errors += 1;
                continue;
            };
            if metadata.file_type().is_symlink() || metadata.dev() != device {
                continue;
            }
            if metadata.is_dir() {
                pending.push(item.path());
                continue;
            }
            let kb = blocks_to_kb(metadata.blocks());
            let age = metadata
                .modified()
                .ok()
                .map(|modified| now.duration_since(modified).unwrap_or_default().as_secs())
                .unwrap_or(u64::MAX);
            let bucket = match age {
                age if age <= 7 * DAY => 0,
                age if age <= 90 * DAY => 1,
                age if age <= 365 * DAY => 2,
                _ => 3,
            };
            profile.files += 1;
            profile.total_kb += kb;
            profile.buckets_kb[bucket] += kb;
            if age != u64::MAX {
                profile.newest_age_secs = Some(profile.newest_age_secs.map_or(age, |n| n.min(age)));
            }
        }
    }
    Ok(profile)
}

fn select_largest_candidates(mut candidates: Vec<StorageItem>) -> Vec<StorageItem> {
    candidates.sort_by(|left, right| {
        right
            .size_kb
            .cmp(&left.size_kb)
            .then_with(|| left.path.cmp(&right.path))
    });

    let mut selected = Vec::with_capacity(MAX_LARGEST_ITEMS);
    for candidate in candidates {
        if selected.len() == MAX_LARGEST_ITEMS {
            break;
        }
        // Directories at depth two are the useful drill-down level after the
        // top-level table. Keep a large file at any depth because container
        // images, virtual disks, and media files are often the real culprit.
        selected.push(candidate);
    }
    selected
}

fn should_skip(path: &Path, scan_root: &Path, depth: usize) -> bool {
    if scan_root != Path::new("/") {
        return false;
    }

    // Mounted volumes below these directories are inventoried separately (or
    // intentionally excluded) so a startup-volume report does not count a
    // USB/NAS disk or walk APFS's volume-management tree twice.
    if depth == 0 {
        return path == Path::new("/Volumes");
    }
    path.starts_with("/Volumes") || path.starts_with("/System/Volumes")
}

fn inventory_roots(scan_root: &Path, account_home: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_unique_root(&mut roots, scan_root);

    if scan_root == Path::new("/") {
        // macOS keeps the user data volume behind the sealed startup volume.
        // Include it explicitly or a scan of `/` can miss nearly all personal
        // and application data while still looking successful.
        for path in [
            Path::new("/System/Volumes/Data"),
            Path::new("/System/Volumes/VM"),
        ] {
            if is_real_directory(path) {
                push_unique_root(&mut roots, path);
            }
        }
        if !roots
            .iter()
            .any(|root| root.starts_with("/System/Volumes/Data"))
            && device_for(scan_root) != device_for(account_home)
        {
            push_unique_root(&mut roots, account_home);
        }
    }
    roots
}

fn push_unique_root(roots: &mut Vec<PathBuf>, path: &Path) {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !roots.iter().any(|root| root == &resolved) {
        roots.push(resolved);
    }
}

fn is_real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

pub(crate) fn accounting_path(scan_root: &Path, account_home: &Path) -> PathBuf {
    if scan_root == Path::new("/") {
        // Prefer the APFS data volume itself so `df` describes the same
        // filesystem that contains the user's data. A network home or an
        // unusual macOS layout can fall back to the supplied account home.
        if is_real_directory(Path::new("/System/Volumes/Data")) {
            return PathBuf::from("/System/Volumes/Data");
        }
        if is_real_directory(account_home) {
            return account_home.to_path_buf();
        }
    }
    scan_root.to_path_buf()
}

fn device_for(path: &Path) -> Option<u64> {
    fs::symlink_metadata(path)
        .ok()
        .map(|metadata| metadata.dev())
}

pub(crate) fn read_volume_stats(path: &Path) -> io::Result<VolumeStats> {
    let device = device_for(path).unwrap_or_default();
    let output = Command::new(DF_COMMAND).args(["-kP"]).arg(path).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "{DF_COMMAND} could not read filesystem usage for {}",
            path.display()
        )));
    }
    let mut stats = parse_df_output(&String::from_utf8_lossy(&output.stdout), path, device)?;
    // `df` reports a device name rather than the filesystem type; the
    // presence of APFSContainerFree in `diskutil` output is what identifies
    // an APFS volume.
    let mut info = diskutil_info_plist(path);
    if info
        .as_ref()
        .and_then(|plist| plist_integer(plist, "APFSContainerFree"))
        .is_none()
    {
        let data_volume = Path::new("/System/Volumes/Data");
        if device_for(data_volume) == Some(device) {
            info = diskutil_info_plist(data_volume);
        }
    }
    if let Some(plist) = info
        && let (Some(size), Some(free)) = (
            plist_integer(&plist, "APFSContainerSize"),
            plist_integer(&plist, "APFSContainerFree"),
        )
    {
        stats.capacity_kb = size / 1_024;
        stats.container_free_kb = Some(free / 1_024);
    }
    Ok(stats)
}

fn diskutil_info_plist(path: &Path) -> Option<String> {
    let output = Command::new(DISKUTIL_COMMAND)
        .args(["info", "-plist"])
        .arg(path)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}

fn plist_integer(plist: &str, key: &str) -> Option<u64> {
    let key_marker = format!("<key>{key}</key>");
    let value = plist.split_once(&key_marker)?.1;
    let value = value.split_once("<integer>")?.1;
    value.split_once("</integer>")?.0.trim().parse().ok()
}

/// Enumerate the local APFS Time Machine snapshot dates on a volume.
/// Report-only: snapshots are never modified or deleted by this application.
fn local_snapshot_dates(mount: &Path) -> Vec<String> {
    Command::new(TMUTIL_COMMAND)
        .arg("listlocalsnapshotdates")
        .arg(mount)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| parse_snapshot_dates(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}

fn parse_snapshot_dates(output: &str) -> Vec<String> {
    let mut dates: Vec<String> = output
        .lines()
        .map(str::trim)
        .filter(|line| is_snapshot_date(line))
        .map(str::to_string)
        .collect();
    dates.sort();
    dates
}

/// `tmutil` prints one `YYYY-MM-DD-HHMMSS` date per line when local
/// snapshots exist and nothing otherwise.
fn is_snapshot_date(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() == 17
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'-'
        && line
            .char_indices()
            .all(|(index, character)| matches!(index, 4 | 7 | 10) || character.is_ascii_digit())
}

fn parse_df_output(output: &str, accounting_path: &Path, device: u64) -> io::Result<VolumeStats> {
    let line = output
        .lines()
        .skip(1)
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "df returned no filesystem row")
        })?;
    let fields: Vec<_> = line.split_whitespace().collect();
    if fields.len() < 5 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "df returned an incomplete filesystem row",
        ));
    }
    let parse = |index: usize, name: &str| {
        fields[index].parse::<u64>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("df returned an invalid {name} value"),
            )
        })
    };
    Ok(VolumeStats {
        accounting_path: accounting_path.to_path_buf(),
        filesystem: fields[0].to_string(),
        capacity_kb: parse(1, "capacity")?,
        used_kb: parse(2, "used")?,
        free_kb: parse(3, "available")?,
        container_free_kb: None,
        device,
    })
}

fn classify_path(path: &Path) -> StorageCategory {
    let components: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => {
                Some(value.to_string_lossy().to_ascii_lowercase())
            }
            _ => None,
        })
        .collect();
    let joined = components.join("/");

    if joined.starts_with("private/tmp")
        || joined.starts_with("private/var/folders")
        || joined.starts_with("tmp")
    {
        StorageCategory::TemporaryData
    } else if components.last().is_some_and(|last| last == "library") {
        StorageCategory::ApplicationData
    } else if joined.starts_with("opt/homebrew") || joined.starts_with("usr/local/cellar") {
        StorageCategory::DeveloperData
    } else if components.iter().any(|component| {
        matches!(
            component.as_str(),
            "documents" | "downloads" | "desktop" | "movies" | "music" | "pictures" | "public"
        )
    }) {
        StorageCategory::PersonalData
    } else if joined.contains("library/developer")
        || joined.contains(".npm")
        || joined.contains(".cache")
        || joined.contains(".gradle")
        || joined.contains("deriveddata")
        || joined.contains("coresimulator")
    {
        StorageCategory::DeveloperData
    } else if components
        .iter()
        .any(|component| component == "applications")
        || path.extension().is_some_and(|extension| extension == "app")
    {
        StorageCategory::Applications
    } else if joined.contains("library/application support")
        || joined.contains("library/containers")
        || joined.contains("library/group containers")
        || joined.contains("library/preferences")
    {
        StorageCategory::ApplicationData
    } else if components.iter().any(|component| {
        matches!(
            component.as_str(),
            "system" | "private" | "var" | "usr" | "bin" | "sbin"
        )
    }) {
        StorageCategory::SystemData
    } else if components.iter().any(|component| {
        matches!(
            component.as_str(),
            ".trash" | ".trashes" | ".temporaryitems" | "tmp" | "caches" | "cache"
        )
    }) {
        StorageCategory::TemporaryData
    } else {
        if components.iter().any(|component| component == "users") {
            StorageCategory::PersonalData
        } else {
            StorageCategory::Other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakdown_opens_generic_folders_and_stops_at_cleanup_targets() {
        let gib = 1_048_576;
        let dir = |path: &str, size_kb: u64| {
            StorageItem::new(Path::new(path), size_kb, StorageItemKind::Directory)
        };
        let mut inventory = StorageInventory::unavailable(Path::new("/"), "test");
        inventory.top_level = vec![
            dir("/Users", 60 * gib),
            dir("/System", 20 * gib),
            dir("/Applications", 8 * gib),
        ];
        let mut children = BTreeMap::new();
        children.insert(PathBuf::from("/Users"), vec![dir("/Users/me", 60 * gib)]);
        children.insert(
            PathBuf::from("/Users/me"),
            vec![
                dir("/Users/me/Library", 35 * gib),
                dir("/Users/me/Movies", 25 * gib),
            ],
        );
        children.insert(
            PathBuf::from("/Users/me/Library"),
            vec![
                dir("/Users/me/Library/Developer", 30 * gib),
                dir("/Users/me/Library/Mail", 5 * gib),
            ],
        );
        children.insert(
            PathBuf::from("/Users/me/Library/Developer"),
            vec![dir("/Users/me/Library/Developer/CoreSimulator", 30 * gib)],
        );
        children.insert(
            PathBuf::from("/Users/me/Library/Developer/CoreSimulator"),
            vec![dir(
                "/Users/me/Library/Developer/CoreSimulator/Devices",
                29 * gib,
            )],
        );
        children.insert(
            PathBuf::from("/Users/me/Library/Developer/CoreSimulator/Devices"),
            vec![dir(
                "/Users/me/Library/Developer/CoreSimulator/Devices/A",
                29 * gib,
            )],
        );
        children.insert(
            PathBuf::from("/Users/me/Movies"),
            vec![
                dir("/Users/me/Movies/a", 13 * gib),
                dir("/Users/me/Movies/b", 12 * gib),
            ],
        );
        inventory.children = children;
        let targets = [PathBuf::from(
            "/Users/me/Library/Developer/CoreSimulator/Devices",
        )];
        let result = breakdown(&inventory, &targets, 1024);
        let paths: Vec<_> = result
            .items
            .iter()
            .map(|item| item.path.to_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                "/Users/me/Library/Developer/CoreSimulator/Devices",
                "/Users/me/Movies",
                "/Applications",
                "/Users/me/Library/Mail",
            ],
            "generic folders are opened, a cleanup target is never opened, and balanced folders stay whole"
        );
        assert_eq!(result.macos_kb, 20 * gib);
    }
    use std::io::Write;

    #[test]
    fn folder_age_buckets_allocated_space_by_modification_time() {
        let temp = tempfile::tempdir().unwrap();
        let day = Duration::from_secs(86_400);
        let now = std::time::SystemTime::now();
        fs::create_dir(temp.path().join("nested")).unwrap();
        for (name, age) in [
            ("fresh.bin", Duration::ZERO),
            ("month.bin", day * 30),
            ("nested/half-year.bin", day * 200),
            ("nested/ancient.bin", day * 800),
        ] {
            let path = temp.path().join(name);
            fs::write(&path, vec![1; 64 * 1024]).unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(now - age)
                .unwrap();
        }
        std::os::unix::fs::symlink("/", temp.path().join("root-link")).unwrap();
        let never = AtomicBool::new(false);
        let profile = folder_age(temp.path(), 1000, Duration::from_secs(5), &never).unwrap();
        assert_eq!(profile.files, 4);
        assert!(profile.complete);
        assert!(profile.buckets_kb.iter().all(|kb| *kb > 0));
        assert_eq!(profile.total_kb, profile.buckets_kb.iter().sum::<u64>());
        assert!(profile.newest_age_secs.unwrap() < 60);
        let limited = folder_age(temp.path(), 2, Duration::from_secs(5), &never).unwrap();
        assert!(!limited.complete, "an entry cap reports a partial walk");
        assert!(
            folder_age(
                &temp.path().join("fresh.bin"),
                10,
                Duration::from_secs(1),
                &never
            )
            .is_err()
        );
    }

    #[test]
    fn exploration_retains_all_children_and_classifies_user_app_data() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..35 {
            let path = temp.path().join(format!("folder-{index:02}"));
            fs::create_dir(&path).unwrap();
            fs::write(path.join("data.bin"), vec![1; (index + 1) * 4096]).unwrap();
        }
        let report = StorageInventory::scan(temp.path(), temp.path());
        let root = temp.path().canonicalize().unwrap();
        let children = &report.children[&root];
        assert_eq!(children.len(), 35);
        assert!(
            children
                .windows(2)
                .all(|pair| pair[0].size_kb >= pair[1].size_kb)
        );
        assert_eq!(report.children[&root.join("folder-00")].len(), 1);
        assert_eq!(
            classify_path(Path::new("/Users/demo/Library/Developer/Xcode")),
            StorageCategory::DeveloperData
        );
        assert_eq!(
            classify_path(Path::new("/Users/demo/Library/Application Support/App")),
            StorageCategory::ApplicationData
        );
        assert_eq!(
            classify_path(Path::new("/Users/demo/Library/Caches/App")),
            StorageCategory::TemporaryData
        );
        assert!(
            serde_json::to_value(&report)
                .unwrap()
                .get("children")
                .is_none()
        );
    }

    #[test]
    fn inventory_reports_top_level_and_nested_consumers() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("volume");
        let home = root.join("Users/tester");
        fs::create_dir_all(home.join("Library/Application Support/app")).unwrap();
        fs::create_dir_all(home.join("Documents/project")).unwrap();
        fs::create_dir_all(root.join("Applications/Big.app/Contents")).unwrap();
        fs::File::create(home.join("Library/Application Support/app/state.db"))
            .unwrap()
            .write_all(&[1_u8; 32_768])
            .unwrap();
        fs::File::create(home.join("Documents/project/archive.bin"))
            .unwrap()
            .write_all(&[2_u8; 16_384])
            .unwrap();

        let report = StorageInventory::scan(&root, &home);

        assert!(report.complete);
        assert!(report.scanned_items >= 6);
        assert!(report.scanned_kb >= 48);
        assert!(
            report
                .top_level
                .iter()
                .any(|item| item.path.ends_with("volume/Users"))
        );
        assert!(
            report
                .largest
                .iter()
                .any(|item| item.path.ends_with("project") || item.path.ends_with("app"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn inventory_does_not_follow_symlinks_or_double_count_hardlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("volume");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("original.bin"), [3_u8; 16_384]).unwrap();
        fs::hard_link(root.join("original.bin"), root.join("hard-link.bin")).unwrap();
        symlink(root.join("original.bin"), root.join("linked.bin")).unwrap();

        let report = StorageInventory::scan(&root, &root);
        let original_size = report
            .top_level
            .iter()
            .find(|item| item.path.ends_with("original.bin"))
            .map(|item| item.size_kb)
            .unwrap();

        assert!(original_size > 0);
        assert!(report.scanned_kb < original_size.saturating_mul(3));
        assert!(report.top_level.iter().all(
            |item| !item.path.ends_with("linked.bin") || item.kind == StorageItemKind::Symlink
        ));
    }

    #[test]
    fn parses_df_rows_with_mount_paths_after_the_fixed_columns() {
        let report = parse_df_output(
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk1 1000 700 300 70% /Volumes/Work Drive\n",
            Path::new("/Volumes/Work Drive"),
            42,
        )
        .unwrap();

        assert_eq!(report.capacity_kb, 1000);
        assert_eq!(report.used_kb, 700);
        assert_eq!(report.free_kb, 300);
        assert_eq!(report.device, 42);
        assert_eq!(report.container_free_kb, None);
    }

    #[test]
    fn progress_reports_actual_monotonic_work_and_final_totals() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("documents")).unwrap();
        fs::write(temp.path().join("documents/example"), [1_u8; 8192]).unwrap();
        let cancel = AtomicBool::new(false);
        let mut progress = Vec::new();
        let inventory =
            StorageInventory::scan_with_progress(temp.path(), temp.path(), &cancel, &mut |p| {
                progress.push(p)
            });
        assert!(progress.len() >= 2);
        assert!(
            progress
                .windows(2)
                .all(|p| p[0].items <= p[1].items && p[0].size_kb <= p[1].size_kb)
        );
        let last = progress.last().unwrap();
        assert_eq!(last.items, inventory.scanned_items);
        assert_eq!(last.size_kb, inventory.scanned_kb);
        assert_eq!(last.errors, inventory.scan_errors);
        assert_eq!(last.path, temp.path());
    }

    #[test]
    fn progress_preserves_cancellation_and_does_not_claim_completion() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("untouched"), [1_u8; 8192]).unwrap();
        let cancel = AtomicBool::new(false);
        let inventory =
            StorageInventory::scan_with_progress(temp.path(), temp.path(), &cancel, &mut |_| {
                cancel.store(true, Ordering::Relaxed)
            });
        assert!(!inventory.complete);
        assert!(temp.path().join("untouched").exists());
        assert!(inventory.scanned_items <= 1);
    }

    #[test]
    fn shared_container_usage_does_not_replace_volume_accounting() {
        let mut volume = parse_df_output(
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk1 250 180 3 98% /\n",
            Path::new("/"), 1,
        ).unwrap();
        assert_eq!(volume.disk_used_kb(), 180);
        volume.container_free_kb = Some(3);
        assert_eq!(volume.disk_used_kb(), 247);
        assert_eq!(volume.other_volume_kb(), 67);
        assert_eq!(volume.used_kb, 180);
        assert_eq!(volume.disk_free_kb(), 3);
    }

    #[test]
    fn parses_snapshot_dates_and_ignores_prose() {
        let dates = parse_snapshot_dates(
            "Snapshot dates for volume group containing disk /:\n2026-09-01-101530\nnot-a-date\n2025-12-31-235959\n",
        );
        assert_eq!(dates, vec!["2025-12-31-235959", "2026-09-01-101530"]);
        assert!(
            parse_snapshot_dates("Snapshot dates for volume group containing disk /:").is_empty()
        );
    }

    #[test]
    fn reads_integer_values_from_diskutil_plist() {
        let plist = "<key>APFSContainerFree</key>\n\t<integer>15434162176</integer>\n";
        assert_eq!(
            plist_integer(plist, "APFSContainerFree"),
            Some(15_434_162_176)
        );
        assert_eq!(plist_integer(plist, "Missing"), None);
    }

    #[test]
    fn cancellation_marks_inventory_incomplete() {
        let temp = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(true);

        let report = StorageInventory::scan_with_cancel(temp.path(), temp.path(), &cancel);

        assert!(!report.complete);
        assert!(report.scanned_items == 0);
    }
}
