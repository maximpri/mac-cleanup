// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit relocation of large user-owned directories to an external volume.
//!
//! Relocation is intentionally separate from cleanup. The source is copied to
//! a new directory on a different mounted volume, the copy is verified, and
//! only then is the original directory replaced with an absolute symlink. A
//! failed copy leaves the source untouched.

use std::{
    fs,
    io::{self, Read},
    os::unix::fs::{MetadataExt, symlink},
    path::{Path, PathBuf},
    process,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

use crate::cache::{self, CacheTarget, PathIdentity};

const VOLUMES_ROOT: &str = "/Volumes";
const TEMP_ROOT: &str = "/private/tmp";

/// A currently mounted, writable external Mac volume. This is a measurement,
/// never authorization to move data; planning and execution recheck it.
#[derive(Debug, Clone)]
pub struct Destination {
    pub path: PathBuf,
    pub free_kb: u64,
    pub capacity_kb: u64,
    pub volume_uuid: String,
}

/// Include room for metadata and allocation overhead. Sparse/compressed data
/// may expand when copied, so allocated size alone is not enough.
pub fn required_destination_kb(allocated_kb: u64, logical_bytes: u64) -> u64 {
    let data = allocated_kb.max(logical_bytes.div_ceil(1024));
    data.saturating_add((data / 20).max(64 * 1024))
}

fn plist_value<'a>(plist: &'a str, key: &str, tag: &str) -> Option<&'a str> {
    let value = plist
        .split_once(&format!("<key>{key}</key>"))?
        .1
        .trim_start();
    value
        .strip_prefix(&format!("<{tag}>"))?
        .split_once(&format!("</{tag}>"))
        .map(|(value, _)| value)
}

fn plist_bool(plist: &str, key: &str) -> Option<bool> {
    let value = plist
        .split_once(&format!("<key>{key}</key>"))?
        .1
        .trim_start();
    if value.starts_with("<true/>") {
        Some(true)
    } else if value.starts_with("<false/>") {
        Some(false)
    } else {
        None
    }
}

fn destination_from_info(path: PathBuf, info: &str) -> Result<Destination, String> {
    if plist_bool(info, "Internal") != Some(false) {
        return Err("Choose an external disk; internal or unidentified disks are not relocation destinations.".into());
    }
    if plist_value(info, "BusProtocol", "string") == Some("Disk Image") {
        return Err("Choose a physical external drive, not a disk image.".into());
    }
    if plist_bool(info, "WritableVolume") != Some(true) {
        return Err(
            "The external volume is read-only or its write access could not be verified.".into(),
        );
    }
    if !matches!(
        plist_value(info, "FilesystemType", "string"),
        Some("apfs" | "hfs")
    ) {
        return Err(
            "Choose an APFS or Mac OS Extended external volume to preserve Mac file metadata."
                .into(),
        );
    }
    let number = |key| plist_value(info, key, "integer").and_then(|s| s.trim().parse::<u64>().ok());
    let free = match (number("VolumeFreeSpace"), number("APFSContainerFree")) {
        (Some(volume), Some(container)) => Some(volume.min(container)),
        (volume, container) => volume.or(container),
    }
    .or_else(|| number("FreeSpace"))
    .ok_or("Could not measure free space on the external volume.")?;
    let uuid = plist_value(info, "VolumeUUID", "string")
        .filter(|s| !s.is_empty())
        .ok_or("Could not identify the external volume.")?;
    Ok(Destination {
        path,
        free_kb: free / 1024,
        capacity_kb: number("TotalSize").ok_or("Could not measure external volume capacity.")?
            / 1024,
        volume_uuid: uuid.to_owned(),
    })
}

/// Validate a destination without creating files or directories.
pub fn inspect_destination(path: &Path, source: &Path) -> Result<Destination, String> {
    let path = validate_destination_root(path, source)?;
    let text = path
        .to_str()
        .ok_or("Destination path is not valid UTF-8.")?;
    let info = crate::care::query(
        "/usr/sbin/diskutil",
        &["info", "-plist", text],
        Duration::from_secs(5),
    )
    .map_err(|error| format!("Could not inspect the external volume: {error}"))?;
    destination_from_info(path, &info)
}

/// Discover a bounded list; callers run this off the UI thread.
pub fn destinations(source: &Path, cancelled: &std::sync::atomic::AtomicBool) -> Vec<Destination> {
    let mut found = Vec::new();
    let began = std::time::Instant::now();
    if let Ok(entries) = fs::read_dir(VOLUMES_ROOT) {
        for entry in entries.take(32).filter_map(Result::ok) {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed)
                || began.elapsed() >= Duration::from_secs(10)
            {
                break;
            }
            if let Ok(destination) = inspect_destination(&entry.path(), source) {
                found.push(destination);
            }
            if found.len() == 8 {
                break;
            }
        }
    }
    found.sort_by(|a, b| b.free_kb.cmp(&a.free_kb).then_with(|| a.path.cmp(&b.path)));
    found
}

