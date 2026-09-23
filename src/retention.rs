// SPDX-License-Identifier: GPL-3.0-or-later
//! Conservative retention policy for user-owned temporary entries.
//!
//! The system `/private/tmp` directory is shared by macOS services and user
//! processes. The policy therefore only considers direct children owned by the
//! current account, older than the configured age, fully owned by that account,
//! and not open by a process. Eligible entries remain exact-path candidates so
//! cleanup can re-check those conditions immediately before removal.

use std::{
    collections::HashSet,
    env, fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

use crate::cache::{self, CacheSpec, CacheTarget, CacheTier, effective_user_id, measure_target_kb};

pub const DEFAULT_RETENTION_DAYS: u64 = 7;

const TEMP_ROOT: &str = "/private/tmp";
const MAX_ERROR_PATHS: usize = 20;

#[derive(Debug, Clone, Serialize)]
pub struct TempRetentionScan {
    pub enabled: bool,
    pub root: Option<PathBuf>,
    pub retention_days: u64,
    pub candidates: Vec<TempRetentionCandidate>,
    pub skipped_recent: u64,
    pub skipped_not_user_owned: u64,
    pub skipped_open: u64,
    pub skipped_symlink: u64,
    pub scan_errors: u64,
    pub scan_error_paths: Vec<PathBuf>,
    pub open_check_error: Option<String>,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TempRetentionCandidate {
    pub path: PathBuf,
    pub size_kb: u64,
    pub age_days: u64,
    pub modified_unix: u64,
    pub kind: TempRetentionKind,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TempRetentionKind {
    File,
    Directory,
}

impl TempRetentionScan {
    /// Create an empty report before a storage scan has been requested.
    ///
    /// Startup screens should never walk `/private/tmp`; discovery begins when
    /// the user explicitly starts a storage scan.
    pub(crate) fn pending(retention_days: u64) -> Self {
        let mut report = Self::disabled(retention_days);
        report.complete = false;
        report
    }

    /// Discover retention candidates for an application scan.
    ///
    /// A synthetic home directory is commonly used by library callers and
    /// tests while the default location picker still points at `/`. In that
    /// case, do not accidentally inspect the host account's real temporary
    /// directory.
    pub fn discover_for_scan(scan_root: &Path, account_home: &Path, retention_days: u64) -> Self {
        if scan_root == Path::new("/")
            && env::var_os("HOME")
                .map(PathBuf::from)
                .is_some_and(|home| !same_path(&home, account_home))
        {
            return Self::disabled(retention_days);
        }
        Self::discover(scan_root, retention_days)
    }

    pub fn discover(scan_root: &Path, retention_days: u64) -> Self {
        let Some(root) = retention_root(scan_root) else {
            return Self::disabled(retention_days);
        };

        let uid = match effective_user_id() {
            Some(uid) => uid,
            None => {
                return Self::unavailable(
                    root,
                    retention_days,
                    "could not determine the effective user ID safely",
                );
            }
        };
        let open_paths = match cache::open_paths_under(&root) {
            Ok(paths) => paths,
            Err(error) => {
                return Self::unavailable(
                    root,
                    retention_days,
                    format!("could not verify open temporary paths: {error}"),
                );
            }
        };

        Self::scan_entries(root, retention_days, uid, SystemTime::now(), &open_paths)
    }

    fn scan_entries(
        root: PathBuf,
        retention_days: u64,
        uid: u32,
        now: SystemTime,
        open_paths: &HashSet<PathBuf>,
    ) -> Self {
        let cutoff = now
            .checked_sub(Duration::from_secs(
                retention_days.saturating_mul(24 * 60 * 60),
            ))
            .unwrap_or(UNIX_EPOCH);
        let mut report = Self {
            enabled: true,
            root: Some(root.clone()),
            retention_days,
            candidates: Vec::new(),
            skipped_recent: 0,
            skipped_not_user_owned: 0,
            skipped_open: 0,
            skipped_symlink: 0,
            scan_errors: 0,
            scan_error_paths: Vec::new(),
            open_check_error: None,
            complete: true,
        };

        let reader = match fs::read_dir(&root) {
            Ok(reader) => reader,
            Err(error) => {
                report.record_error(&root, error);
                return report.finish();
            }
        };

        for entry in reader {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    report.record_error(&root, error);
                    continue;
                }
            };
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    report.record_error(&path, error);
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                report.skipped_symlink += 1;
                continue;
            }
            if metadata.uid() != uid {
                report.skipped_not_user_owned += 1;
                continue;
            }
            let modified = match metadata.modified() {
                Ok(modified) => modified,
                Err(error) => {
                    report.record_error(&path, error);
                    continue;
                }
            };
            if modified > cutoff {
                report.skipped_recent += 1;
                continue;
            }
            if is_open(&path, open_paths) {
                report.skipped_open += 1;
                continue;
            }
            if let Err(error) = verify_tree_ownership(&path, uid) {
                if error.kind() == io::ErrorKind::PermissionDenied {
                    report.skipped_not_user_owned += 1;
                } else {
                    report.record_error(&path, error);
                }
                continue;
            }
            let size_kb = match measure_target_kb(&path, CacheTarget::ExactPath) {
                Ok(size_kb) => size_kb,
                Err(error) => {
                    report.record_error(&path, error);
                    continue;
                }
            };
            if size_kb == 0 {
                continue;
            }
            let age_days = now
                .duration_since(modified)
                .map(|age| age.as_secs() / (24 * 60 * 60))
                .unwrap_or_default();
            report.candidates.push(TempRetentionCandidate {
                path,
                size_kb,
                age_days,
                modified_unix: modified
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                kind: if metadata.is_dir() {
                    TempRetentionKind::Directory
                } else {
                    TempRetentionKind::File
                },
            });
        }

        report.candidates.sort_by(|left, right| {
            right
                .size_kb
                .cmp(&left.size_kb)
                .then_with(|| left.path.cmp(&right.path))
        });
        report.finish()
    }

    pub fn cache_specs(&self) -> Vec<CacheSpec> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        self.candidates
            .iter()
            .map(|candidate| CacheSpec {
                home: root.clone(),
                path: candidate.path.clone(),
                label: "Stale temporary data",
                tier: CacheTier::Routine,
                process_pattern: "",
                note: "user-owned temporary entry past retention and not currently open",
                target: CacheTarget::ExactPath,
            })
            .collect()
    }

    pub fn candidate_kb(&self) -> u64 {
        self.candidates
            .iter()
            .map(|candidate| candidate.size_kb)
            .sum()
    }

    fn disabled(retention_days: u64) -> Self {
        Self {
            enabled: false,
            root: None,
            retention_days,
            candidates: Vec::new(),
            skipped_recent: 0,
            skipped_not_user_owned: 0,
            skipped_open: 0,
            skipped_symlink: 0,
            scan_errors: 0,
            scan_error_paths: Vec::new(),
            open_check_error: None,
            complete: true,
        }
    }

    fn unavailable(root: PathBuf, retention_days: u64, error: impl Into<String>) -> Self {
        let error = error.into();
        Self {
            enabled: true,
            root: Some(root.clone()),
            retention_days,
            candidates: Vec::new(),
            skipped_recent: 0,
            skipped_not_user_owned: 0,
            skipped_open: 0,
            skipped_symlink: 0,
            scan_errors: 1,
            scan_error_paths: vec![root],
            open_check_error: Some(error),
            complete: false,
        }
    }

    fn record_error(&mut self, path: &Path, _error: io::Error) {
        self.scan_errors += 1;
        self.complete = false;
        if self.scan_error_paths.len() < MAX_ERROR_PATHS {
            self.scan_error_paths.push(path.to_path_buf());
        }
    }

    fn finish(mut self) -> Self {
        self.complete = self.complete && self.open_check_error.is_none();
        self
    }
}

