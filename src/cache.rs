// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::HashSet,
    env,
    ffi::OsString,
    fs, io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::whitelist::Whitelist;

const DF_COMMAND: &str = "/bin/df";
const DSCL_COMMAND: &str = "/usr/bin/dscl";
const DU_COMMAND: &str = "/usr/bin/du";
const ID_COMMAND: &str = "/usr/bin/id";
const PGREP_COMMAND: &str = "/usr/bin/pgrep";
const LSOF_COMMAND: &str = "/usr/sbin/lsof";
const DISKUTIL_COMMAND: &str = "/usr/sbin/diskutil";
const MOUNT_COMMAND: &str = "/sbin/mount";
const NATIVE_COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTier {
    Routine,
    Reinstallable,
    /// Large app-managed data that is useful for diagnosis but must never be
    /// deleted as though it were a cache.
    ReviewOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTarget {
    /// Keep the allowlisted directory and clear only its contents.
    DirectoryContents,
    /// Keep the allowlisted directory and clear only its direct contents
    /// whose modification time is at least this many days old.
    AgedContents { min_age_days: u64 },
    /// Remove the exact allowlisted file or directory itself.
    ExactPath,
}

/// Filesystem identity of a cleanup candidate captured while scanning.
///
/// Deleting re-verifies the exact target and its parent directory still have
/// the device and inode numbers observed during the scan, so a path that was
/// removed and recreated (or swapped) after the scan is never cleaned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathIdentity {
    pub dev: u64,
    pub ino: u64,
    pub parent_dev: u64,
    pub parent_ino: u64,
}

impl PathIdentity {
    pub fn capture(path: &Path) -> Option<Self> {
        let metadata = fs::symlink_metadata(path).ok()?;
        let parent = path.parent()?;
        let parent_metadata = fs::symlink_metadata(parent).ok()?;
        Some(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            parent_dev: parent_metadata.dev(),
            parent_ino: parent_metadata.ino(),
        })
    }

    fn still_matches(&self, path: &Path) -> bool {
        Self::capture(path).is_some_and(|current| current == *self)
    }
}

#[derive(Debug, Clone)]
pub struct CacheSpec {
    /// Safety boundary that must contain this exact cleanup candidate.
    pub home: PathBuf,
    pub path: PathBuf,
    pub label: &'static str,
    pub tier: CacheTier,
    pub process_pattern: &'static str,
    pub note: &'static str,
    pub target: CacheTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    Ready,
    Optional,
    Review,
    InUse,
    ScanError,
    Symlink,
    Invalid,
    Missing,
    Whitelisted,
}