#[derive(Debug, Clone, Serialize)]
pub struct RelocationPlan {
    pub source: PathBuf,
    pub destination_root: PathBuf,
    pub destination: PathBuf,
    pub size_kb: u64,
    pub logical_bytes: u64,
    pub file_count: u64,
    pub directory_count: u64,
    pub destination_free_kb: u64,
    pub required_destination_kb: u64,
    #[serde(skip)]
    source_identity: PathIdentity,
    #[serde(skip)]
    destination_identity: PathIdentity,
    #[serde(skip)]
    destination_volume_uuid: String,
    #[serde(skip)]
    account_home: PathBuf,
    #[serde(skip)]
    process_pattern: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelocationStatus {
    Planned,
    Relocated,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelocationReport {
    pub source: PathBuf,
    pub destination_root: PathBuf,
    pub destination: PathBuf,
    pub size_kb: u64,
    pub logical_bytes: u64,
    pub file_count: u64,
    pub directory_count: u64,
    pub destination_free_kb: u64,
    pub required_destination_kb: u64,
    pub status: RelocationStatus,
    pub backup_removed: bool,
    pub message: Option<String>,
}

impl RelocationReport {
    pub fn succeeded(&self) -> bool {
        self.status == RelocationStatus::Relocated
    }
}

/// Validate a source directory and an external destination, without changing
/// the filesystem. `process_pattern` is supplied when the source is a known
/// app-managed location so its owning application can also block relocation.
pub fn plan(
    source: &Path,
    destination_root: &Path,
    account_home: &Path,
    process_pattern: Option<&str>,
) -> Result<RelocationPlan, String> {
    require_absolute(source, "source")?;
    require_absolute(destination_root, "destination")?;

    let source = validate_source(source, account_home)?;
    let source_identity = PathIdentity::capture(&source).ok_or("Could not identify the source.")?;
    let destination_info = inspect_destination(destination_root, &source)?;
    let destination_root = destination_info.path;
    let destination_identity = PathIdentity::capture(&destination_root)
        .ok_or("Could not identify the destination directory.")?;
    let source_device = fs::symlink_metadata(&source)
        .map_err(|error| format!("cannot inspect source {}: {error}", source.display()))?
        .dev();
    let destination_device = fs::symlink_metadata(&destination_root)
        .map_err(|error| {
            format!(
                "cannot inspect destination {}: {error}",
                destination_root.display()
            )
        })?
        .dev();
    if source_device == destination_device {
        return Err(format!(
            "destination {} is on the same filesystem as the source; choose a mounted external volume",
            destination_root.display()
        ));
    }

    let stats = tree_stats(&source).map_err(|error| {
        format!(
            "source {} could not be safely inspected: {error}",
            source.display()
        )
    })?;
    let size_kb = cache::measure_target_kb(&source, CacheTarget::ExactPath)
        .map_err(|error| format!("could not measure source {}: {error}", source.display()))?;
    let destination_info = inspect_destination(&destination_root, &source)?;
    let required_destination_kb = required_destination_kb(size_kb, stats.logical_bytes);
    if destination_info.free_kb < required_destination_kb {
        return Err(format!(
            "Not enough external space: need {required_destination_kb} KiB including copy overhead; {} KiB available. Nothing was moved.",
            destination_info.free_kb
        ));
    }
    ensure_not_open(&source)?;
    ensure_related_process_closed(process_pattern)?;

    let name = source
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("source has no usable directory name: {}", source.display()))?;
    let destination = destination_root.join(name);
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            return Err(format!(
                "destination already exists: {}",
                destination.display()
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot check destination {}: {error}",
                destination.display()
            ));
        }
    }

    let account_home = account_home.canonicalize().map_err(|error| {
        format!(
            "cannot resolve the current account home {}: {error}",
            account_home.display()
        )
    })?;
    let plan = RelocationPlan {
        source,
        destination_root,
        destination,
        size_kb,
        logical_bytes: stats.logical_bytes,
        file_count: stats.file_count,
        directory_count: stats.directory_count,
        destination_free_kb: destination_info.free_kb,
        required_destination_kb,
        source_identity,
        destination_identity,
        destination_volume_uuid: destination_info.volume_uuid,
        account_home,
        process_pattern: process_pattern
            .filter(|pattern| !pattern.is_empty())
            .map(str::to_owned),
    };
    ensure_identities(&plan)?;
    Ok(plan)
}

pub fn preview(plan: &RelocationPlan) -> RelocationReport {
    report_from_plan(
        plan,
        RelocationStatus::Planned,
        false,
        Some("No files changed. Re-run with --yes to perform the verified move and symlink replacement.".into()),
    )
}

/// Execute a previously validated relocation. Every important source and
/// destination check is repeated before the source is renamed.
pub fn execute(plan: &RelocationPlan) -> RelocationReport {
    match execute_inner(plan) {
        Ok(details) => report_from_plan(
            plan,
            RelocationStatus::Relocated,
            details.backup_removed,
            details.message,
        ),
        Err(error) => report_from_plan(plan, RelocationStatus::Failed, false, Some(error)),
    }
}

struct ExecutionDetails {
    backup_removed: bool,
    message: Option<String>,
}

fn execute_inner(prepared: &RelocationPlan) -> Result<ExecutionDetails, String> {
    ensure_identities(prepared)?;
    let current = plan(
        &prepared.source,
        &prepared.destination_root,
        &prepared.account_home,
        prepared.process_pattern.as_deref(),
    )?;
    if current.destination_volume_uuid != prepared.destination_volume_uuid
        || current.destination != prepared.destination
        || current.size_kb != prepared.size_kb
        || current.logical_bytes != prepared.logical_bytes
        || current.file_count != prepared.file_count
        || current.directory_count != prepared.directory_count
    {
        return Err("the source changed after the relocation preview; nothing was moved".into());
    }

    execute_after_revalidation(prepared)
}

#[cfg(test)]
fn execute_inner_for_test(prepared: &RelocationPlan) -> Result<ExecutionDetails, String> {
    execute_after_revalidation(prepared)
}

