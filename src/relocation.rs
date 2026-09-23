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
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

use crate::cache::{self, CacheTarget, ScanLocationKind};

const VOLUMES_ROOT: &str = "/Volumes";
const TEMP_ROOT: &str = "/private/tmp";

#[derive(Debug, Clone, Serialize)]
pub struct RelocationPlan {
    pub source: PathBuf,
    pub destination_root: PathBuf,
    pub destination: PathBuf,
    pub size_kb: u64,
    pub logical_bytes: u64,
    pub file_count: u64,
    pub directory_count: u64,
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
    let destination_root = validate_destination_root(destination_root, &source)?;
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
    Ok(RelocationPlan {
        source,
        destination_root,
        destination,
        size_kb,
        logical_bytes: stats.logical_bytes,
        file_count: stats.file_count,
        directory_count: stats.directory_count,
        account_home,
        process_pattern: process_pattern
            .filter(|pattern| !pattern.is_empty())
            .map(str::to_owned),
    })
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
    let current = plan(
        &prepared.source,
        &prepared.destination_root,
        &prepared.account_home,
        prepared.process_pattern.as_deref(),
    )?;
    if current.destination != prepared.destination
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
    fs::rename(&staging, &prepared.destination).map_err(|error| {
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
    if let Err(error) = fs::rename(&prepared.source, &backup) {
        remove_owned_tree(&prepared.destination);
        return Err(format!(
            "could not stage the original source for linking: {error}"
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
    if cache::scan_location_kind(&volume_root) == ScanLocationKind::Network {
        return Err(format!(
            "network volumes are not supported as relocation destinations: {}",
            volume_root.display()
        ));
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
        fs::copy(source, destination)?;
        fs::set_permissions(destination, metadata.permissions())?;
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
    fs::set_permissions(destination, metadata.permissions())?;
    Ok(stats)
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
        && let Err(error) = fs::rename(backup, source)
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
            "{reason}; rollback also failed: {}. Your data is kept in two places: the original at {} and the verified copy at {}.",
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
        let stats = tree_stats(source).unwrap();
        RelocationPlan {
            source: source.to_path_buf(),
            destination_root: destination_root.to_path_buf(),
            destination: destination_root.join(source.file_name().unwrap()),
            size_kb: cache::measure_target_kb(source, CacheTarget::ExactPath).unwrap(),
            logical_bytes: stats.logical_bytes,
            file_count: stats.file_count,
            directory_count: stats.directory_count,
            account_home: source.parent().unwrap().to_path_buf(),
            process_pattern: None,
        }
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
