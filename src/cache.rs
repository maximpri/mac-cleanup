use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTier {
    Routine,
    Reinstallable,
}

#[derive(Debug, Clone)]
pub struct CacheSpec {
    /// Safety boundary for this candidate (the home directory by default, or a selected volume).
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
    InUse,
    Symlink,
    Invalid,
    Missing,
}

impl CacheStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "READY",
            Self::Optional => "OPTIONAL",
            Self::InUse => "IN USE",
            Self::Symlink => "SYMLINK",
            Self::Invalid => "INVALID",
            Self::Missing => "MISSING",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Self::Ready => "All current safeguards passed.",
            Self::Optional => "Enable reinstallable items to select this cache.",
            Self::InUse => "A related application or package manager is running.",
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
    Failed(String),
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
}

pub fn cache_specs(root: &Path) -> Vec<CacheSpec> {
    let definitions = [
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

pub fn scan_cache(spec: &CacheSpec, include_reinstallable: bool) -> CacheEntry {
    let status = status_for(spec, include_reinstallable);
    let size_kb = if matches!(
        status,
        CacheStatus::Missing | CacheStatus::Symlink | CacheStatus::Invalid
    ) {
        0
    } else {
        directory_kb(&spec.path)
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
    } else if process_is_running(spec.process_pattern) {
        CacheStatus::InUse
    } else if spec.tier == CacheTier::Reinstallable && !include_reinstallable {
        CacheStatus::Optional
    } else {
        CacheStatus::Ready
    }
}

pub fn clean_cache(
    entry: &mut CacheEntry,
    allowlist: &[PathBuf],
    include_reinstallable: bool,
) -> CleanupOutcome {
    let current_status = status_for(&entry.spec, include_reinstallable);
    if current_status != CacheStatus::Ready {
        let outcome =
            CleanupOutcome::SafetySkipped(format!("status changed to {}", current_status.label()));
        entry.status = current_status;
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    let size_before = directory_kb(&entry.spec.path);
    if size_before == 0 {
        let outcome = CleanupOutcome::SafetySkipped("already empty".into());
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    if process_is_running(entry.spec.process_pattern) {
        let outcome = CleanupOutcome::SafetySkipped("a related process just started".into());
        entry.status = CacheStatus::InUse;
        entry.outcome = Some(outcome.clone());
        return outcome;
    }

    let outcome = match clear_directory_contents(&entry.spec.path, allowlist, &entry.spec.home) {
        Ok(()) => {
            let removed = size_before.saturating_sub(directory_kb(&entry.spec.path));
            entry.size_kb = directory_kb(&entry.spec.path);
            CleanupOutcome::Cleared(removed)
        }
        Err(error) => CleanupOutcome::Failed(error.to_string()),
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

    for item in fs::read_dir(target)? {
        let path = item?.path();
        let item_metadata = fs::symlink_metadata(&path)?;
        if item_metadata.is_dir() && !item_metadata.file_type().is_symlink() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn directory_kb(path: &Path) -> u64 {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
        .unwrap_or(true)
    {
        return 0;
    }

    Command::new("du")
        .args(["-sk"])
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|line| line.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
}

pub fn free_kb(root: &Path) -> u64 {
    Command::new("df")
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
    !pattern.is_empty()
        && Command::new("pgrep")
            .args(["-f", pattern])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
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

fn scan_locations_in(
    home: &Path,
    volumes_dir: &Path,
    include_startup_volume: bool,
) -> Vec<ScanLocation> {
    let mut locations = vec![ScanLocation {
        label: "Home directory".into(),
        path: home.to_path_buf(),
    }];

    if include_startup_volume {
        push_unique_location(
            &mut locations,
            ScanLocation {
                label: "Startup volume".into(),
                path: PathBuf::from("/"),
            },
        );
    }

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
                path,
            })
        })
        .collect::<Vec<_>>();
    mounted.sort_by_key(|location| location.label.to_lowercase());
    for location in mounted {
        push_unique_location(&mut locations, location);
    }
    locations
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
    let testing = env::var("MAC_CLEANUP_TESTING").is_ok_and(|value| value == "1");
    if !testing && env::consts::OS != "macos" {
        return Err("this application is intended for macOS".into());
    }

    if !testing && effective_user_id() == Some(0) {
        return Err("do not run this application with sudo or as root".into());
    }

    let home = if testing {
        env::var_os("MAC_CLEANUP_TEST_HOME")
            .or_else(|| env::var_os("HOME"))
            .map(PathBuf::from)
    } else {
        account_home().or_else(|| env::var_os("HOME").map(PathBuf::from))
    }
    .ok_or_else(|| "could not determine the current account's home directory".to_string())?;

    if home == Path::new("/") || !home.is_dir() {
        return Err(format!(
            "unsafe or invalid home directory: {}",
            home.display()
        ));
    }
    Ok(home)
}

fn effective_user_id() -> Option<u32> {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| value.trim().parse().ok())
}

fn account_home() -> Option<PathBuf> {
    let user = Command::new("id")
        .arg("-un")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())?;
    let user = user.trim();

    let output = Command::new("dscl")
        .args([".", "-read", &format!("/Users/{user}"), "NFSHomeDirectory"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    text.split_whitespace().nth(1).map(PathBuf::from)
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
    fn builds_cache_paths_relative_to_the_selected_root() {
        let root = Path::new("/Volumes/Archive");
        let specs = cache_specs(root);

        assert_eq!(specs[0].home, root);
        assert_eq!(specs[0].path, root.join("Library/Caches/pip"));
        assert_eq!(specs[8].path, root.join(".npm/_cacache"));
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
        assert_eq!(locations[0].path, home);
        assert_eq!(locations[1].label, "Work Drive");
        assert_eq!(
            locations[1].path,
            volumes.join("Work Drive").canonicalize().unwrap()
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