impl CacheStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "READY",
            Self::Optional => "OPTIONAL",
            Self::Review => "REVIEW",
            Self::InUse => "IN USE",
            Self::ScanError => "SCAN ERROR",
            Self::Symlink => "SYMLINK",
            Self::Invalid => "INVALID",
            Self::Missing => "MISSING",
            Self::Whitelisted => "PROTECTED",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Self::Ready => "All current safeguards passed.",
            Self::Optional => {
                "Reinstallable cache; explicitly opt in because restoring it may require a large download."
            }
            Self::Review => {
                "App-managed or personal data; protected from ordinary cleanup. In clean mode, use the advanced single-item review deletion only if you accept losing this data."
            }
            Self::InUse => "A related application or package manager is running.",
            Self::ScanError => {
                "The directory or a required safety check could not be read completely. It will not be cleaned. macOS-protected user data may require Full Disk Access for the terminal."
            }
            Self::Symlink => "The cache path redirects elsewhere and will not be touched.",
            Self::Invalid => "The allowlisted path is not a directory.",
            Self::Missing => "No cache directory exists at this location.",
            Self::Whitelisted => {
                "Matches the user whitelist (~/.config/diskray/whitelist); it is never cleaned."
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub spec: CacheSpec,
    pub status: CacheStatus,
    pub size_kb: u64,
    pub outcome: Option<CleanupOutcome>,
    pub identity: Option<PathIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOutcome {
    Cleared {
        removed_kb: u64,
        method: CleanupMethod,
    },
    SafetySkipped(String),
    Failed {
        error: String,
        removed_kb: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupMethod {
    ExactPath,
    /// Exact-path cleanup that left whitelisted or still-open items in place.
    ExactPathKeeping {
        kept: usize,
    },
    NativeUnavailableThenExactPath {
        command: &'static str,
    },
    /// The native command was not run because a whitelisted path is inside.
    NativeSkippedForWhitelist {
        command: &'static str,
        kept: usize,
    },
    Native {
        command: &'static str,
    },
}

impl CleanupMethod {
    pub fn explanation(&self) -> String {
        match self {
            Self::ExactPath => "exact-path filesystem cleanup (no safe scoped command)".into(),
            Self::ExactPathKeeping { kept } => format!(
                "exact-path filesystem cleanup; kept {kept} whitelisted or still-open item(s)"
            ),
            Self::NativeSkippedForWhitelist { command, kept } => format!(
                "exact-path cleanup; `{command}` was skipped because a whitelisted path is inside, and {kept} protected item(s) were kept"
            ),
            Self::NativeUnavailableThenExactPath { command } => {
                format!("exact-path cleanup (`{command}` was not available for this account)")
            }
            Self::Native { command } => format!("native command: {command}"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CleanupStats {
    pub measured_removed_kb: u64,
    pub filesystem_change_kb: u64,
    pub cleared: usize,
    pub safety_skipped: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanLocation {
    pub label: String,
    pub path: PathBuf,
    pub kind: ScanLocationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScanLocationKind {
    Local,
    Usb,
    Network,
}

impl ScanLocationKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "LOCAL",
            Self::Usb => "USB",
            Self::Network => "NETWORK",
        }
    }
}

/// Every active rule under an account home. Rules come from the rule packs
/// in `rules/` plus any user packs; see `crate::rules`.
pub fn cache_specs(root: &Path) -> Vec<CacheSpec> {
    crate::rules::home_specs(root)
}

/// Build useful candidates for a selected scan location.
///
/// A home-directory scan checks user caches and Trash. A mounted-volume scan
/// checks volume-level recycle and temporary directories instead of pretending
/// that the volume root is a user home. Startup and backup system volumes also
/// include the current user's home layout when it can be resolved safely.
pub fn scan_specs(scan_root: &Path, account_home: &Path) -> Vec<CacheSpec> {
    if paths_match(scan_root, account_home) {
        return cache_specs(account_home);
    }
    if looks_like_home(scan_root) {
        return cache_specs(scan_root);
    }

    let mut specs = volume_specs(scan_root);
    let home_on_volume = if scan_root == Path::new("/") {
        Some(account_home.to_path_buf())
    } else {
        account_home.file_name().and_then(|account_name| {
            let candidate = scan_root.join("Users").join(account_name);
            candidate.is_dir().then_some(candidate)
        })
    };

    if let Some(home) = home_on_volume {
        specs.extend(cache_specs(&home));
    }
    specs
}

fn volume_specs(root: &Path) -> Vec<CacheSpec> {
    volume_specs_for(root, scan_location_kind(root))
}

/// Volume-level waste. Server- and Windows-managed recycle bins can hold other
/// people's deleted files, so they are always review-only. On a network share
/// nothing here is routine: the share may be used by other accounts and
/// machines that this Mac cannot see.
fn volume_specs_for(root: &Path, kind: ScanLocationKind) -> Vec<CacheSpec> {
    let shared_tier = if kind == ScanLocationKind::Network {
        CacheTier::ReviewOnly
    } else {
        CacheTier::Routine
    };
    let definitions = [
        (
            ".Trash",
            "Volume Trash",
            "files already moved to this volume's Trash",
            shared_tier,
        ),
        (
            "#recycle",
            "NAS recycle bin",
            "server-managed deleted files that may belong to other users; empty it from the NAS instead",
            CacheTier::ReviewOnly,
        ),
        (
            "@Recycle",
            "NAS recycle bin",
            "server-managed deleted files that may belong to other users; empty it from the NAS instead",
            CacheTier::ReviewOnly,
        ),
        (
            "$RECYCLE.BIN",
            "Windows recycle bin",
            "Windows-managed deleted files that may belong to other accounts",
            CacheTier::ReviewOnly,
        ),
        (
            ".TemporaryItems",
            "Temporary items",
            "temporary files left on this volume; open files are kept",
            shared_tier,
        ),
    ];

    let mut specs: Vec<_> = definitions
        .into_iter()
        .map(|(relative, label, note, tier)| CacheSpec {
            home: root.to_path_buf(),
            path: root.join(relative),
            label,
            tier,
            process_pattern: "",
            note,
            target: CacheTarget::DirectoryContents,
        })
        .collect();

    if let Some(uid) = effective_user_id() {
        for path in [
            root.join(".Trashes").join(uid.to_string()),
            root.join(format!(".Trash-{uid}")),
        ] {
            specs.push(CacheSpec {
                home: root.to_path_buf(),
                path,
                label: "Volume Trash",
                tier: shared_tier,
                process_pattern: "",
                note: "files already moved to this volume's Trash",
                target: CacheTarget::DirectoryContents,
            });
        }
    }

    specs
}

/// Folders that any app may be writing to right now get an open-file check
/// before their contents are removed.
fn needs_open_check(spec: &CacheSpec) -> bool {
    spec.label == "Temporary items"
}

fn looks_like_home(path: &Path) -> bool {
    path.join("Library").is_dir()
        && (path.join(".Trash").exists()
            || path.join(".cache").exists()
            || path.join(".npm").exists()
            || path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "Users"))
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left == right
        || left.canonicalize().ok().is_some_and(|resolved_left| {
            right
                .canonicalize()
                .ok()
                .is_some_and(|resolved_right| resolved_left == resolved_right)
        })
}

pub fn scan_cache(spec: &CacheSpec, include_reinstallable: bool) -> CacheEntry {
    let mut status = status_for(spec, include_reinstallable);
    let mut identity = None;
    let size_kb = if matches!(
        status,
        CacheStatus::Missing | CacheStatus::ScanError | CacheStatus::Symlink | CacheStatus::Invalid
    ) {
        0
    } else {
        identity = PathIdentity::capture(&spec.path);
        if identity.is_none() {
            status = CacheStatus::ScanError;
            0
        } else {
            match try_target_kb(&spec.path, spec.target) {
                Ok(size_kb) => size_kb,
                Err(_) => {
                    status = CacheStatus::ScanError;
                    0
                }
            }
        }
    };

    CacheEntry {
        spec: spec.clone(),
        status,
        size_kb,
        outcome: None,
        identity,
    }
}

pub fn status_for(spec: &CacheSpec, include_reinstallable: bool) -> CacheStatus {
    let metadata = match fs::symlink_metadata(&spec.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return CacheStatus::Missing,
        Err(_) => return CacheStatus::Invalid,
    };

    if metadata.file_type().is_symlink() || has_symlink_component_below(&spec.path, &spec.home) {
        CacheStatus::Symlink
    } else if spec.target != CacheTarget::ExactPath && !metadata.is_dir() {
        CacheStatus::Invalid
    } else if metadata.is_dir() && fs::read_dir(&spec.path).is_err() {
        CacheStatus::ScanError
    } else if spec.tier == CacheTier::ReviewOnly {
        CacheStatus::Review
    } else {
        match related_process_state(spec.process_pattern) {
            ProcessState::Running => CacheStatus::InUse,
            ProcessState::CheckFailed => CacheStatus::ScanError,
            ProcessState::NotRunning
                if spec.tier == CacheTier::Reinstallable && !include_reinstallable =>
            {
                CacheStatus::Optional
            }
            ProcessState::NotRunning => CacheStatus::Ready,
        }
    }
}

struct NativeCleanupCommand {
    display: &'static str,
    program: PathBuf,
    args: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
}

enum NativeCommandResult {
    Succeeded,
    Failed(String),
}

/// Describes the actual order used for this finding. Vendor commands are used
/// only when they can be pinned to the exact allowlisted cache for the current
/// macOS account. All other candidates use the guarded exact-path implementation.
pub fn cleanup_plan(spec: &CacheSpec) -> String {
    if let CacheTarget::AgedContents { min_age_days } = spec.target {
        return format!(
            "Remove only direct entries untouched for at least {min_age_days} day(s) from the exact allowlisted directory; newer entries stay."
        );
    }
    match native_command_name(spec) {
        Some(command) => format!(
            "Run `{command}` only inside the exact allowlisted scope; if the tool is unavailable, use guarded exact-path cleanup. A failed command stops without a filesystem sweep."
        ),
        None => crate::rules::for_label(spec.label)
            .and_then(|rule| rule.advice)
            .map(str::to_owned)
            .unwrap_or_else(|| "No safe command can clear this exact directory without broader side effects; remove only contents of the exact allowlisted path.".into()),
    }
}

/// The compiled-in native command a rule references, if its path is still
/// exactly the path the command is pinned to.
fn native_for(spec: &CacheSpec) -> Option<&'static crate::rules::Native> {
    let native = crate::rules::for_label(spec.label)?.native?;
    (spec.path == spec.home.join(native.path)).then_some(native)
}

fn native_command_name(spec: &CacheSpec) -> Option<&'static str> {
    native_for(spec).map(|native| native.display)
}

fn native_cleanup_command(spec: &CacheSpec) -> Option<NativeCleanupCommand> {
    if !is_current_account_home(&spec.home) {
        return None;
    }

    let target = spec.path.as_os_str().to_os_string();
    let command = match native_for(spec)?.id {
        "pip" => NativeCleanupCommand {
            display: "python3 -m pip cache purge",
            program: resolve_executable(&[
                "python3",
                "/opt/homebrew/bin/python3",
                "/usr/local/bin/python3",
                "/usr/bin/python3",
            ])?,
            args: os_args(&["-m", "pip", "--cache-dir"])
                .into_iter()
                .chain([target])
                .chain(os_args(&["cache", "purge"]))
                .collect(),
            environment: vec![],
        },
        "go" => NativeCleanupCommand {
            display: "go clean -cache -testcache -fuzzcache",
            program: resolve_executable(&["go", "/opt/homebrew/bin/go", "/usr/local/bin/go"])?,
            args: os_args(&["clean", "-cache", "-testcache", "-fuzzcache"]),
            environment: vec![(OsString::from("GOCACHE"), target)],
        },
        "uv" => NativeCleanupCommand {
            display: "uv cache clean",
            program: resolve_executable(&["uv", "/opt/homebrew/bin/uv", "/usr/local/bin/uv"])?,
            args: os_args(&["cache", "clean", "--cache-dir"])
                .into_iter()
                .chain([target])
                .collect(),
            environment: vec![],
        },
        "yarn" => NativeCleanupCommand {
            display: "yarn cache clean",
            program: resolve_executable(&[
                "yarn",
                "/opt/homebrew/bin/yarn",
                "/usr/local/bin/yarn",
            ])?,
            args: os_args(&["cache", "clean"]),
            environment: vec![(OsString::from("YARN_CACHE_FOLDER"), target)],
        },
        "playwright" => NativeCleanupCommand {
            display: "playwright uninstall --all",
            // Deliberately do not invoke npx: it may download a package while
            // the application is trying to reclaim space.
            program: resolve_executable(&[
                "playwright",
                "/opt/homebrew/bin/playwright",
                "/usr/local/bin/playwright",
            ])?,
            args: os_args(&["uninstall", "--all"]),
            environment: vec![(OsString::from("PLAYWRIGHT_BROWSERS_PATH"), target)],
        },
        "cypress" => NativeCleanupCommand {
            display: "cypress cache clear",
            program: resolve_executable(&[
                "cypress",
                "/opt/homebrew/bin/cypress",
                "/usr/local/bin/cypress",
            ])?,
            args: os_args(&["cache", "clear"]),
            environment: vec![(OsString::from("CYPRESS_CACHE_FOLDER"), target)],
        },
        "huggingface" => NativeCleanupCommand {
            display: "hf cache prune --yes",
            program: resolve_executable(&["hf", "/opt/homebrew/bin/hf", "/usr/local/bin/hf"])?,
            args: os_args(&["cache", "prune", "--yes", "--cache-dir"])
                .into_iter()
                .chain([target])
                .collect(),
            environment: vec![],
        },
        _ => return None,
    };
    Some(command)
}

fn os_args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn is_current_account_home(home: &Path) -> bool {
    let Some(current_home) = env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    match (home.canonicalize(), current_home.canonicalize()) {
        (Ok(candidate), Ok(current)) => candidate == current,
        _ => home == current_home,
    }
}

fn resolve_executable(candidates: &[&str]) -> Option<PathBuf> {
    for candidate in candidates {
        let path = Path::new(candidate);
        if path.components().count() > 1 {
            if is_executable(path) {
                return Some(path.to_path_buf());
            }
            continue;
        }
        if let Some(found) = env::var_os("PATH")
            .into_iter()
            .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
            .map(|directory| directory.join(path))
            .find(|path| is_executable(path))
        {
            return Some(found);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn run_native_cleanup(
    command: &NativeCleanupCommand,
    working_directory: &Path,
) -> NativeCommandResult {
    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        .envs(command.environment.iter().cloned())
        .current_dir(working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(error) => return NativeCommandResult::Failed(error.to_string()),
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return NativeCommandResult::Succeeded,
            Ok(Some(status)) => {
                return NativeCommandResult::Failed(format!("exited with status {status}"));
            }
            Ok(None) if started.elapsed() < NATIVE_COMMAND_TIMEOUT => {
                thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return NativeCommandResult::Failed(format!(
                    "timed out after {} minutes",
                    NATIVE_COMMAND_TIMEOUT.as_secs() / 60
                ));
            }
            Err(error) => return NativeCommandResult::Failed(error.to_string()),
        }
    }
}

fn clear_with_native_first(
    spec: &CacheSpec,
    allowlist: &[PathBuf],
    expected: Option<&PathIdentity>,
    whitelist: &Whitelist,
) -> io::Result<CleanupMethod> {
    let swept = |kept: usize| {
        if kept == 0 {
            CleanupMethod::ExactPath
        } else {
            CleanupMethod::ExactPathKeeping { kept }
        }
    };
    if spec.target == CacheTarget::ExactPath {
        remove_exact_path(&spec.path, allowlist, &spec.home, expected, whitelist)?;
        return Ok(CleanupMethod::ExactPath);
    }
    if let CacheTarget::AgedContents { min_age_days } = spec.target {
        let kept = clear_aged_contents(
            &spec.path,
            allowlist,
            &spec.home,
            min_age_days,
            expected,
            whitelist,
        )?;
        return Ok(swept(kept));
    }

    validate_cleanup_target(
        &spec.path,
        allowlist,
        &spec.home,
        CacheTarget::DirectoryContents,
        expected,
    )?;
    let Some(command_name) = native_command_name(spec) else {
        let kept =
            clear_directory_contents(&spec.path, allowlist, &spec.home, expected, whitelist)?;
        return Ok(swept(kept));
    };
    // A vendor command cannot be told to spare one protected subfolder, so it
    // never runs when the whitelist names something inside the cache.
    if whitelist.covers_descendant(&spec.path) {
        let kept =
            clear_directory_contents(&spec.path, allowlist, &spec.home, expected, whitelist)?;
        return Ok(CleanupMethod::NativeSkippedForWhitelist {
            command: command_name,
            kept,
        });
    }
    let Some(command) = native_cleanup_command(spec) else {
        clear_directory_contents(&spec.path, allowlist, &spec.home, expected, whitelist)?;
        return Ok(CleanupMethod::NativeUnavailableThenExactPath {
            command: command_name,
        });
    };

    match run_native_cleanup(&command, &spec.home) {
        NativeCommandResult::Succeeded => {
            Ok(CleanupMethod::Native {
                command: command.display,
                // A successful vendor command is the complete operation. It
                // may intentionally leave protected, newer, or otherwise
                // ineligible data behind; sweeping those leftovers would
                // silently broaden the command's scope.
            })
        }
        NativeCommandResult::Failed(error) => Err(io::Error::other(format!(
            "native cleanup command `{}` failed; no filesystem fallback was attempted: {error}",
            command.display
        ))),
    }
}

pub fn clean_cache(
    entry: &mut CacheEntry,
    allowlist: &[PathBuf],
    include_reinstallable: bool,
    whitelist: &Whitelist,
) -> CleanupOutcome {
    if entry.spec.tier == CacheTier::ReviewOnly {
        let outcome = CleanupOutcome::SafetySkipped(
            "review-only storage is excluded from ordinary cleanup".into(),
        );
        entry.status = CacheStatus::Review;
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    if whitelist.protects(&entry.spec.path) {
        let outcome = CleanupOutcome::SafetySkipped("path matches the user whitelist".into());
        entry.status = CacheStatus::Whitelisted;
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    let current_status = status_for(&entry.spec, include_reinstallable);
    if current_status != CacheStatus::Ready {
        let outcome =
            CleanupOutcome::SafetySkipped(format!("status changed to {}", current_status.label()));
        entry.status = current_status;
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    if !identity_still_holds(entry) {
        let outcome = CleanupOutcome::SafetySkipped(
            "the path changed on disk after the scan; it will not be cleaned".into(),
        );
        entry.status = CacheStatus::ScanError;
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    let size_before = match try_target_kb(&entry.spec.path, entry.spec.target) {
        Ok(size_kb) => size_kb,
        Err(error) => {
            let outcome = CleanupOutcome::SafetySkipped(format!(
                "could not measure the directory safely: {error}"
            ));
            entry.status = CacheStatus::ScanError;
            entry.outcome = Some(outcome.clone());
            return outcome;
        }
    };
    if size_before == 0 {
        let reason = if matches!(entry.spec.target, CacheTarget::AgedContents { .. }) {
            "no entries are past the retention age"
        } else {
            "already empty"
        };
        let outcome = CleanupOutcome::SafetySkipped(reason.into());
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    if entry.spec.target == CacheTarget::ExactPath {
        if whitelist.covers_descendant(&entry.spec.path) {
            let outcome =
                CleanupOutcome::SafetySkipped("a whitelisted path is inside this entry".into());
            entry.status = CacheStatus::Whitelisted;
            entry.outcome = Some(outcome.clone());
            return outcome;
        }
        if let Err(error) = verify_current_user_ownership(&entry.spec.path) {
            let outcome = CleanupOutcome::SafetySkipped(format!(
                "the temporary path ownership could not be revalidated: {error}"
            ));
            entry.status = CacheStatus::ScanError;
            entry.outcome = Some(outcome.clone());
            return outcome;
        }
        match path_is_open(&entry.spec.path) {
            Ok(true) => {
                let outcome = CleanupOutcome::SafetySkipped(
                    "the temporary path is still open by a process".into(),
                );
                entry.status = CacheStatus::InUse;
                entry.outcome = Some(outcome.clone());
                return outcome;
            }
            Ok(false) => {}
            Err(error) => {
                let outcome = CleanupOutcome::SafetySkipped(format!(
                    "could not verify whether the temporary path is open: {error}"
                ));
                entry.status = CacheStatus::ScanError;
                entry.outcome = Some(outcome.clone());
                return outcome;
            }
        }
    }

    if needs_open_check(&entry.spec) {
        match path_is_open(&entry.spec.path) {
            Ok(false) => {}
            Ok(true) => {
                let outcome =
                    CleanupOutcome::SafetySkipped("a process has files open in this folder".into());
                entry.status = CacheStatus::InUse;
                entry.outcome = Some(outcome.clone());
                return outcome;
            }
            Err(error) => {
                let outcome = CleanupOutcome::SafetySkipped(format!(
                    "could not verify whether files here are open: {error}"
                ));
                entry.status = CacheStatus::ScanError;
                entry.outcome = Some(outcome.clone());
                return outcome;
            }
        }
    }

    match related_process_state(entry.spec.process_pattern) {
        ProcessState::Running => {
            let outcome = CleanupOutcome::SafetySkipped("a related process just started".into());
            entry.status = CacheStatus::InUse;
            entry.outcome = Some(outcome.clone());
            return outcome;
        }
        ProcessState::CheckFailed => {
            let outcome = CleanupOutcome::SafetySkipped(
                "could not verify that related applications are closed".into(),
            );
            entry.status = CacheStatus::ScanError;
            entry.outcome = Some(outcome.clone());
            return outcome;
        }
        ProcessState::NotRunning => {}
    }

    let outcome =
        match clear_with_native_first(&entry.spec, allowlist, entry.identity.as_ref(), whitelist) {
            Ok(method) => {
                let size_after = target_kb(&entry.spec.path, entry.spec.target);
                let removed = size_before.saturating_sub(size_after);
                entry.size_kb = size_after;
                CleanupOutcome::Cleared {
                    removed_kb: removed,
                    method,
                }
            }
            Err(error) => {
                let size_after = target_kb(&entry.spec.path, entry.spec.target);
                entry.size_kb = size_after;
                CleanupOutcome::Failed {
                    error: error.to_string(),
                    removed_kb: size_before.saturating_sub(size_after),
                }
            }
        };
    entry.outcome = Some(outcome.clone());
    outcome
}

/// Permanently clear one explicitly confirmed review-only directory.
///
/// This is deliberately separate from `clean_cache` so review data can never
/// enter ordinary multi-select or unattended cleanup. The TUI calls it only
/// after a per-item typed confirmation.
pub fn clean_review_data(
    entry: &mut CacheEntry,
    allowlist: &[PathBuf],
    whitelist: &Whitelist,
) -> CleanupOutcome {
    if entry.spec.tier != CacheTier::ReviewOnly {
        let outcome = CleanupOutcome::SafetySkipped(
            "advanced review deletion only accepts review-only storage".into(),
        );
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    if whitelist.protects(&entry.spec.path) {
        let outcome = CleanupOutcome::SafetySkipped("path matches the user whitelist".into());
        entry.status = CacheStatus::Whitelisted;
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    let current_status = status_for(&entry.spec, false);
    if current_status != CacheStatus::Review {
        let outcome =
            CleanupOutcome::SafetySkipped(format!("status changed to {}", current_status.label()));
        entry.status = current_status;
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    if !identity_still_holds(entry) {
        let outcome = CleanupOutcome::SafetySkipped(
            "the path changed on disk after the scan; it will not be deleted".into(),
        );
        entry.status = CacheStatus::ScanError;
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    match related_process_state(entry.spec.process_pattern) {
        ProcessState::Running => {
            let outcome = CleanupOutcome::SafetySkipped(
                "the related application is running; close it before deleting its data".into(),
            );
            entry.outcome = Some(outcome.clone());
            return outcome;
        }
        ProcessState::CheckFailed => {
            let outcome = CleanupOutcome::SafetySkipped(
                "could not verify that the related application is closed".into(),
            );
            entry.status = CacheStatus::ScanError;
            entry.outcome = Some(outcome.clone());
            return outcome;
        }
        ProcessState::NotRunning => {}
    }

    let size_before = match try_target_kb(&entry.spec.path, entry.spec.target) {
        Ok(size_kb) => size_kb,
        Err(error) => {
            let outcome = CleanupOutcome::SafetySkipped(format!(
                "could not measure the directory safely: {error}"
            ));
            entry.status = CacheStatus::ScanError;
            entry.outcome = Some(outcome.clone());
            return outcome;
        }
    };
    if size_before == 0 {
        let outcome = CleanupOutcome::SafetySkipped("already empty".into());
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    let outcome =
        match clear_with_native_first(&entry.spec, allowlist, entry.identity.as_ref(), whitelist) {
            Ok(method) => {
                let size_after = target_kb(&entry.spec.path, entry.spec.target);
                entry.size_kb = size_after;
                CleanupOutcome::Cleared {
                    removed_kb: size_before.saturating_sub(size_after),
                    method,
                }
            }
            Err(error) => {
                let size_after = target_kb(&entry.spec.path, entry.spec.target);
                entry.size_kb = size_after;
                CleanupOutcome::Failed {
                    error: error.to_string(),
                    removed_kb: size_before.saturating_sub(size_after),
                }
            }
        };
    entry.outcome = Some(outcome.clone());
    outcome
}

/// Remove every direct entry of `target` except whitelisted ones (or ones with
/// a whitelisted descendant). Returns how many entries were kept.
pub fn clear_directory_contents(
    target: &Path,
    allowlist: &[PathBuf],
    root: &Path,
    expected: Option<&PathIdentity>,
    whitelist: &Whitelist,
) -> io::Result<usize> {
    validate_cleanup_target(
        target,
        allowlist,
        root,
        CacheTarget::DirectoryContents,
        expected,
    )?;

    let mut kept = 0;
    let mut completely_removed = 0;
    for item in fs::read_dir(target)? {
        let path = item
            .map_err(|error| cleanup_progress_error(error, completely_removed))?
            .path();
        if whitelist.keeps(&path) {
            kept += 1;
            continue;
        }
        let item_metadata = fs::symlink_metadata(&path)
            .map_err(|error| cleanup_progress_error(error, completely_removed))?;
        let result = if item_metadata.is_dir() && !item_metadata.file_type().is_symlink() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        result.map_err(|error| cleanup_progress_error(error, completely_removed))?;
        completely_removed += 1;
    }
    Ok(kept)
}

/// Clear only the direct entries of an aged directory whose whole subtree has
/// been untouched past the retention window and that no process holds open.
/// Symlinks are never followed or removed here, whitelisted entries stay, and
/// the directory itself always stays. Returns how many stale entries were kept
/// because they are whitelisted or open.
fn clear_aged_contents(
    target: &Path,
    allowlist: &[PathBuf],
    root: &Path,
    min_age_days: u64,
    expected: Option<&PathIdentity>,
    whitelist: &Whitelist,
) -> io::Result<usize> {
    validate_cleanup_target(
        target,
        allowlist,
        root,
        CacheTarget::DirectoryContents,
        expected,
    )?;
    let cutoff = age_cutoff(min_age_days);
    // Logs and saved state are written by running apps. Nothing is removed
    // unless the open-file check itself succeeds.
    let open = open_paths_under(target).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not verify which old entries are open; nothing was removed: {error}"),
        )
    })?;

    let mut kept = 0;
    let mut completely_removed = 0;
    for item in fs::read_dir(target)? {
        let path = item
            .map_err(|error| cleanup_progress_error(error, completely_removed))?
            .path();
        let Ok(item_metadata) = fs::symlink_metadata(&path) else {
            // A vanished or unreadable entry is left alone.
            continue;
        };
        if item_metadata.file_type().is_symlink()
            || !subtree_is_stale(&path, &item_metadata, cutoff)
        {
            continue;
        }
        if whitelist.keeps(&path) || holds_open_path(&path, &open) {
            kept += 1;
            continue;
        }
        let result = if item_metadata.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        result.map_err(|error| cleanup_progress_error(error, completely_removed))?;
        completely_removed += 1;
    }
    Ok(kept)
}

/// Whether any open path reported by lsof is `path` or inside it.
fn holds_open_path(path: &Path, open: &HashSet<PathBuf>) -> bool {
    let resolved = path.canonicalize().ok();
    open.iter().any(|open_path| {
        open_path.starts_with(path)
            || resolved
                .as_ref()
                .is_some_and(|resolved| open_path.starts_with(resolved))
    })
}

fn remove_exact_path(
    target: &Path,
    allowlist: &[PathBuf],
    root: &Path,
    expected: Option<&PathIdentity>,
    whitelist: &Whitelist,
) -> io::Result<()> {
    validate_cleanup_target(target, allowlist, root, CacheTarget::ExactPath, expected)?;
    if whitelist.keeps(target) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("a whitelisted path is at or inside {}", target.display()),
        ));
    }
    let metadata = fs::symlink_metadata(target)?;
    if metadata.is_dir() {
        fs::remove_dir_all(target)
    } else {
        fs::remove_file(target)
    }
}

fn validate_cleanup_target(
    target: &Path,
    allowlist: &[PathBuf],
    root: &Path,
    target_kind: CacheTarget,
    expected: Option<&PathIdentity>,
) -> io::Result<()> {
    if !allowlist.iter().any(|allowed| allowed == target) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("path is not on the exact allowlist: {}", target.display()),
        ));
    }

    // The deletion sink re-verifies the identity observed during the scan so
    // a path deleted and recreated after the scan is never cleaned.
    if let Some(expected) = expected
        && !expected.still_matches(target)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "path changed on disk since the scan; refusing to clean {}",
                target.display()
            ),
        ));
    }

    let metadata = fs::symlink_metadata(target)?;
    if metadata.file_type().is_symlink() || has_symlink_component_below(target, root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "target is not a real filesystem entry: {}",
                target.display()
            ),
        ));
    }
    if target_kind == CacheTarget::DirectoryContents && !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("target is not a real directory: {}", target.display()),
        ));
    }

    let resolved_root = root.canonicalize()?;
    let resolved_target = target.canonicalize()?;
    if resolved_target == resolved_root || !resolved_target.starts_with(&resolved_root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "target is not a strict descendant of the safety root: {}",
                target.display()
            ),
        ));
    }
    Ok(())
}