fn retention_root(scan_root: &Path) -> Option<PathBuf> {
    if scan_root == Path::new("/") || scan_root == Path::new(TEMP_ROOT) {
        Some(PathBuf::from(TEMP_ROOT))
    } else {
        None
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn is_open(path: &Path, open_paths: &HashSet<PathBuf>) -> bool {
    open_paths
        .iter()
        .any(|open_path| open_path == path || open_path.starts_with(path))
}

fn verify_tree_ownership(path: &Path, uid: u32) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "temporary tree contains an entry owned by another account",
        ));
    }
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        verify_tree_ownership(&entry.path(), uid)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::whitelist::Whitelist;
    use std::io::Write;

    #[test]
    fn retention_filters_recent_open_and_symlink_entries() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("tmp");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("stale.bin"), [1_u8; 16_384]).unwrap();
        fs::write(root.join("open.bin"), [2_u8; 8_192]).unwrap();
        fs::write(root.join("recent.bin"), [3_u8; 4_096]).unwrap();
        fs::File::open(root.join("recent.bin"))
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(3 * 24 * 60 * 60))
            .unwrap();
        fs::create_dir(root.join("stale-dir")).unwrap();
        fs::write(root.join("stale-dir/data"), [4_u8; 8_192]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("stale.bin"), root.join("link")).unwrap();

        let uid = effective_user_id().unwrap();
        let now = SystemTime::now();
        let open_paths = HashSet::from([root.join("open.bin")]);
        let report = TempRetentionScan::scan_entries(
            root.clone(),
            1,
            uid,
            now + Duration::from_secs(2 * 24 * 60 * 60),
            &open_paths,
        );

        assert!(report.complete);
        assert!(report.candidates.iter().any(|candidate| {
            candidate.path.ends_with("stale.bin") && candidate.kind == TempRetentionKind::File
        }));
        assert!(report.candidates.iter().any(|candidate| {
            candidate.path.ends_with("stale-dir") && candidate.kind == TempRetentionKind::Directory
        }));
        assert_eq!(report.skipped_open, 1);
        assert_eq!(report.skipped_recent, 1);
        assert_eq!(report.skipped_symlink, 1);
    }

    #[test]
    fn retention_specs_remove_exact_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("tmp");
        fs::create_dir(&root).unwrap();
        let file = root.join("stale.bin");
        fs::File::create(&file)
            .unwrap()
            .write_all(&[1_u8; 8_192])
            .unwrap();
        let uid = effective_user_id().unwrap();
        let report = TempRetentionScan::scan_entries(
            root.clone(),
            1,
            uid,
            SystemTime::now() + Duration::from_secs(24 * 60 * 60),
            &HashSet::new(),
        );
        let specs = report.cache_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].target, CacheTarget::ExactPath);
        assert_eq!(specs[0].home, root);
    }

    #[test]
    fn retention_cleanup_removes_an_exact_stale_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("tmp");
        fs::create_dir(&root).unwrap();
        let file = root.join("stale.bin");
        fs::write(&file, [1_u8; 8_192]).unwrap();
        let uid = effective_user_id().unwrap();
        let report = TempRetentionScan::scan_entries(
            root.clone(),
            1,
            uid,
            SystemTime::now() + Duration::from_secs(2 * 24 * 60 * 60),
            &HashSet::new(),
        );
        let spec = report.cache_specs().into_iter().next().unwrap();
        let mut entry = cache::scan_cache(&spec, false);

        let outcome = cache::clean_cache(
            &mut entry,
            std::slice::from_ref(&file),
            false,
            &Whitelist::empty(),
        );

        assert!(matches!(outcome, cache::CleanupOutcome::Cleared { .. }));
        assert!(!file.exists());
    }
}