fn execute_after_revalidation(prepared: &RelocationPlan) -> Result<ExecutionDetails, String> {
    let staging = unique_sibling(&prepared.destination, "copy")?;
    let backup = unique_sibling(&prepared.source, "original")?;
    let copied = match copy_tree(&prepared.source, &staging) {
        Ok(stats) => stats,
        Err(error) => {
            remove_owned_tree(&staging);
            return Err(format!("copy to {} failed: {error}", staging.display()));
        }
    };
    let expected_stats = TreeStats {
        logical_bytes: prepared.logical_bytes,
        file_count: prepared.file_count,
        directory_count: prepared.directory_count,
    };
    if copied != expected_stats {
        remove_owned_tree(&staging);
        return Err("the copied directory did not match the source; nothing was moved".into());
    }

    if let Err(error) = ensure_source_unchanged(prepared, &staging) {
        remove_owned_tree(&staging);
        return Err(error);
    }
    rename_exclusive(&staging, &prepared.destination).map_err(|error| {
        remove_owned_tree(&staging);
        format!(
            "could not commit the copy at {}: {error}",
            prepared.destination.display()
        )
    })?;

    // Verifying a large copy takes time. Re-check right before the original
    // moves aside so a file opened during verification is never stranded in a
    // backup that is about to be deleted.
    if let Err(error) = pre_link_checks(prepared) {
        remove_owned_tree(&prepared.destination);
        return Err(format!("{error}; the original was not moved"));
    }
    if let Err(error) = rename_exclusive(&prepared.source, &backup) {
        remove_owned_tree(&prepared.destination);
        return Err(format!(
            "could not stage the original source for linking: {error}"
        ));
    }

    // Catch same-size edits made after the initial verification. Restore the
    // original if the committed destination no longer matches it.
    if let Err(error) = verify_copy(&backup, &prepared.destination) {
        return Err(rollback_after_link_failure(
            &prepared.source,
            &backup,
            &prepared.destination,
            format!("the original changed before linking: {error}"),
        ));
    }

    if let Err(error) = symlink(&prepared.destination, &prepared.source) {
        return Err(rollback_after_link_failure(
            &prepared.source,
            &backup,
            &prepared.destination,
            format!("could not create the replacement symlink: {error}"),
        ));
    }
    if !matches!(fs::read_link(&prepared.source), Ok(target) if target == prepared.destination) {
        return Err(rollback_after_link_failure(
            &prepared.source,
            &backup,
            &prepared.destination,
            "the replacement symlink did not point to the verified destination".into(),
        ));
    }

    match fs::remove_dir_all(&backup) {
        Ok(()) => Ok(ExecutionDetails {
            backup_removed: true,
            message: Some(format!(
                "moved and verified {} file(s); replaced the original directory with a symlink",
                prepared.file_count
            )),
        }),
        Err(error) => Ok(ExecutionDetails {
            backup_removed: false,
            message: Some(format!(
                "moved and linked successfully, but the temporary original backup {} could not be removed: {error}",
                backup.display()
            )),
        }),
    }
}

fn report_from_plan(
    plan: &RelocationPlan,
    status: RelocationStatus,
    backup_removed: bool,
    message: Option<String>,
) -> RelocationReport {
    RelocationReport {
        source: plan.source.clone(),
        destination_root: plan.destination_root.clone(),
        destination: plan.destination.clone(),
        size_kb: plan.size_kb,
        logical_bytes: plan.logical_bytes,
        file_count: plan.file_count,
        directory_count: plan.directory_count,
        destination_free_kb: plan.destination_free_kb,
        required_destination_kb: plan.required_destination_kb,
        status,
        backup_removed,
        message,
    }
}

fn validate_source(source: &Path, account_home: &Path) -> Result<PathBuf, String> {
    if cache::has_symlink_component_below(source, Path::new("/")) {
        return Err(format!(
            "source or one of its path components is a symlink: {}",
            source.display()
        ));
    }
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("cannot access source {}: {error}", source.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "relocation source must be a real directory: {}",
            source.display()
        ));
    }
    let source = source
        .canonicalize()
        .map_err(|error| format!("cannot resolve source {}: {error}", source.display()))?;
    let account_home = account_home.canonicalize().map_err(|error| {
        format!(
            "cannot resolve the current account home {}: {error}",
            account_home.display()
        )
    })?;
    let under_home = source.starts_with(&account_home) && source != account_home;
    let temp_root = Path::new(TEMP_ROOT);
    let under_temp = source.starts_with(temp_root) && source != temp_root;
    if !under_home && !under_temp {
        return Err(format!(
            "source must be inside the current account home or a child of {TEMP_ROOT}: {}",
            source.display()
        ));
    }
    let uid = cache::effective_user_id()
        .ok_or_else(|| "could not determine the effective user ID safely".to_string())?;
    verify_tree(&source, uid).map_err(|error| {
        format!("source contains data that is not entirely owned by the current account: {error}")
    })?;
    Ok(source)
}

fn validate_destination_root(destination_root: &Path, source: &Path) -> Result<PathBuf, String> {
    if cache::has_symlink_component_below(destination_root, Path::new("/")) {
        return Err(format!(
            "destination or one of its path components is a symlink: {}",
            destination_root.display()
        ));
    }
    let destination_root = destination_root.canonicalize().map_err(|error| {
        format!(
            "cannot resolve destination {}: {error}",
            destination_root.display()
        )
    })?;
    let relative = destination_root.strip_prefix(VOLUMES_ROOT).map_err(|_| {
        format!(
            "destination must be on a mounted external volume under {VOLUMES_ROOT}: {}",
            destination_root.display()
        )
    })?;
    let Some(volume_name) = relative.components().next() else {
        return Err("destination must name a mounted volume under /Volumes".into());
    };
    if !fs::symlink_metadata(&destination_root)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
    {
        return Err(format!(
            "destination is not a real directory: {}",
            destination_root.display()
        ));
    }
    let volume_root = Path::new(VOLUMES_ROOT).join(volume_name.as_os_str());
    let volume_device = fs::symlink_metadata(&volume_root)
        .map_err(|e| e.to_string())?
        .dev();
    let parent_device = fs::symlink_metadata(VOLUMES_ROOT)
        .map_err(|e| e.to_string())?
        .dev();
    let source_device = fs::symlink_metadata(source)
        .map_err(|e| e.to_string())?
        .dev();
    let destination_device = fs::symlink_metadata(&destination_root)
        .map_err(|e| e.to_string())?
        .dev();
    if volume_device == parent_device
        || volume_device == source_device
        || destination_device != volume_device
    {
        return Err("Destination must be a mounted external filesystem, different from the source; ordinary /Volumes folders are not supported.".into());
    }
    if destination_root == source || destination_root.starts_with(source) {
        return Err("relocation destination cannot be inside the source".into());
    }
    Ok(destination_root)
}