fn cleanup_progress_error(error: io::Error, completely_removed: usize) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "cleanup may be partial; removed {completely_removed} complete top-level item(s) before the error: {error}"
        ),
    )
}

/// Whether the entry's scan-time identity still describes the current
/// filesystem. Entries without a captured identity fail closed.
fn identity_still_holds(entry: &CacheEntry) -> bool {
    match entry.identity {
        Some(identity) => identity.still_matches(&entry.spec.path),
        None => false,
    }
}

pub(crate) fn age_cutoff(min_age_days: u64) -> SystemTime {
    SystemTime::now()
        .checked_sub(Duration::from_secs(
            min_age_days.saturating_mul(24 * 60 * 60),
        ))
        .unwrap_or(UNIX_EPOCH)
}

fn is_stale(metadata: &fs::Metadata, cutoff: SystemTime) -> bool {
    metadata.modified().is_ok_and(|modified| modified <= cutoff)
}

/// Whether an entry and everything inside it predate `cutoff`. A folder's own
/// timestamp does not change when a file inside it is rewritten, so every
/// descendant is checked. The walk fails closed: an unreadable, cross-device,
/// or unfinished walk reports "not stale".
fn subtree_is_stale(path: &Path, metadata: &fs::Metadata, cutoff: SystemTime) -> bool {
    const MAX_ENTRIES: usize = 10_000;
    const TIME_LIMIT: Duration = Duration::from_secs(2);
    if !is_stale(metadata, cutoff) {
        return false;
    }
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return true;
    }
    let device = metadata.dev();
    let started = std::time::Instant::now();
    let mut seen = 0;
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(reader) = fs::read_dir(&directory) else {
            return false;
        };
        for item in reader {
            seen += 1;
            if seen > MAX_ENTRIES || started.elapsed() > TIME_LIMIT {
                return false;
            }
            let Ok(item) = item else {
                return false;
            };
            let Ok(item_metadata) = fs::symlink_metadata(item.path()) else {
                return false;
            };
            if item_metadata.dev() != device || !is_stale(&item_metadata, cutoff) {
                return false;
            }
            if item_metadata.is_dir() && !item_metadata.file_type().is_symlink() {
                pending.push(item.path());
            }
        }
    }
    true
}

