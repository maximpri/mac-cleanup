use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const DF_COMMAND: &str = "/bin/df";
const DSCL_COMMAND: &str = "/usr/bin/dscl";
const DU_COMMAND: &str = "/usr/bin/du";
const ID_COMMAND: &str = "/usr/bin/id";
const PGREP_COMMAND: &str = "/usr/bin/pgrep";
const DISKUTIL_COMMAND: &str = "/usr/sbin/diskutil";
const MOUNT_COMMAND: &str = "/sbin/mount";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTier {
    Routine,
    Reinstallable,
    /// Large app-managed data that is useful for diagnosis but must never be
    /// deleted as though it were a cache.
    ReviewOnly,
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
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Self::Ready => "All current safeguards passed.",
            Self::Optional => "Enable reinstallable items to select this cache.",
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
        }
    }
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub spec: CacheSpec,
    pub status: CacheStatus,
    pub size_kb: u64,
    pub outcome: Option<CleanupOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOutcome {
    Cleared(u64),
    SafetySkipped(String),
    Failed { error: String, removed_kb: u64 },
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

pub fn cache_specs(root: &Path) -> Vec<CacheSpec> {
    let definitions = [
        (
            ".Trash",
            "Trash",
            CacheTier::Routine,
            "",
            "files already moved to Trash",
        ),
        (
            "Library/Caches/pip",
            "pip cache",
            CacheTier::Routine,
            "[p]ip(3)? (install|download|cache)",
            "downloaded Python packages",
        ),
        (
            "Library/Caches/node-gyp",
            "node-gyp cache",
            CacheTier::Routine,
            "[n]ode-gyp",
            "downloaded Node build files",
        ),
        (
            "Library/Caches/Homebrew",
            "Homebrew cache",
            CacheTier::Routine,
            "[b]rew (install|upgrade|update|cleanup)",
            "downloaded Homebrew files",
        ),
        (
            "Library/Caches/com.apple.python",
            "Python cache",
            CacheTier::Routine,
            "[p]ython(3)? .*pip",
            "Python-generated cache files",
        ),
        (
            "Library/Caches/tradingview-desktop-updater",
            "TradingView updater",
            CacheTier::Routine,
            "/TradingView.app/|[t]radingview-desktop-updater",
            "obsolete updater downloads",
        ),
        (
            "Library/Caches/@zcodedesktop-updater",
            "ZCode updater",
            CacheTier::Routine,
            "/Zed.app/|/[Zz][Cc]ode.app/|@[z]codedesktop-updater",
            "obsolete updater downloads",
        ),
        (
            "Library/Caches/ru.keepcoder.Telegram",
            "Telegram cache",
            CacheTier::Routine,
            "/Telegram.app/Contents/MacOS/Telegram",
            "messages remain; media thumbnails reload",
        ),
        (
            "Library/Caches/Google",
            "Google app cache",
            CacheTier::Routine,
            "/Google Chrome.app/|/Google Drive.app/|[k]eystone.*Google",
            "browser and app cache data reloads",
        ),
        (
            "Library/Caches/com.apple.Safari",
            "Safari cache",
            CacheTier::Routine,
            "/Safari.app/Contents/MacOS/Safari",
            "website cache reloads; profiles and browsing data remain",
        ),
        (
            "Library/Caches/Firefox",
            "Firefox cache",
            CacheTier::Routine,
            "/Firefox.app/Contents/MacOS/firefox",
            "website cache reloads; profiles and browsing data remain",
        ),
        (
            "Library/Caches/Adobe",
            "Adobe app cache",
            CacheTier::Routine,
            "[A]dobe|[P]hotoshop|[I]llustrator|[P]remiere|[A]fter Effects|[L]ightroom",
            "Adobe applications regenerate cached files",
        ),
        (
            ".npm/_cacache",
            "npm package cache",
            CacheTier::Routine,
            "[n]pm |[n]px ",
            "downloaded npm packages",
        ),
        (
            ".cache/opencode",
            "OpenCode cache",
            CacheTier::Routine,
            "[o]pencode",
            "OpenCode regenerates this cache",
        ),
        (
            "Library/Caches/go-build",
            "Go build cache",
            CacheTier::Routine,
            "[g]o (build|test|install|run)",
            "compiled Go build artifacts",
        ),
        (
            ".cache/uv",
            "uv package cache",
            CacheTier::Routine,
            "[u]v (add|build|cache|pip|run|sync)",
            "downloaded Python packages and build artifacts",
        ),
        (
            "Library/Caches/Yarn",
            "Yarn package cache",
            CacheTier::Routine,
            "[y]arn ",
            "downloaded JavaScript packages",
        ),
        (
            "Library/Developer/Xcode/DerivedData",
            "Xcode derived data",
            CacheTier::Routine,
            "/Xcode.app/|[x]codebuild",
            "indexes and build products will be regenerated",
        ),
        (
            "Library/Developer/CoreSimulator/Caches",
            "Simulator cache",
            CacheTier::Routine,
            "[S]imulator.app/|[C]oreSimulator",
            "simulator cache data will be regenerated",
        ),
        (
            "Library/Caches/org.swift.swiftpm",
            "SwiftPM cache",
            CacheTier::Reinstallable,
            "[s]wift (build|package|run|test)|[s]wift-(build|package)",
            "package metadata and downloads will be regenerated",
        ),
        (
            "Library/Caches/ms-playwright",
            "Playwright browsers",
            CacheTier::Reinstallable,
            "[p]laywright|[m]s-playwright",
            "browser binaries download again",
        ),
        (
            "Library/Caches/ms-playwright-go",
            "Playwright Go browsers",
            CacheTier::Reinstallable,
            "[p]laywright|[m]s-playwright-go",
            "browser binaries download again",
        ),
        (
            ".npm/_npx",
            "npm npx packages",
            CacheTier::Reinstallable,
            "[n]pm |[n]px ",
            "temporary npx packages install again",
        ),
        (
            ".cache/chrome-devtools-mcp",
            "Chrome DevTools MCP",
            CacheTier::Reinstallable,
            "[c]hrome-devtools-mcp",
            "browser and runtime downloads may return",
        ),
        (
            ".cache/codex-runtimes",
            "Codex runtimes",
            CacheTier::Reinstallable,
            "/Codex.app/|[c]odex-runtimes",
            "Codex runtime downloads may return",
        ),
        (
            ".gradle/caches",
            "Gradle caches",
            CacheTier::Reinstallable,
            "[g]radle|[G]radleDaemon",
            "dependencies and build tooling download again",
        ),
        (
            "Library/Caches/CocoaPods",
            "CocoaPods cache",
            CacheTier::Reinstallable,
            "[/ ]pod (cache|install|repo|update)",
            "downloaded Pods install again",
        ),
        (
            "Library/Caches/Cypress",
            "Cypress runtimes",
            CacheTier::Reinstallable,
            "[c]ypress",
            "Cypress application binaries download again",
        ),
        (
            ".cache/huggingface/hub",
            "Hugging Face models",
            CacheTier::Reinstallable,
            "[h]uggingface|[t]ransformers",
            "models and datasets download again",
        ),
        (
            "Library/Developer/Xcode/iOS DeviceSupport",
            "Xcode device support",
            CacheTier::ReviewOnly,
            "/Xcode.app/|[x]codebuild",
            "old iOS support files; review Developer storage in System Settings",
        ),
        (
            "Library/Developer/Xcode/Archives",
            "Xcode archives",
            CacheTier::ReviewOnly,
            "/Xcode.app/|[x]codebuild",
            "signed build archives may be needed for distribution or symbolication",
        ),
        (
            "Library/Developer/CoreSimulator/Devices",
            "Simulator devices",
            CacheTier::ReviewOnly,
            "[S]imulator.app/|[C]oreSimulator",
            "simulators may contain apps and data; remove unneeded devices in Xcode",
        ),
        (
            "Library/Application Support/Cursor/User",
            "Cursor user data",
            CacheTier::ReviewOnly,
            "/Cursor.app/|[C]ursor Helper",
            "settings, workspace state, and history; review inside Cursor",
        ),
        (
            "Library/Group Containers/HUAQ24HBR6.dev.orbstack/data",
            "OrbStack data",
            CacheTier::ReviewOnly,
            "/OrbStack.app/|[o]rbstack",
            "containers, images, machines, and volumes; reclaim through OrbStack",
        ),
        (
            "Library/Group Containers/6N38VWS5BX.ru.keepcoder.Telegram/stable",
            "Telegram local data",
            CacheTier::ReviewOnly,
            "/Telegram.app/Contents/MacOS/Telegram",
            "local account and media data; use Telegram's Storage Usage controls",
        ),
    ];

    definitions
        .into_iter()
        .map(|(relative, label, tier, process_pattern, note)| CacheSpec {
            home: root.to_path_buf(),
            path: root.join(relative),
            label,
            tier,
            process_pattern,
            note,
        })
        .collect()
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
    let definitions = [
        (
            ".Trash",
            "Volume Trash",
            "files already moved to this volume's Trash",
        ),
        (
            "#recycle",
            "NAS recycle bin",
            "server-managed deleted files; this is separate from Finder Trash",
        ),
        (
            "@Recycle",
            "NAS recycle bin",
            "server-managed deleted files; this is separate from Finder Trash",
        ),
        (
            "$RECYCLE.BIN",
            "Windows recycle bin",
            "Windows-managed deleted files; this is separate from Finder Trash",
        ),
        (
            ".TemporaryItems",
            "Temporary items",
            "temporary files left on this volume",
        ),
    ];

    let mut specs: Vec<_> = definitions
        .into_iter()
        .map(|(relative, label, note)| CacheSpec {
            home: root.to_path_buf(),
            path: root.join(relative),
            label,
            tier: CacheTier::Routine,
            process_pattern: "",
            note,
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
                tier: CacheTier::Routine,
                process_pattern: "",
                note: "files already moved to this volume's Trash",
            });
        }
    }

    specs
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
    let size_kb = if matches!(
        status,
        CacheStatus::Missing | CacheStatus::ScanError | CacheStatus::Symlink | CacheStatus::Invalid
    ) {
        0
    } else {
        match try_directory_kb(&spec.path) {
            Ok(size_kb) => size_kb,
            Err(_) => {
                status = CacheStatus::ScanError;
                0
            }
        }
    };

    CacheEntry {
        spec: spec.clone(),
        status,
        size_kb,
        outcome: None,
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
    } else if !metadata.is_dir() {
        CacheStatus::Invalid
    } else if fs::read_dir(&spec.path).is_err() {
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

pub fn clean_cache(
    entry: &mut CacheEntry,
    allowlist: &[PathBuf],
    include_reinstallable: bool,
) -> CleanupOutcome {
    if entry.spec.tier == CacheTier::ReviewOnly {
        let outcome = CleanupOutcome::SafetySkipped(
            "review-only storage is excluded from ordinary cleanup".into(),
        );
        entry.status = CacheStatus::Review;
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

    let size_before = match try_directory_kb(&entry.spec.path) {
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

    let outcome = match clear_directory_contents(&entry.spec.path, allowlist, &entry.spec.home) {
        Ok(()) => {
            let size_after = directory_kb(&entry.spec.path);
            let removed = size_before.saturating_sub(size_after);
            entry.size_kb = size_after;
            CleanupOutcome::Cleared(removed)
        }
        Err(error) => {
            let size_after = try_directory_kb(&entry.spec.path).unwrap_or(size_before);
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
pub fn clean_review_data(entry: &mut CacheEntry, allowlist: &[PathBuf]) -> CleanupOutcome {
    if entry.spec.tier != CacheTier::ReviewOnly {
        let outcome = CleanupOutcome::SafetySkipped(
            "advanced review deletion only accepts review-only storage".into(),
        );
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

    let size_before = match try_directory_kb(&entry.spec.path) {
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

    let outcome = match clear_directory_contents(&entry.spec.path, allowlist, &entry.spec.home) {
        Ok(()) => {
            let size_after = directory_kb(&entry.spec.path);
            entry.size_kb = size_after;
            CleanupOutcome::Cleared(size_before.saturating_sub(size_after))
        }
        Err(error) => {
            let size_after = try_directory_kb(&entry.spec.path).unwrap_or(size_before);
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

pub fn clear_directory_contents(
    target: &Path,
    allowlist: &[PathBuf],
    root: &Path,
) -> io::Result<()> {
    if !allowlist.iter().any(|allowed| allowed == target) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("path is not on the exact allowlist: {}", target.display()),
        ));
    }

    let metadata = fs::symlink_metadata(target)?;
    if metadata.file_type().is_symlink()
        || has_symlink_component_below(target, root)
        || !metadata.is_dir()
    {
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

    for (completely_removed, item) in fs::read_dir(target)?.enumerate() {
        let path = item
            .map_err(|error| cleanup_progress_error(error, completely_removed))?
            .path();
        let item_metadata = fs::symlink_metadata(&path)
            .map_err(|error| cleanup_progress_error(error, completely_removed))?;
        let result = if item_metadata.is_dir() && !item_metadata.file_type().is_symlink() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        result.map_err(|error| cleanup_progress_error(error, completely_removed))?;
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

pub fn directory_kb(path: &Path) -> u64 {
    try_directory_kb(path).unwrap_or(0)
}

fn try_directory_kb(path: &Path) -> io::Result<u64> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not a real directory: {}", path.display()),
        ));
    }

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

fn has_symlink_component_below(path: &Path, root: &Path) -> bool {
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

fn mounted_filesystem_from_output(output: &str, path: &Path) -> Option<String> {
    let expected_mount = path.to_string_lossy();
    output.lines().find_map(|line| {
        let (mount_description, options) = line.rsplit_once(" (")?;
        let (_, mount_point) = mount_description.rsplit_once(" on ")?;
        if mount_point != expected_mount {
            return None;
        }
        options
            .strip_suffix(')')?
            .split(',')
            .next()
            .map(|filesystem| filesystem.trim().to_string())
    })
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

fn effective_user_id() -> Option<u32> {
    Command::new(ID_COMMAND)
        .arg("-u")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| value.trim().parse().ok())
}

fn account_home() -> Option<PathBuf> {
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
    use std::io::Write;

    #[test]
    fn formats_sizes() {
        assert_eq!(format_kb(12), "12 KiB");
        assert_eq!(format_kb(1_536), "1.5 MiB");
        assert_eq!(format_kb(2_097_152), "2.0 GiB");
    }

    #[test]
    fn refuses_paths_outside_allowlist() {
        let temp = tempfile::tempdir().unwrap();
        let error = clear_directory_contents(temp.path(), &[], temp.path()).unwrap_err();
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

        let error =
            clear_directory_contents(&escaping_path, std::slice::from_ref(&escaping_path), &root)
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

        clear_directory_contents(&cache, std::slice::from_ref(&cache), temp.path()).unwrap();

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
            },
            status: CacheStatus::Ready,
            size_kb: size_before,
            outcome: None,
        };

        let outcome = clean_cache(&mut entry, &[], false);

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

        let error = clear_directory_contents(&linked, std::slice::from_ref(&linked), temp.path())
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

        clear_directory_contents(&cache, std::slice::from_ref(&cache), temp.path()).unwrap();

        assert!(outside.join("keep-me").exists());
        assert_eq!(fs::read_dir(cache).unwrap().count(), 0);
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
    fn recycle_bin_is_reported_as_removable_data() {
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
        assert!(entry.spec.note.contains("separate from Finder Trash"));
        assert_eq!(entry.status, CacheStatus::Ready);
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
            },
            status: CacheStatus::Review,
            size_kb: 8,
            outcome: None,
        };

        assert_eq!(status_for(&entry.spec, true), CacheStatus::Review);
        let outcome = clean_cache(&mut entry, std::slice::from_ref(&managed), true);

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
            },
            status: CacheStatus::Review,
            size_kb: directory_kb(&managed),
            outcome: None,
        };

        let outcome = clean_review_data(&mut entry, std::slice::from_ref(&managed));

        assert!(matches!(outcome, CleanupOutcome::Cleared(kb) if kb > 0));
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