fn ensure_not_open(path: &Path) -> Result<(), String> {
    match cache::path_is_open(path) {
        Ok(true) => Err(format!(
            "a process currently has the source open: {}",
            path.display()
        )),
        Ok(false) => Ok(()),
        Err(error) => Err(format!(
            "could not verify whether the source is open: {error}"
        )),
    }
}

fn ensure_related_process_closed(process_pattern: Option<&str>) -> Result<(), String> {
    if let Some(pattern) = process_pattern.filter(|pattern| !pattern.is_empty())
        && cache::process_is_running(pattern)
    {
        return Err(
            "the owning application appears to be running or its process check failed".into(),
        );
    }
    Ok(())
}

fn ensure_source_unchanged(plan: &RelocationPlan, copied_path: &Path) -> Result<(), String> {
    ensure_not_open(&plan.source)?;
    ensure_related_process_closed(plan.process_pattern.as_deref())?;
    let stats = verify_copy(&plan.source, copied_path).map_err(|error| {
        format!("copied data could not be verified against the source: {error}")
    })?;
    if stats.logical_bytes != plan.logical_bytes
        || stats.file_count != plan.file_count
        || stats.directory_count != plan.directory_count
    {
        return Err("the source changed while it was being copied; nothing was linked".into());
    }
    Ok(())
}

/// Compare a staging copy with the source recursively. Aggregate sizes alone
/// are not enough: two different files can have the same length. The source is
/// intentionally read again immediately before the rename so a same-sized
/// concurrent change is also detected in the normal case.
fn verify_copy(source: &Path, copied: &Path) -> io::Result<TreeStats> {
    let source_metadata = fs::symlink_metadata(source)?;
    let copied_metadata = fs::symlink_metadata(copied)?;
    if source_metadata.file_type().is_symlink() || copied_metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "symbolic links are not relocated",
        ));
    }
    if source_metadata.is_file() {
        if !copied_metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("copied entry has a different type: {}", copied.display()),
            ));
        }
        if source_metadata.len() != copied_metadata.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("copied file has a different length: {}", source.display()),
            ));
        }
        if !files_equal(source, copied)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("copied file contents differ: {}", source.display()),
            ));
        }
        return Ok(TreeStats {
            logical_bytes: source_metadata.len(),
            file_count: 1,
            directory_count: 0,
        });
    }
    if !source_metadata.is_dir() || !copied_metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("copied entry has a different type: {}", copied.display()),
        ));
    }

    let mut stats = TreeStats {
        logical_bytes: 0,
        file_count: 0,
        directory_count: 1,
    };
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_child = entry.path();
        let copied_child = copied.join(entry.file_name());
        stats += verify_copy(&source_child, &copied_child)?;
    }
    for entry in fs::read_dir(copied)? {
        let entry = entry?;
        if fs::symlink_metadata(source.join(entry.file_name())).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "copied directory contains an unexpected entry: {}",
                    entry.path().display()
                ),
            ));
        }
    }
    Ok(stats)
}

fn files_equal(left: &Path, right: &Path) -> io::Result<bool> {
    const BUFFER_SIZE: usize = 1024 * 1024;
    let mut left = fs::File::open(left)?;
    let mut right = fs::File::open(right)?;
    let mut left_buffer = vec![0_u8; BUFFER_SIZE];
    let mut right_buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        let left_read = left.read(&mut left_buffer)?;
        let right_read = right.read(&mut right_buffer)?;
        if left_read != right_read {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
        if left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
    }
}

fn verify_tree(path: &Path, uid: u32) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "symbolic links are not relocated",
        ));
    }
    if metadata.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is owned by another account", path.display()),
        ));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            verify_tree(&entry?.path(), uid)?;
        }
    } else if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported non-file entry: {}", path.display()),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TreeStats {
    logical_bytes: u64,
    file_count: u64,
    directory_count: u64,
}

fn tree_stats(path: &Path) -> io::Result<TreeStats> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("symbolic link is not relocatable: {}", path.display()),
        ));
    }
    if metadata.is_file() {
        return Ok(TreeStats {
            logical_bytes: metadata.len(),
            file_count: 1,
            directory_count: 0,
        });
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported filesystem entry: {}", path.display()),
        ));
    }
    let mut stats = TreeStats {
        logical_bytes: 0,
        file_count: 0,
        directory_count: 1,
    };
    for entry in fs::read_dir(path)? {
        stats += tree_stats(&entry?.path())?;
    }
    Ok(stats)
}

impl std::ops::AddAssign for TreeStats {
    fn add_assign(&mut self, rhs: Self) {
        self.logical_bytes = self.logical_bytes.saturating_add(rhs.logical_bytes);
        self.file_count = self.file_count.saturating_add(rhs.file_count);
        self.directory_count = self.directory_count.saturating_add(rhs.directory_count);
    }
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<TreeStats> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("source contains a symbolic link: {}", source.display()),
        ));
    }
    if metadata.is_file() {
        #[cfg(target_os = "macos")]
        copy_mac_metadata_and_data(source, destination, true)?;
        #[cfg(not(target_os = "macos"))]
        {
            fs::copy(source, destination)?;
            fs::set_permissions(destination, metadata.permissions())?;
        }
        return Ok(TreeStats {
            logical_bytes: metadata.len(),
            file_count: 1,
            directory_count: 0,
        });
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported source entry: {}", source.display()),
        ));
    }
    fs::create_dir(destination)?;
    let mut stats = TreeStats {
        logical_bytes: 0,
        file_count: 0,
        directory_count: 1,
    };
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        stats += copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
    }
    #[cfg(target_os = "macos")]
    copy_mac_metadata_and_data(source, destination, false)?;
    #[cfg(not(target_os = "macos"))]
    fs::set_permissions(destination, metadata.permissions())?;
    Ok(stats)
}