pub fn directory_kb(path: &Path) -> u64 {
    try_directory_kb(path).unwrap_or(0)
}

pub(crate) fn target_kb(path: &Path, target: CacheTarget) -> u64 {
    try_target_kb(path, target).unwrap_or(0)
}

pub(crate) fn measure_target_kb(path: &Path, target: CacheTarget) -> io::Result<u64> {
    try_target_kb(path, target)
}

fn try_target_kb(path: &Path, target: CacheTarget) -> io::Result<u64> {
    if target == CacheTarget::DirectoryContents {
        return try_directory_kb(path);
    }
    if let CacheTarget::AgedContents { min_age_days } = target {
        return aged_contents_kb(path, min_age_days);
    }

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is a symlink: {}", path.display()),
        ));
    }
    measure_with_du(path)
}

/// Total size of the direct entries old enough to be cleaned. Recent entries,
/// symlinks, and unreadable entries are excluded so the reported size always
/// describes what a cleanup would actually remove.
fn aged_contents_kb(path: &Path, min_age_days: u64) -> io::Result<u64> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not a real directory: {}", path.display()),
        ));
    }

    let cutoff = age_cutoff(min_age_days);
    let mut total_kb = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let Ok(item_metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if item_metadata.file_type().is_symlink()
            || !subtree_is_stale(&entry.path(), &item_metadata, cutoff)
        {
            continue;
        }
        total_kb += measure_with_du(&entry.path()).unwrap_or(0);
    }
    Ok(total_kb)
}

fn try_directory_kb(path: &Path) -> io::Result<u64> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not a real directory: {}", path.display()),
        ));
    }

    measure_with_du(path)
}

fn measure_with_du(path: &Path) -> io::Result<u64> {
    let output = Command::new(DU_COMMAND).args(["-sk"]).arg(path).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "{DU_COMMAND} could not read {}",
            path.display()
        )));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "du returned non-UTF-8 output"))?;
    text.split_whitespace()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "du returned no size"))?
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "du returned an invalid size"))
}

pub(crate) fn open_paths_under(root: &Path) -> io::Result<HashSet<PathBuf>> {
    let output = Command::new(LSOF_COMMAND)
        .args(["-nP", "-F", "n", "+D"])
        .arg(root)
        .output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(io::Error::other(format!(
            "{LSOF_COMMAND} could not inspect open paths under {}",
            root.display()
        )));
    }
    Ok(parse_open_paths(&String::from_utf8_lossy(&output.stdout)))
}

pub(crate) fn path_is_open(path: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    let mut command = Command::new(LSOF_COMMAND);
    command.args(["-nP", "-F", "n"]);
    if metadata.is_dir() {
        command.arg("+D");
    }
    let output = command.arg(path).output()?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(io::Error::other(format!(
            "{LSOF_COMMAND} could not inspect {}",
            path.display()
        )));
    }
    let open_paths = parse_open_paths(&String::from_utf8_lossy(&output.stdout));
    // lsof reports canonical paths (for example `/private/var/...`), so a
    // symlinked spelling of the same location must be compared resolved.
    Ok(holds_open_path(path, &open_paths))
}

fn parse_open_paths(output: &str) -> HashSet<PathBuf> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn verify_current_user_ownership(path: &Path) -> io::Result<()> {
    let uid = effective_user_id().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "could not determine the effective user ID",
        )
    })?;
    verify_tree_ownership(path, uid)
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
        verify_tree_ownership(&entry?.path(), uid)?;
    }
    Ok(())
}