/// Apple's copyfile preserves resource forks, extended attributes, ACLs, and
/// timestamps as well as data. Plain byte copies would discard folder metadata.
#[cfg(target_os = "macos")]
fn copy_mac_metadata_and_data(source: &Path, destination: &Path, data: bool) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    unsafe extern "C" {
        fn copyfile(
            from: *const std::ffi::c_char,
            to: *const std::ffi::c_char,
            state: *mut std::ffi::c_void,
            flags: u32,
        ) -> std::ffi::c_int;
    }
    let from = CString::new(source.as_os_str().as_bytes())?;
    let to = CString::new(destination.as_os_str().as_bytes())?;
    // COPYFILE_ACL | STAT | XATTR; file copies add DATA | EXCL.
    let flags = 7 | (1 << 18) | (1 << 19) | if data { 8 | (1 << 17) } else { 0 };
    // SAFETY: both NUL-terminated paths live through this call; a null state
    // asks copyfile to manage its own state, and flags match the macOS SDK.
    if unsafe { copyfile(from.as_ptr(), to.as_ptr(), std::ptr::null_mut(), flags) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn rename_exclusive(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        unsafe extern "C" {
            fn renamex_np(
                from: *const std::ffi::c_char,
                to: *const std::ffi::c_char,
                flags: u32,
            ) -> std::ffi::c_int;
        }
        let from = CString::new(source.as_os_str().as_bytes())?;
        let to = CString::new(destination.as_os_str().as_bytes())?;
        // SAFETY: valid NUL-terminated paths; RENAME_EXCL atomically refuses
        // any existing destination, including an empty directory or symlink.
        if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), 4) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (source, destination);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Verified relocation requires macOS.",
        ))
    }
}

fn unique_sibling(path: &Path, role: &str) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("path has no parent directory: {}", path.display()))?;
    let name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("path has no usable name: {}", path.display()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))?
        .as_nanos();
    let candidate = parent.join(format!(
        "{}-{role}-{}-{}-{nonce}",
        crate::paths::RELOCATION_PREFIX,
        process::id(),
        name.to_string_lossy()
    ));
    if fs::symlink_metadata(&candidate).is_ok() {
        return Err(format!(
            "temporary relocation path already exists: {}",
            candidate.display()
        ));
    }
    Ok(candidate)
}

/// Last checks immediately before the source is renamed aside.
fn pre_link_checks(plan: &RelocationPlan) -> Result<(), String> {
    ensure_identities(plan)?;
    #[cfg(not(test))]
    if inspect_destination(&plan.destination_root, &plan.source)?.volume_uuid
        != plan.destination_volume_uuid
    {
        return Err(
            "The external volume changed during the copy; the original was not moved.".into(),
        );
    }
    ensure_not_open(&plan.source)?;
    ensure_related_process_closed(plan.process_pattern.as_deref())?;
    let stats = tree_stats(&plan.source)
        .map_err(|error| format!("the source could not be re-read before linking: {error}"))?;
    if stats.logical_bytes != plan.logical_bytes
        || stats.file_count != plan.file_count
        || stats.directory_count != plan.directory_count
    {
        return Err("the source changed after the copy was verified".into());
    }
    Ok(())
}

fn ensure_identities(plan: &RelocationPlan) -> Result<(), String> {
    if PathIdentity::capture(&plan.source) != Some(plan.source_identity)
        || PathIdentity::capture(&plan.destination_root) != Some(plan.destination_identity)
        || cache::has_symlink_component_below(&plan.source, Path::new("/"))
        || cache::has_symlink_component_below(&plan.destination_root, Path::new("/"))
    {
        return Err(
            "The source or external destination changed since review; nothing was moved.".into(),
        );
    }
    Ok(())
}

fn remove_owned_tree(path: &Path) {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            let _ = fs::remove_dir_all(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }
}

fn rollback_after_link_failure(
    source: &Path,
    backup: &Path,
    destination: &Path,
    reason: String,
) -> String {
    let mut rollback_errors = Vec::new();
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if let Err(error) = fs::remove_file(source) {
                rollback_errors.push(format!("could not remove the failed symlink: {error}"));
            }
        }
        Ok(_) => rollback_errors.push("the original source path was unexpectedly recreated".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => rollback_errors.push(format!(
            "could not inspect the source during rollback: {error}"
        )),
    }
    if rollback_errors.is_empty()
        && let Err(error) = rename_exclusive(backup, source)
    {
        rollback_errors.push(format!("could not restore the original source: {error}"));
    }
    if rollback_errors.is_empty() {
        // The original is back in place, so the copy is redundant.
        remove_owned_tree(destination);
        reason
    } else {
        // Never delete the verified copy unless the original was restored:
        // it may be the only intact version left.
        format!(
            "{reason}; rollback also failed: {}. Recovery paths: original backup at {}; verified copy at {}. Neither was removed during rollback.",
            rollback_errors.join("; "),
            backup.display(),
            destination.display()
        )
    }
}