pub fn free_kb(root: &Path) -> u64 {
    Command::new(DF_COMMAND)
        .args(["-k"])
        .arg(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|text| text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok())
        .unwrap_or(0)
}

pub fn format_kb(kb: u64) -> String {
    if kb >= 1_048_576 {
        format!("{:.1} GiB", kb as f64 / 1_048_576.0)
    } else if kb >= 1_024 {
        format!("{:.1} MiB", kb as f64 / 1_024.0)
    } else {
        format!("{kb} KiB")
    }
}

pub fn process_is_running(pattern: &str) -> bool {
    !matches!(related_process_state(pattern), ProcessState::NotRunning)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessState {
    NotRunning,
    Running,
    CheckFailed,
}

/// Running processes that match a rule's process pattern, as `name (PID n)`,
/// so the interface can say what blocks an in-use cache. At most three.
pub fn blocking_processes(pattern: &str) -> Vec<String> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let Ok(output) = Command::new(PGREP_COMMAND)
        .args(["-lf", pattern])
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, command) = line.trim().split_once(' ')?;
            let pid: u32 = pid.parse().ok()?;
            (pid != std::process::id()).then(|| {
                let mut words = command.split_whitespace();
                let program = words
                    .next()
                    .map(|program| program.rsplit('/').next().unwrap_or(program))
                    .unwrap_or_default();
                let rest: Vec<&str> = words.take(2).collect();
                let name = if rest.is_empty() {
                    program.to_string()
                } else {
                    format!("{program} {}", rest.join(" "))
                };
                format!(
                    "{} (PID {pid})",
                    crate::ai::display_text(&name)
                        .chars()
                        .take(48)
                        .collect::<String>()
                )
            })
        })
        .take(3)
        .collect()
}

fn related_process_state(pattern: &str) -> ProcessState {
    if pattern.is_empty() {
        return ProcessState::NotRunning;
    }
    match Command::new(PGREP_COMMAND)
        .args(["-f", pattern])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => ProcessState::Running,
        Ok(status) if status.code() == Some(1) => ProcessState::NotRunning,
        Ok(_) | Err(_) => ProcessState::CheckFailed,
    }
}

pub(crate) fn has_symlink_component_below(path: &Path, root: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    let mut current = root.to_path_buf();
    if fs::symlink_metadata(&current)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return true;
    }
    relative.components().any(|component| {
        current.push(component);
        fs::symlink_metadata(&current)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    })
}

pub fn validate_scan_root(path: &Path) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot access scan root {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("scan root cannot be a symlink: {}", path.display()));
    }
    if !metadata.is_dir() {
        return Err(format!("scan root is not a directory: {}", path.display()));
    }
    path.canonicalize()
        .map_err(|error| format!("cannot resolve scan root {}: {error}", path.display()))
}

pub fn scan_locations(home: &Path) -> Vec<ScanLocation> {
    scan_locations_in(home, Path::new("/Volumes"), true)
}

pub fn scan_location_kind(path: &Path) -> ScanLocationKind {
    let filesystem = mounted_filesystem(path);
    classify_scan_location(disk_info(path).as_deref(), filesystem.as_deref())
}

fn scan_locations_in(
    home: &Path,
    volumes_dir: &Path,
    include_startup_volume: bool,
) -> Vec<ScanLocation> {
    let mut locations = Vec::new();

    if include_startup_volume {
        push_unique_location(
            &mut locations,
            ScanLocation {
                label: "Startup volume".into(),
                path: PathBuf::from("/"),
                kind: ScanLocationKind::Local,
            },
        );
    }

    push_unique_location(
        &mut locations,
        ScanLocation {
            label: "Home directory".into(),
            path: home.to_path_buf(),
            kind: ScanLocationKind::Local,
        },
    );

    let mut mounted = fs::read_dir(volumes_dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).ok()?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return None;
            }
            Some(ScanLocation {
                label: entry.file_name().to_string_lossy().into_owned(),
                kind: scan_location_kind(&path),
                path,
            })
        })
        .collect::<Vec<_>>();
    mounted.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.label.to_lowercase().cmp(&right.label.to_lowercase()))
    });
    for location in mounted {
        push_unique_location(&mut locations, location);
    }
    locations
}

fn classify_scan_location(
    disk_info: Option<&str>,
    filesystem_type: Option<&str>,
) -> ScanLocationKind {
    if filesystem_type.is_some_and(is_network_filesystem) {
        return ScanLocationKind::Network;
    }
    if disk_info
        .and_then(|info| plist_string(info, "BusProtocol"))
        .is_some_and(|protocol| protocol.eq_ignore_ascii_case("USB"))
    {
        return ScanLocationKind::Usb;
    }
    ScanLocationKind::Local
}

fn is_network_filesystem(filesystem_type: &str) -> bool {
    matches!(
        filesystem_type.to_ascii_lowercase().as_str(),
        "afpfs" | "autofs" | "cifs" | "fuse.sshfs" | "nfs" | "smbfs" | "sshfs" | "webdav"
    )
}

fn disk_info(path: &Path) -> Option<String> {
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

fn mounted_filesystem(path: &Path) -> Option<String> {
    let output = Command::new(MOUNT_COMMAND).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let mounts = String::from_utf8(output.stdout).ok()?;
    mounted_filesystem_from_output(&mounts, path)
}

/// The filesystem type of the mount that contains `path`: the longest mount
/// point that is a whole-component prefix of it. A folder inside a network
/// share is therefore recognized as network storage too.
fn mounted_filesystem_from_output(output: &str, path: &Path) -> Option<String> {
    output
        .lines()
        .filter_map(|line| {
            let (mount_description, options) = line.rsplit_once(" (")?;
            let (_, mount_point) = mount_description.rsplit_once(" on ")?;
            if !path.starts_with(Path::new(mount_point)) {
                return None;
            }
            let filesystem = options
                .strip_suffix(')')?
                .split(',')
                .next()?
                .trim()
                .to_string();
            Some((Path::new(mount_point).components().count(), filesystem))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, filesystem)| filesystem)
}

fn plist_string<'a>(plist: &'a str, key: &str) -> Option<&'a str> {
    let key_marker = format!("<key>{key}</key>");
    let value = plist.split_once(&key_marker)?.1;
    let value = value.split_once("<string>")?.1;
    value.split_once("</string>").map(|(value, _)| value.trim())
}

fn push_unique_location(locations: &mut Vec<ScanLocation>, location: ScanLocation) {
    let resolved = location
        .path
        .canonicalize()
        .unwrap_or_else(|_| location.path.clone());
    if locations.iter().any(|existing| {
        existing
            .path
            .canonicalize()
            .unwrap_or_else(|_| existing.path.clone())
            == resolved
    }) {
        return;
    }
    locations.push(ScanLocation {
        path: resolved,
        ..location
    });
}

pub fn validate_environment() -> Result<PathBuf, String> {
    if env::consts::OS != "macos" {
        return Err("this application is intended for macOS".into());
    }

    let user_id = effective_user_id()
        .ok_or_else(|| "could not determine the effective user ID safely".to_string())?;
    if user_id == 0 {
        return Err("do not run this application with sudo or as root".into());
    }

    let home = account_home()
        .or_else(|| env::var_os("HOME").map(PathBuf::from))
        .ok_or_else(|| "could not determine the current account's home directory".to_string())?;

    if home == Path::new("/") || !home.is_dir() {
        return Err(format!(
            "unsafe or invalid home directory: {}",
            home.display()
        ));
    }
    validate_scan_root(&home).map_err(|error| format!("invalid home directory: {error}"))
}

pub(crate) fn effective_user_id() -> Option<u32> {
    Command::new(ID_COMMAND)
        .arg("-u")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| value.trim().parse().ok())
}