fn require_absolute(path: &Path, label: &str) -> Result<(), String> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(format!("{label} path must be absolute: {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn test_plan(source: &Path, destination_root: &Path) -> RelocationPlan {
        let source_path = source.canonicalize().unwrap();
        let destination_path = destination_root.canonicalize().unwrap();
        let source = source_path.as_path();
        let destination_root = destination_path.as_path();
        let stats = tree_stats(source).unwrap();
        RelocationPlan {
            source: source.to_path_buf(),
            destination_root: destination_root.to_path_buf(),
            destination: destination_root.join(source.file_name().unwrap()),
            size_kb: cache::measure_target_kb(source, CacheTarget::ExactPath).unwrap(),
            logical_bytes: stats.logical_bytes,
            file_count: stats.file_count,
            directory_count: stats.directory_count,
            destination_free_kb: u64::MAX,
            required_destination_kb: required_destination_kb(0, stats.logical_bytes),
            source_identity: PathIdentity::capture(source).unwrap(),
            destination_identity: PathIdentity::capture(destination_root).unwrap(),
            destination_volume_uuid: "fixture".into(),
            account_home: source.parent().unwrap().to_path_buf(),
            process_pattern: None,
        }
    }

    fn populated_test_plan() -> (tempfile::TempDir, RelocationPlan) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        let destination_root = root.join("external");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination_root).unwrap();
        fs::write(source.join("data"), b"original").unwrap();
        let plan = test_plan(&source, &destination_root);
        (temp, plan)
    }

    fn assert_no_temporary_copies(plan: &RelocationPlan) {
        for directory in [plan.source.parent().unwrap(), &plan.destination_root] {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                assert!(
                    !entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(crate::paths::RELOCATION_PREFIX),
                    "temporary relocation data was left at {}",
                    entry.path().display()
                );
            }
        }
    }

    #[test]
    fn capacity_accounts_for_sparse_files_and_copy_overhead() {
        assert_eq!(
            required_destination_kb(8, 1024 * 1024 * 1024),
            1_048_576 + 65_536
        );
        assert_eq!(required_destination_kb(2_000_000, 10), 2_100_000);
        assert_eq!(required_destination_kb(u64::MAX, u64::MAX), u64::MAX);
    }

    const EXTERNAL_INFO: &str = r#"<plist><dict>
        <key>Internal</key><false/><key>WritableVolume</key><true/>
        <key>FilesystemType</key><string>apfs</string>
        <key>VolumeUUID</key><string>fixture-volume</string>
        <key>VolumeFreeSpace</key><integer>1048576</integer>
        <key>APFSContainerFree</key><integer>2097152</integer>
        <key>TotalSize</key><integer>8388608</integer>
        </dict></plist>"#;

    #[test]
    fn external_volume_policy_fails_closed_and_respects_volume_limits() {
        let path = PathBuf::from("/Volumes/Fixture");
        let info = destination_from_info(path.clone(), EXTERNAL_INFO).unwrap();
        assert_eq!(info.free_kb, 1024);
        assert_eq!(info.capacity_kb, 8192);
        let constrained_container = EXTERNAL_INFO.replace(
            "<key>APFSContainerFree</key><integer>2097152</integer>",
            "<key>APFSContainerFree</key><integer>524288</integer>",
        );
        assert_eq!(
            destination_from_info(path.clone(), &constrained_container)
                .unwrap()
                .free_kb,
            512
        );
        let hfs = EXTERNAL_INFO
            .replace("<string>apfs</string>", "<string>hfs</string>")
            .replace(
                "<key>VolumeFreeSpace</key><integer>1048576</integer>",
                "<key>FreeSpace</key><integer>1048576</integer>",
            )
            .replace("<key>APFSContainerFree</key><integer>2097152</integer>", "");
        assert_eq!(
            destination_from_info(path.clone(), &hfs).unwrap().free_kb,
            1024
        );
        for invalid in [
            EXTERNAL_INFO.replace("<key>Internal</key><false/>", "<key>Internal</key><true/>"),
            EXTERNAL_INFO.replace("<key>Internal</key><false/>", ""),
            EXTERNAL_INFO.replace(
                "<key>WritableVolume</key><true/>",
                "<key>WritableVolume</key><false/>",
            ),
            EXTERNAL_INFO.replace("<key>WritableVolume</key><true/>", ""),
            EXTERNAL_INFO.replace(
                "<dict>",
                "<dict><key>BusProtocol</key><string>Disk Image</string>",
            ),
            EXTERNAL_INFO.replace("<string>apfs</string>", "<string>exfat</string>"),
            EXTERNAL_INFO.replace("<key>VolumeUUID</key><string>fixture-volume</string>", ""),
            EXTERNAL_INFO.replace("<string>fixture-volume</string>", "<string></string>"),
            EXTERNAL_INFO.replace("<key>TotalSize</key><integer>8388608</integer>", ""),
            EXTERNAL_INFO
                .replace("<key>VolumeFreeSpace</key><integer>1048576</integer>", "")
                .replace("<key>APFSContainerFree</key><integer>2097152</integer>", ""),
        ] {
            assert!(destination_from_info(path.clone(), &invalid).is_err());
        }
        assert!(destination_from_info(path, "").is_err());
    }

    #[test]
    fn replaced_source_or_destination_invalidates_review() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("external");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(source.join("data"), "keep this").unwrap();
        let plan = test_plan(&source, &destination);
        assert!(ensure_identities(&plan).is_ok());
        fs::rename(&destination, temp.path().join("old-external")).unwrap();
        fs::create_dir(&destination).unwrap();
        assert!(ensure_identities(&plan).is_err());
        let plan = test_plan(&source, &destination);
        fs::rename(&source, temp.path().join("old-source")).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(source.join("data"), "same size").unwrap();
        assert!(execute_inner(&plan).is_err());
        assert_eq!(
            fs::read(temp.path().join("old-source/data")).unwrap(),
            b"keep this"
        );
    }

    #[test]
    fn commit_never_overwrites_an_existing_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(source.join("data"), "keep").unwrap();
        assert!(rename_exclusive(&source, &destination).is_err());
        assert_eq!(fs::read(source.join("data")).unwrap(), b"keep");
        assert!(destination.is_dir());
    }

    #[test]
    fn source_verification_detects_same_size_edits_after_the_first_read_buffer() {
        let (_temp, fixture) = populated_test_plan();
        let source = fixture.source.join("data");
        let copied = fixture.destination.join("data");
        let mut contents = vec![7_u8; 1024 * 1024 + 17];
        fs::write(&source, &contents).unwrap();
        let plan = test_plan(&fixture.source, &fixture.destination_root);
        copy_tree(&plan.source, &plan.destination).unwrap();
        ensure_source_unchanged(&plan, &plan.destination).unwrap();

        *contents.last_mut().unwrap() = 8;
        fs::write(&source, &contents).unwrap();
        assert_eq!(tree_stats(&source).unwrap(), tree_stats(&copied).unwrap());
        let error = ensure_source_unchanged(&plan, &plan.destination).unwrap_err();
        assert!(error.contains("could not be verified against the source"));
        assert!(error.contains("contents differ"));
        assert_eq!(fs::read(&source).unwrap().last(), Some(&8));
        assert_eq!(fs::read(&copied).unwrap().last(), Some(&7));
    }

    #[test]
    fn verification_rejects_missing_extra_and_type_changed_nested_entries() {
        for change in ["missing", "extra", "type"] {
            let (_temp, plan) = populated_test_plan();
            let nested = plan.source.join("nested");
            fs::create_dir(&nested).unwrap();
            fs::write(nested.join("data"), b"nested contents").unwrap();
            copy_tree(&plan.source, &plan.destination).unwrap();

            let copied_nested = plan.destination.join("nested");
            match change {
                "missing" => fs::remove_file(copied_nested.join("data")).unwrap(),
                "extra" => fs::write(copied_nested.join("unexpected"), b"extra").unwrap(),
                "type" => {
                    fs::remove_file(copied_nested.join("data")).unwrap();
                    fs::create_dir(copied_nested.join("data")).unwrap();
                }
                _ => unreachable!(),
            }

            let error = verify_copy(&plan.source, &plan.destination).unwrap_err();
            assert!(
                matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                ),
                "{change}: {error}"
            );
            assert_eq!(fs::read(nested.join("data")).unwrap(), b"nested contents");
        }
    }

    #[test]
    fn verification_rejects_symlinks_on_either_side_even_when_contents_match() {
        for replace_source in [false, true] {
            let (_temp, plan) = populated_test_plan();
            copy_tree(&plan.source, &plan.destination).unwrap();
            let (replace, target) = if replace_source {
                (plan.source.join("data"), plan.destination.join("data"))
            } else {
                (plan.destination.join("data"), plan.source.join("data"))
            };
            fs::remove_file(&replace).unwrap();
            symlink(&target, &replace).unwrap();

            let error = verify_copy(&plan.source, &plan.destination).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(fs::read(&target).unwrap(), b"original");
            assert_eq!(fs::read_link(&replace).unwrap(), target);
        }
    }

    #[test]
    fn source_validation_rejects_nested_symlinks_without_following_them() {
        let (temp, plan) = populated_test_plan();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), b"outside data").unwrap();
        symlink(&outside, plan.source.join("nested-link")).unwrap();

        let error = validate_source(&plan.source, &plan.account_home).unwrap_err();
        assert!(error.contains("symbolic links"), "{error}");
        assert_eq!(fs::read(outside.join("keep")).unwrap(), b"outside data");
        assert_eq!(fs::read(plan.source.join("data")).unwrap(), b"original");
    }

    #[test]
    fn ancestor_symlink_swaps_invalidate_review_even_if_leaf_identities_match() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source_branch = root.join("source-branch");
        let destination_branch = root.join("destination-branch");
        let source = source_branch.join("project/source");
        let destination_root = destination_branch.join("volume/external");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination_root).unwrap();
        fs::write(source.join("data"), b"original").unwrap();
        let plan = test_plan(&source, &destination_root);

        for branch in [&source_branch, &destination_branch] {
            let original = branch.with_extension("original");
            fs::rename(branch, &original).unwrap();
            symlink(&original, branch).unwrap();
            assert_eq!(
                PathIdentity::capture(&plan.source),
                Some(plan.source_identity)
            );
            assert_eq!(
                PathIdentity::capture(&plan.destination_root),
                Some(plan.destination_identity)
            );

            let report = execute(&plan);
            assert_eq!(report.status, RelocationStatus::Failed);
            assert!(!report.backup_removed);
            assert!(report.message.unwrap().contains("changed since review"));
            assert_eq!(fs::read(source.join("data")).unwrap(), b"original");
            assert!(!plan.destination.exists());
            assert_no_temporary_copies(&plan);

            fs::remove_file(branch).unwrap();
            fs::rename(&original, branch).unwrap();
        }
    }

    #[test]
    fn changed_tree_after_revalidation_aborts_and_removes_the_staging_copy() {
        let (_temp, plan) = populated_test_plan();
        fs::write(plan.source.join("new-data"), b"new contents").unwrap();

        let error = execute_inner_for_test(&plan).err().unwrap();
        assert!(error.contains("did not match the source"), "{error}");
        assert_eq!(fs::read(plan.source.join("data")).unwrap(), b"original");
        assert_eq!(
            fs::read(plan.source.join("new-data")).unwrap(),
            b"new contents"
        );
        assert!(!plan.destination.exists());
        assert_no_temporary_copies(&plan);
    }

    #[test]
    fn symlink_introduced_after_review_aborts_copy_and_preserves_its_target() {
        let (temp, plan) = populated_test_plan();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), b"outside data").unwrap();
        let link = plan.source.join("new-link");
        symlink(&outside, &link).unwrap();

        let error = execute_inner_for_test(&plan).err().unwrap();
        assert!(error.contains("symbolic link"), "{error}");
        assert_eq!(fs::read(plan.source.join("data")).unwrap(), b"original");
        assert_eq!(fs::read(outside.join("keep")).unwrap(), b"outside data");
        assert_eq!(fs::read_link(&link).unwrap(), outside);
        assert!(!plan.destination.exists());
        assert_no_temporary_copies(&plan);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn conflicting_destination_never_overwrites_files_directories_or_dangling_links() {
        for conflict in ["file", "directory", "symlink"] {
            let (temp, plan) = populated_test_plan();
            let missing_target = temp.path().join("missing");
            match conflict {
                "file" => fs::write(&plan.destination, b"destination data").unwrap(),
                "directory" => fs::create_dir(&plan.destination).unwrap(),
                "symlink" => symlink(&missing_target, &plan.destination).unwrap(),
                _ => unreachable!(),
            }
            let before = PathIdentity::capture(&plan.destination).unwrap();

            let error = execute_inner_for_test(&plan).err().unwrap();
            assert!(
                error.contains("could not commit the copy"),
                "{conflict}: {error}"
            );
            assert_eq!(PathIdentity::capture(&plan.destination), Some(before));
            assert_eq!(fs::read(plan.source.join("data")).unwrap(), b"original");
            match conflict {
                "file" => assert_eq!(fs::read(&plan.destination).unwrap(), b"destination data"),
                "directory" => assert_eq!(fs::read_dir(&plan.destination).unwrap().count(), 0),
                "symlink" => {
                    assert_eq!(fs::read_link(&plan.destination).unwrap(), missing_target);
                    assert!(!missing_target.exists());
                }
                _ => unreachable!(),
            }
            assert_no_temporary_copies(&plan);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_copy_preserves_file_and_folder_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let copied = temp.path().join("copied");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("data"), "contents").unwrap();
        let modified = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        for path in [&source, &source.join("data")] {
            let status = std::process::Command::new("/usr/bin/xattr")
                .args(["-w", "com.diskray.fixture", "preserve-me"])
                .arg(path)
                .status()
                .unwrap();
            assert!(status.success());
            fs::File::open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(modified))
                .unwrap();
        }
        copy_tree(&source, &copied).unwrap();
        verify_copy(&source, &copied).unwrap();
        for path in [&copied, &copied.join("data")] {
            let output = std::process::Command::new("/usr/bin/xattr")
                .args(["-p", "com.diskray.fixture"])
                .arg(path)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout).unwrap().trim(),
                "preserve-me"
            );
            assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
        }
        fs::write(copied.join("data"), "tampered").unwrap();
        assert!(verify_copy(&source, &copied).is_err());
        assert_eq!(fs::read(source.join("data")).unwrap(), b"contents");
    }

    #[test]
    fn failed_rollback_keeps_both_copies_and_names_the_backup() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let backup = temp.path().join(".backup");
        let destination = temp.path().join("destination");
        fs::create_dir(&backup).unwrap();
        fs::write(backup.join("data"), b"original").unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("data"), b"original").unwrap();
        // An app recreated the source path, so the original cannot be restored.
        fs::create_dir(&source).unwrap();

        let message =
            rollback_after_link_failure(&source, &backup, &destination, "link failed".into());

        assert!(backup.join("data").exists(), "the original is kept");
        assert!(
            destination.join("data").exists(),
            "the verified copy is kept"
        );
        assert!(message.contains(&backup.display().to_string()));
        assert!(message.contains(&destination.display().to_string()));
    }

    #[test]
    fn successful_rollback_restores_the_original_and_drops_the_copy() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let backup = temp.path().join(".backup");
        let destination = temp.path().join("destination");
        fs::create_dir(&backup).unwrap();
        fs::write(backup.join("data"), b"original").unwrap();
        fs::create_dir(&destination).unwrap();

        let message =
            rollback_after_link_failure(&source, &backup, &destination, "link failed".into());

        assert_eq!(message, "link failed");
        assert!(source.join("data").exists());
        assert!(!destination.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rollback_removes_a_replacement_link_without_touching_its_target() {
        let (temp, plan) = populated_test_plan();
        copy_tree(&plan.source, &plan.destination).unwrap();
        let backup = temp.path().join("backup");
        let unrelated = temp.path().join("unrelated");
        fs::create_dir(&unrelated).unwrap();
        fs::write(unrelated.join("keep"), b"unrelated data").unwrap();
        fs::rename(&plan.source, &backup).unwrap();
        symlink(&unrelated, &plan.source).unwrap();

        let message = rollback_after_link_failure(
            &plan.source,
            &backup,
            &plan.destination,
            "wrong replacement link".into(),
        );
        assert_eq!(message, "wrong replacement link");
        assert!(
            !fs::symlink_metadata(&plan.source)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(plan.source.join("data")).unwrap(), b"original");
        assert_eq!(fs::read(unrelated.join("keep")).unwrap(), b"unrelated data");
        assert!(!backup.exists());
        assert!(!plan.destination.exists());
    }

    #[test]
    fn rollback_retains_the_verified_copy_when_the_original_cannot_be_restored() {
        let (temp, plan) = populated_test_plan();
        copy_tree(&plan.source, &plan.destination).unwrap();
        let backup = temp.path().join("missing-backup");
        fs::remove_dir_all(&plan.source).unwrap();

        let message = rollback_after_link_failure(
            &plan.source,
            &backup,
            &plan.destination,
            "link creation failed".into(),
        );
        assert!(message.contains("could not restore the original source"));
        assert!(message.contains(&plan.destination.display().to_string()));
        assert!(
            !message.contains("Your data is kept in two places"),
            "a missing original backup must not be reported as intact: {message}"
        );
        assert_eq!(
            fs::read(plan.destination.join("data")).unwrap(),
            b"original"
        );
        assert!(!plan.source.exists());
    }

    #[test]
    fn pre_link_checks_refuse_a_source_with_an_open_file() {
        let temp = tempfile::tempdir().unwrap();
        let destination_root = temp.path().join("external");
        let source = temp.path().join("source");
        fs::create_dir(&destination_root).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(source.join("data"), b"content").unwrap();
        let plan = test_plan(&source, &destination_root);
        assert!(pre_link_checks(&plan).is_ok());
        let _held = fs::File::open(source.join("data")).unwrap();
        assert!(pre_link_checks(&plan).is_err());
    }

    #[test]
    fn public_plan_rejects_non_external_destinations() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        assert!(plan(&source, temp.path(), temp.path(), None).is_err());
    }

    #[test]
    fn relocation_copies_data_and_replaces_source_with_a_link() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let source = temp_root.join("source");
        let destination_root = temp_root.join("external");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination_root).unwrap();
        fs::create_dir(source.join("nested")).unwrap();
        fs::File::create(source.join("nested/data.bin"))
            .unwrap()
            .write_all(&[7_u8; 8_192])
            .unwrap();

        let plan = test_plan(&source, &destination_root);
        let details = execute_inner_for_test(&plan).unwrap();
        let report = report_from_plan(
            &plan,
            RelocationStatus::Relocated,
            details.backup_removed,
            details.message,
        );

        assert!(report.succeeded(), "{report:?}");
        assert!(report.backup_removed);
        assert!(
            fs::symlink_metadata(&source)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_link(&source).unwrap(), plan.destination);
        assert_eq!(
            fs::read(source.join("nested/data.bin")).unwrap(),
            vec![7_u8; 8_192]
        );
    }
}