pub(crate) fn account_home() -> Option<PathBuf> {
    let user = Command::new(ID_COMMAND)
        .arg("-un")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())?;
    let user = user.trim();

    let output = Command::new(DSCL_COMMAND)
        .args([".", "-read", &format!("/Users/{user}"), "NFSHomeDirectory"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    parse_account_home(&text)
}

fn parse_account_home(output: &str) -> Option<PathBuf> {
    output.lines().find_map(|line| {
        let path = line.trim_start().strip_prefix("NFSHomeDirectory:")?.trim();
        (!path.is_empty()).then(|| PathBuf::from(path))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::whitelist::Whitelist;
    use std::io::Write;

    fn no_whitelist() -> Whitelist {
        Whitelist::empty()
    }

    fn native_command_name_for(spec: &CacheSpec) -> Option<&'static str> {
        super::native_command_name(spec)
    }

    fn native_command_name(label: &str) -> Option<&'static str> {
        crate::rules::for_label(label)
            .and_then(|rule| rule.native)
            .map(|native| native.display)
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_kb(12), "12 KiB");
        assert_eq!(format_kb(1_536), "1.5 MiB");
        assert_eq!(format_kb(2_097_152), "2.0 GiB");
    }

    #[test]
    fn refuses_paths_outside_allowlist() {
        let temp = tempfile::tempdir().unwrap();
        let error = clear_directory_contents(temp.path(), &[], temp.path(), None, &no_whitelist())
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn refuses_to_clear_the_safety_root_itself() {
        let temp = tempfile::tempdir().unwrap();
        fs::File::create(temp.path().join("keep-me")).unwrap();

        let error = clear_directory_contents(
            temp.path(),
            std::slice::from_ref(&temp.path().to_path_buf()),
            temp.path(),
            None,
            &no_whitelist(),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(temp.path().join("keep-me").exists());
    }

    #[test]
    fn refuses_an_allowlisted_path_that_escapes_its_safety_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::File::create(outside.join("keep-me")).unwrap();
        let escaping_path = root.join("../outside");

        let error = clear_directory_contents(
            &escaping_path,
            std::slice::from_ref(&escaping_path),
            &root,
            None,
            &no_whitelist(),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(outside.join("keep-me").exists());
    }

    #[test]
    fn clears_contents_but_keeps_cache_directory() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        fs::create_dir(&cache).unwrap();
        fs::create_dir(cache.join("nested")).unwrap();
        fs::File::create(cache.join("nested/file"))
            .unwrap()
            .write_all(b"data")
            .unwrap();
        fs::File::create(cache.join("top-level"))
            .unwrap()
            .write_all(b"data")
            .unwrap();

        clear_directory_contents(
            &cache,
            std::slice::from_ref(&cache),
            temp.path(),
            None,
            &no_whitelist(),
        )
        .unwrap();

        assert!(cache.is_dir());
        assert_eq!(fs::read_dir(cache).unwrap().count(), 0);
    }

    #[test]
    fn failed_cleanup_preserves_the_remaining_measured_size() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("keep-me"), [1_u8; 8_192]).unwrap();
        let size_before = directory_kb(&cache);
        let mut entry = CacheEntry {
            spec: CacheSpec {
                home: temp.path().to_path_buf(),
                path: cache.clone(),
                label: "Test cache",
                tier: CacheTier::Routine,
                process_pattern: "",
                note: "test data",
                target: CacheTarget::DirectoryContents,
            },
            status: CacheStatus::Ready,
            size_kb: size_before,
            outcome: None,
            identity: PathIdentity::capture(&cache),
        };

        let outcome = clean_cache(&mut entry, &[], false, &no_whitelist());

        assert!(matches!(
            outcome,
            CleanupOutcome::Failed { removed_kb: 0, .. }
        ));
        assert_eq!(entry.size_kb, size_before);
        assert!(cache.join("keep-me").exists());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_cache_directory() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("actual");
        let linked = temp.path().join("linked");
        fs::create_dir(&actual).unwrap();
        symlink(&actual, &linked).unwrap();

        let error = clear_directory_contents(
            &linked,
            std::slice::from_ref(&linked),
            temp.path(),
            None,
            &no_whitelist(),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(actual.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_cache_beneath_a_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let actual_parent = temp.path().join("actual-parent");
        let linked_parent = temp.path().join("linked-parent");
        let actual_cache = actual_parent.join("cache");
        fs::create_dir(&actual_parent).unwrap();
        fs::create_dir(&actual_cache).unwrap();
        fs::File::create(actual_cache.join("keep-me")).unwrap();
        symlink(&actual_parent, &linked_parent).unwrap();
        let linked_cache = linked_parent.join("cache");

        let error = clear_directory_contents(
            &linked_cache,
            std::slice::from_ref(&linked_cache),
            temp.path(),
            None,
            &no_whitelist(),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(actual_cache.join("keep-me").exists());
    }

    #[cfg(unix)]
    #[test]
    fn removes_child_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let outside = temp.path().join("outside");
        fs::create_dir(&cache).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::File::create(outside.join("keep-me")).unwrap();
        symlink(&outside, cache.join("linked-child")).unwrap();

        clear_directory_contents(
            &cache,
            std::slice::from_ref(&cache),
            temp.path(),
            None,
            &no_whitelist(),
        )
        .unwrap();

        assert!(outside.join("keep-me").exists());
        assert_eq!(fs::read_dir(cache).unwrap().count(), 0);
    }

    #[test]
    fn a_target_replaced_after_the_scan_is_not_cleaned() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let identity = PathIdentity::capture(&cache).unwrap();
        // Simulate the original directory being deleted and recreated: the
        // path is the same but the inode is new.
        fs::remove_dir_all(&cache).unwrap();
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("fresh-data"), [1_u8; 8_192]).unwrap();

        let error = clear_directory_contents(
            &cache,
            std::slice::from_ref(&cache),
            temp.path(),
            Some(&identity),
            &no_whitelist(),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("changed on disk"));
        assert!(cache.join("fresh-data").exists());
    }

    #[test]
    fn clean_cache_skips_when_the_path_changed_after_the_scan() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let mut entry = scan_cache(
            &CacheSpec {
                home: temp.path().to_path_buf(),
                path: cache.clone(),
                label: "Test cache",
                tier: CacheTier::Routine,
                process_pattern: "",
                note: "test data",
                target: CacheTarget::DirectoryContents,
            },
            false,
        );
        fs::write(cache.join("data"), [1_u8; 8_192]).unwrap();
        fs::remove_dir_all(&cache).unwrap();
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("replacement"), [2_u8; 8_192]).unwrap();

        let outcome = clean_cache(
            &mut entry,
            std::slice::from_ref(&cache),
            false,
            &no_whitelist(),
        );

        assert!(matches!(outcome, CleanupOutcome::SafetySkipped(_)));
        assert_eq!(entry.status, CacheStatus::ScanError);
        assert!(cache.join("replacement").exists());
    }

    #[test]
    fn whitelisted_paths_are_never_cleaned() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("keep-me"), [1_u8; 8_192]).unwrap();
        let mut entry = scan_cache(
            &CacheSpec {
                home: temp.path().to_path_buf(),
                path: cache.clone(),
                label: "Test cache",
                tier: CacheTier::Routine,
                process_pattern: "",
                note: "test data",
                target: CacheTarget::DirectoryContents,
            },
            false,
        );
        let whitelist = Whitelist::parse("cache\n", temp.path());

        let outcome = clean_cache(&mut entry, std::slice::from_ref(&cache), false, &whitelist);

        assert!(matches!(outcome, CleanupOutcome::SafetySkipped(_)));
        assert_eq!(entry.status, CacheStatus::Whitelisted);
        assert!(cache.join("keep-me").exists());
    }

    fn backdate(path: &Path, days: u64) {
        fs::File::open(path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60))
            .unwrap();
    }

    fn test_spec(home: &Path, path: &Path, target: CacheTarget) -> CacheSpec {
        CacheSpec {
            home: home.to_path_buf(),
            path: path.to_path_buf(),
            label: "Test cache",
            tier: CacheTier::Routine,
            process_pattern: "",
            note: "test data",
            target,
        }
    }

    #[test]
    fn aged_cleanup_removes_only_fully_stale_entries_and_keeps_the_directory() {
        let temp = tempfile::tempdir().unwrap();
        let logs = temp.path().join("logs");
        fs::create_dir(&logs).unwrap();
        fs::write(logs.join("old.log"), [1_u8; 8_192]).unwrap();
        fs::write(logs.join("recent.log"), [2_u8; 4_096]).unwrap();
        // A folder whose own timestamp is old but which holds a fresh file is
        // still in use and must be kept.
        fs::create_dir(logs.join("active-dir")).unwrap();
        fs::write(logs.join("active-dir/current.log"), [3_u8; 4_096]).unwrap();
        // A folder that is old all the way down is removable.
        fs::create_dir(logs.join("old-dir")).unwrap();
        fs::write(logs.join("old-dir/data"), [4_u8; 4_096]).unwrap();
        backdate(&logs.join("old-dir/data"), 14);
        for name in ["old.log", "old-dir", "active-dir"] {
            backdate(&logs.join(name), 14);
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(logs.join("recent.log"), logs.join("link.log")).unwrap();

        let spec = CacheSpec {
            label: "User logs",
            ..test_spec(
                temp.path(),
                &logs,
                CacheTarget::AgedContents { min_age_days: 7 },
            )
        };
        let measured = scan_cache(&spec, false).size_kb;
        assert!(
            measured >= 12,
            "stale entries should be measured, got {measured}"
        );
        assert!(
            measured < 16,
            "active entries must not be measured, got {measured}"
        );

        let mut entry = scan_cache(&spec, false);
        let outcome = clean_cache(
            &mut entry,
            std::slice::from_ref(&logs),
            false,
            &no_whitelist(),
        );

        assert!(matches!(
            outcome,
            CleanupOutcome::Cleared { removed_kb, .. } if removed_kb >= 12
        ));
        assert!(logs.is_dir());
        assert!(!logs.join("old.log").exists());
        assert!(!logs.join("old-dir").exists());
        assert!(logs.join("active-dir/current.log").exists());
        assert!(logs.join("recent.log").exists());
        assert!(logs.join("link.log").exists());
    }

    #[test]
    fn aged_cleanup_keeps_old_files_that_a_process_holds_open() {
        let temp = tempfile::tempdir().unwrap();
        let logs = temp.path().join("logs");
        fs::create_dir(&logs).unwrap();
        fs::write(logs.join("held.log"), [1_u8; 4_096]).unwrap();
        fs::write(logs.join("free.log"), [2_u8; 4_096]).unwrap();
        backdate(&logs.join("held.log"), 30);
        backdate(&logs.join("free.log"), 30);
        let held = fs::File::open(logs.join("held.log")).unwrap();

        let kept = clear_aged_contents(
            &logs,
            std::slice::from_ref(&logs),
            temp.path(),
            7,
            None,
            &no_whitelist(),
        )
        .unwrap();
        drop(held);

        assert_eq!(kept, 1);
        assert!(
            logs.join("held.log").exists(),
            "an open log is never removed"
        );
        assert!(!logs.join("free.log").exists());
    }

    #[test]
    fn whitelisted_child_survives_contents_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        fs::create_dir_all(cache.join("keep/inner")).unwrap();
        fs::write(cache.join("keep/inner/important"), [1_u8; 4_096]).unwrap();
        fs::create_dir(cache.join("parent")).unwrap();
        fs::write(cache.join("parent/protected"), [2_u8; 4_096]).unwrap();
        fs::write(cache.join("parent/junk"), [3_u8; 4_096]).unwrap();
        fs::write(cache.join("junk.bin"), [4_u8; 8_192]).unwrap();
        let whitelist = Whitelist::parse("cache/keep\ncache/parent/protected\n", temp.path());
        let mut entry = scan_cache(
            &test_spec(temp.path(), &cache, CacheTarget::DirectoryContents),
            false,
        );

        let outcome = clean_cache(&mut entry, std::slice::from_ref(&cache), false, &whitelist);

        assert!(matches!(
            outcome,
            CleanupOutcome::Cleared {
                method: CleanupMethod::ExactPathKeeping { kept: 2 },
                ..
            }
        ));
        assert!(cache.join("keep/inner/important").exists());
        assert!(cache.join("parent/protected").exists());
        assert!(!cache.join("junk.bin").exists());
    }

    #[test]
    fn whitelisted_file_in_an_aged_directory_survives() {
        let temp = tempfile::tempdir().unwrap();
        let logs = temp.path().join("logs");
        fs::create_dir(&logs).unwrap();
        fs::write(logs.join("keep.log"), [1_u8; 4_096]).unwrap();
        fs::write(logs.join("old.log"), [2_u8; 4_096]).unwrap();
        backdate(&logs.join("keep.log"), 30);
        backdate(&logs.join("old.log"), 30);
        let whitelist = Whitelist::parse("logs/keep.log\n", temp.path());

        let kept = clear_aged_contents(
            &logs,
            std::slice::from_ref(&logs),
            temp.path(),
            7,
            None,
            &whitelist,
        )
        .unwrap();

        assert_eq!(kept, 1);
        assert!(logs.join("keep.log").exists());
        assert!(!logs.join("old.log").exists());
    }

    #[test]
    fn native_commands_never_run_when_the_whitelist_names_something_inside() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("Library/Caches/pip");
        fs::create_dir_all(cache.join("wheels")).unwrap();
        fs::write(cache.join("wheels/pinned.whl"), [1_u8; 64]).unwrap();
        fs::write(cache.join("http.bin"), [2_u8; 64]).unwrap();
        let spec = CacheSpec {
            label: "pip cache",
            ..test_spec(temp.path(), &cache, CacheTarget::DirectoryContents)
        };
        let whitelist = Whitelist::parse("Library/Caches/pip/wheels\n", temp.path());
        let method =
            clear_with_native_first(&spec, std::slice::from_ref(&cache), None, &whitelist).unwrap();
        assert!(matches!(
            method,
            CleanupMethod::NativeSkippedForWhitelist { kept: 1, .. }
        ));
        assert!(cache.join("wheels/pinned.whl").exists());
        assert!(!cache.join("http.bin").exists());
    }

    #[test]
    fn safety_doc_native_table_matches_the_commands_that_run() {
        let doc = include_str!("../docs/SAFETY.md");
        let section = doc
            .split("### Native-command cleanup")
            .nth(1)
            .expect("docs/SAFETY.md documents native commands");
        let documented: HashSet<&str> = section
            .lines()
            .skip_while(|line| !line.starts_with("| ---"))
            .skip(1)
            .take_while(|line| line.starts_with('|'))
            .filter_map(|line| line.split('`').nth(1))
            .collect();
        let labels = [
            "pip cache",
            "Go build cache",
            "uv package cache",
            "Yarn package cache",
            "Playwright browsers",
            "Cypress runtimes",
            "Hugging Face models",
        ];
        let actual: HashSet<&str> = labels
            .iter()
            .filter_map(|label| native_command_name(label))
            .collect();
        assert_eq!(actual.len(), labels.len());
        assert_eq!(
            documented, actual,
            "docs/SAFETY.md table must list exactly the commands that run"
        );
    }

    #[test]
    fn exact_paths_with_a_whitelisted_descendant_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let stale = temp.path().join("stale");
        fs::create_dir(&stale).unwrap();
        fs::write(stale.join("keep"), [1_u8; 64]).unwrap();
        let whitelist = Whitelist::parse("stale/keep\n", temp.path());
        let error = remove_exact_path(
            &stale,
            std::slice::from_ref(&stale),
            temp.path(),
            None,
            &whitelist,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(stale.join("keep").exists());
    }

    #[test]
    fn process_check_errors_block_cleanup() {
        assert_eq!(related_process_state("["), ProcessState::CheckFailed);
        assert!(process_is_running("["));
    }

    #[test]
    fn parses_account_home_paths_containing_spaces() {
        assert_eq!(
            parse_account_home("NFSHomeDirectory: /Users/Example User\n"),
            Some(PathBuf::from("/Users/Example User"))
        );
        assert_eq!(parse_account_home("NFSHomeDirectory:\n"), None);
    }

    #[test]
    fn cleanup_errors_warn_that_removal_may_be_partial() {
        let error = cleanup_progress_error(io::Error::other("test failure"), 2);

        assert!(error.to_string().contains("cleanup may be partial"));
        assert!(error.to_string().contains("removed 2 complete"));
    }

    #[test]
    fn builds_cache_paths_relative_to_a_home_directory() {
        let root = Path::new("/Users/tester");
        let specs = cache_specs(root);

        assert_eq!(specs[0].home, root);
        assert_eq!(specs[0].path, root.join(".Trash"));
        assert_eq!(
            specs
                .iter()
                .find(|spec| spec.label == "npm package cache")
                .unwrap()
                .path,
            root.join(".npm/_cacache")
        );
        assert_eq!(
            specs
                .iter()
                .find(|spec| spec.label == "Safari cache")
                .unwrap()
                .tier,
            CacheTier::Routine
        );
        assert_eq!(
            specs
                .iter()
                .find(|spec| spec.label == "SwiftPM cache")
                .unwrap()
                .tier,
            CacheTier::Reinstallable
        );
        assert_eq!(
            specs
                .iter()
                .find(|spec| spec.label == "CocoaPods cache")
                .unwrap()
                .path,
            root.join("Library/Caches/CocoaPods")
        );
        assert_eq!(
            specs
                .iter()
                .find(|spec| spec.label == "Chrome DevTools MCP profile")
                .unwrap()
                .tier,
            CacheTier::ReviewOnly
        );
    }

    #[test]
    fn home_scan_includes_aged_directories_with_retention_windows() {
        let root = Path::new("/Users/tester");
        let specs = cache_specs(root);

        let logs = specs.iter().find(|spec| spec.label == "User logs").unwrap();
        assert_eq!(logs.path, root.join("Library/Logs"));
        assert_eq!(logs.target, CacheTarget::AgedContents { min_age_days: 7 });
        assert_eq!(
            specs
                .iter()
                .find(|spec| spec.label == "Saved app state")
                .unwrap()
                .target,
            CacheTarget::AgedContents { min_age_days: 30 }
        );
        assert_eq!(
            specs
                .iter()
                .find(|spec| spec.label == "Crash reports")
                .unwrap()
                .target,
            CacheTarget::AgedContents { min_age_days: 30 }
        );
        assert_eq!(
            cleanup_plan(logs),
            "Remove only direct entries untouched for at least 7 day(s) from the exact allowlisted directory; newer entries stay."
        );
    }

    #[test]
    fn documents_every_supported_native_first_strategy() {
        let expected = [
            ("pip cache", "python3 -m pip cache purge"),
            ("Go build cache", "go clean -cache -testcache -fuzzcache"),
            ("uv package cache", "uv cache clean"),
            ("Yarn package cache", "yarn cache clean"),
            ("Playwright browsers", "playwright uninstall --all"),
            ("Cypress runtimes", "cypress cache clear"),
            ("Hugging Face models", "hf cache prune --yes"),
        ];

        for (label, command) in expected {
            assert_eq!(native_command_name(label), Some(command));
        }
        assert_eq!(native_command_name("Homebrew cache"), None);
        assert_eq!(native_command_name("Codex runtimes"), None);
        assert_eq!(native_command_name("npm package cache"), None);
        assert_eq!(native_command_name("npm npx packages"), None);
        assert_eq!(native_command_name("SwiftPM cache"), None);
        assert_eq!(native_command_name("CocoaPods cache"), None);
        assert_eq!(native_command_name("Simulator devices"), None);
        assert_eq!(native_command_name("OrbStack data"), None);
    }

    #[test]
    fn unavailable_native_tool_falls_back_to_the_exact_allowlisted_path() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join(".cache/huggingface/hub");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("model.bin"), [1_u8; 8_192]).unwrap();
        let spec = CacheSpec {
            home: temp.path().to_path_buf(),
            path: cache.clone(),
            label: "Hugging Face models",
            tier: CacheTier::Reinstallable,
            process_pattern: "",
            note: "test model data",
            target: CacheTarget::DirectoryContents,
        };

        let method =
            clear_with_native_first(&spec, std::slice::from_ref(&cache), None, &no_whitelist())
                .unwrap();

        assert_eq!(
            method,
            CleanupMethod::NativeUnavailableThenExactPath {
                command: "hf cache prune --yes"
            }
        );
        assert!(cache.is_dir());
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);

        // The same label at a different path never gets the vendor command.
        let elsewhere = temp.path().join("hub");
        fs::create_dir(&elsewhere).unwrap();
        let moved = CacheSpec {
            path: elsewhere.clone(),
            ..spec
        };
        assert_eq!(native_command_name_for(&moved), None);
    }

    #[test]
    fn native_command_runner_reports_success_and_failure_without_a_shell() {
        let success = NativeCleanupCommand {
            display: "true",
            program: PathBuf::from("/usr/bin/true"),
            args: vec![],
            environment: vec![],
        };
        let failure = NativeCleanupCommand {
            display: "false",
            program: PathBuf::from("/usr/bin/false"),
            args: vec![],
            environment: vec![],
        };

        assert!(matches!(
            run_native_cleanup(&success, Path::new("/")),
            NativeCommandResult::Succeeded
        ));
        assert!(matches!(
            run_native_cleanup(&failure, Path::new("/")),
            NativeCommandResult::Failed(_)
        ));
    }

    #[test]
    fn mounted_volume_scan_targets_real_volume_waste() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("Users/tester");
        let volume = temp.path().join("DATA");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir(&volume).unwrap();

        let specs = scan_specs(&volume, &home);

        assert!(
            specs
                .iter()
                .any(|spec| spec.path == volume.join("#recycle"))
        );
        assert!(
            specs
                .iter()
                .all(|spec| spec.path != volume.join("Library/Caches/pip"))
        );
    }

    #[test]
    fn nas_recycle_bin_is_review_only() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("Users/tester");
        let volume = temp.path().join("DATA");
        let recycle = volume.join("#recycle");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&recycle).unwrap();
        fs::File::create(recycle.join("discarded.bin"))
            .unwrap()
            .write_all(&[1; 8_192])
            .unwrap();

        let spec = scan_specs(&volume, &home)
            .into_iter()
            .find(|spec| spec.path == recycle)
            .unwrap();
        let entry = scan_cache(&spec, false);

        assert_eq!(entry.spec.label, "NAS recycle bin");
        assert!(entry.spec.note.contains("other users"));
        assert_eq!(entry.status, CacheStatus::Review, "never routine cleanup");
        assert!(entry.size_kb > 0);
    }

    #[test]
    fn home_scan_includes_user_waste_instead_of_volume_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();

        let specs = scan_specs(&home, &home);

        assert!(specs.iter().any(|spec| spec.path == home.join(".Trash")));
        assert!(specs.iter().all(|spec| spec.path != home.join("#recycle")));
    }

    #[test]
    fn directly_selected_home_does_not_duplicate_user_trash() {
        let temp = tempfile::tempdir().unwrap();
        let account_home = temp.path().join("Users/current");
        let selected_home = temp.path().join("Users/other");
        fs::create_dir_all(&account_home).unwrap();
        fs::create_dir_all(selected_home.join("Library")).unwrap();
        fs::create_dir(selected_home.join(".Trash")).unwrap();

        let specs = scan_specs(&selected_home, &account_home);
        let trash_count = specs
            .iter()
            .filter(|spec| spec.path == selected_home.join(".Trash"))
            .count();

        assert_eq!(trash_count, 1);
        assert!(
            specs
                .iter()
                .all(|spec| spec.path != selected_home.join("#recycle"))
        );
    }

    #[test]
    fn home_scan_includes_large_app_managed_storage_as_review_only() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();

        let specs = scan_specs(&home, &home);
        let xcode_support = specs
            .iter()
            .find(|spec| spec.label == "Xcode device support")
            .unwrap();
        let xcode_archives = specs
            .iter()
            .find(|spec| spec.label == "Xcode archives")
            .unwrap();
        let orbstack = specs
            .iter()
            .find(|spec| spec.label == "OrbStack data")
            .unwrap();

        assert_eq!(xcode_support.tier, CacheTier::ReviewOnly);
        assert_eq!(
            xcode_support.path,
            home.join("Library/Developer/Xcode/iOS DeviceSupport")
        );
        assert_eq!(xcode_archives.tier, CacheTier::ReviewOnly);
        assert_eq!(
            xcode_archives.path,
            home.join("Library/Developer/Xcode/Archives")
        );
        assert_eq!(orbstack.tier, CacheTier::ReviewOnly);
    }

    #[test]
    fn review_only_storage_is_refused_by_ordinary_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let managed = temp.path().join("managed");
        fs::create_dir(&managed).unwrap();
        fs::File::create(managed.join("keep-me"))
            .unwrap()
            .write_all(&[1; 8_192])
            .unwrap();
        let mut entry = CacheEntry {
            spec: CacheSpec {
                home: temp.path().to_path_buf(),
                path: managed.clone(),
                label: "Managed data",
                tier: CacheTier::ReviewOnly,
                process_pattern: "",
                note: "must be managed by its app",
                target: CacheTarget::DirectoryContents,
            },
            status: CacheStatus::Review,
            size_kb: 8,
            outcome: None,
            identity: None,
        };

        assert_eq!(status_for(&entry.spec, true), CacheStatus::Review);
        let outcome = clean_cache(
            &mut entry,
            std::slice::from_ref(&managed),
            true,
            &no_whitelist(),
        );

        assert!(matches!(outcome, CleanupOutcome::SafetySkipped(_)));
        assert!(managed.join("keep-me").exists());
        assert_eq!(entry.status, CacheStatus::Review);
    }

    #[test]
    fn explicitly_confirmed_review_cleanup_clears_contents_but_keeps_directory() {
        let temp = tempfile::tempdir().unwrap();
        let managed = temp.path().join("managed");
        fs::create_dir(&managed).unwrap();
        fs::File::create(managed.join("delete-me"))
            .unwrap()
            .write_all(&[1; 8_192])
            .unwrap();
        let mut entry = CacheEntry {
            spec: CacheSpec {
                home: temp.path().to_path_buf(),
                path: managed.clone(),
                label: "Managed data",
                tier: CacheTier::ReviewOnly,
                process_pattern: "",
                note: "contains app state",
                target: CacheTarget::DirectoryContents,
            },
            status: CacheStatus::Review,
            size_kb: directory_kb(&managed),
            outcome: None,
            identity: PathIdentity::capture(&managed),
        };

        let outcome =
            clean_review_data(&mut entry, std::slice::from_ref(&managed), &no_whitelist());

        assert!(matches!(
            outcome,
            CleanupOutcome::Cleared { removed_kb, .. } if removed_kb > 0
        ));
        assert!(managed.is_dir());
        assert_eq!(fs::read_dir(&managed).unwrap().count(), 0);
        assert_eq!(entry.size_kb, 0);
    }

    #[test]
    fn discovers_home_and_mounted_volume_directories() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let volumes = temp.path().join("Volumes");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&volumes).unwrap();
        fs::create_dir(volumes.join("Work Drive")).unwrap();
        fs::File::create(volumes.join("not-a-volume")).unwrap();

        let locations = scan_locations_in(&home, &volumes, false);

        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].path, home.canonicalize().unwrap());
        assert_eq!(locations[0].kind, ScanLocationKind::Local);
        assert_eq!(locations[1].label, "Work Drive");
        assert_eq!(locations[1].kind, ScanLocationKind::Local);
        assert_eq!(
            locations[1].path,
            volumes.join("Work Drive").canonicalize().unwrap()
        );
    }

    #[test]
    fn classifies_local_usb_and_network_storage() {
        let local_plist = "<key>BusProtocol</key><string>Apple Fabric</string>";
        let usb_plist = "<key>BusProtocol</key>\n<string>USB</string>";

        assert_eq!(
            classify_scan_location(Some(local_plist), Some("apfs")),
            ScanLocationKind::Local
        );
        assert_eq!(
            classify_scan_location(Some(usb_plist), Some("apfs")),
            ScanLocationKind::Usb
        );
        assert_eq!(
            classify_scan_location(Some(usb_plist), Some("smbfs")),
            ScanLocationKind::Network
        );
        assert_eq!(
            classify_scan_location(None, Some("nfs")),
            ScanLocationKind::Network
        );
    }

    #[test]
    fn parses_filesystem_types_for_mount_points_with_spaces() {
        let mounts = concat!(
            "/dev/disk3s3s1 on / (apfs, sealed, local)\n",
            "/dev/disk7s1 on /Volumes/USB Drive (exfat, local, noowners)\n",
            "//user@nas/Data on /Volumes/Team Share (smbfs, nodev, nosuid)\n",
        );

        assert_eq!(
            mounted_filesystem_from_output(mounts, Path::new("/")),
            Some("apfs".into())
        );
        assert_eq!(
            mounted_filesystem_from_output(mounts, Path::new("/Volumes/USB Drive")),
            Some("exfat".into())
        );
        assert_eq!(
            mounted_filesystem_from_output(mounts, Path::new("/Volumes/Team Share")),
            Some("smbfs".into())
        );
        assert_eq!(
            mounted_filesystem_from_output(mounts, Path::new("/Volumes/Team Share/Projects/x")),
            Some("smbfs".into()),
            "a folder inside a share is network storage"
        );
        assert_eq!(
            mounted_filesystem_from_output(mounts, Path::new("/Volumes/Team Shared")),
            Some("apfs".into()),
            "a sibling with a longer name is not inside the share"
        );
    }

    #[test]
    fn network_volumes_make_every_volume_rule_review_only() {
        let temp = tempfile::tempdir().unwrap();
        for spec in volume_specs_for(temp.path(), ScanLocationKind::Network) {
            assert_eq!(spec.tier, CacheTier::ReviewOnly, "{}", spec.label);
        }
        let local = volume_specs_for(temp.path(), ScanLocationKind::Local);
        let tier = |name: &str| {
            local
                .iter()
                .find(|spec| spec.path.ends_with(name))
                .map(|spec| spec.tier)
        };
        assert_eq!(tier(".Trash"), Some(CacheTier::Routine));
        assert_eq!(tier(".TemporaryItems"), Some(CacheTier::Routine));
        assert_eq!(tier("#recycle"), Some(CacheTier::ReviewOnly));
        assert_eq!(tier("$RECYCLE.BIN"), Some(CacheTier::ReviewOnly));
    }

    #[cfg(unix)]
    #[test]
    fn scan_root_validation_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("actual");
        let linked = temp.path().join("linked");
        fs::create_dir(&actual).unwrap();
        symlink(&actual, &linked).unwrap();

        assert!(validate_scan_root(&linked).is_err());
        assert_eq!(
            validate_scan_root(&actual).unwrap(),
            actual.canonicalize().unwrap()
        );
    }
}
