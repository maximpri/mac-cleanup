// SPDX-License-Identifier: GPL-3.0-or-later
//! Installer images and packages left in Downloads and Desktop.
//!
//! Disk images (`.dmg`), packages (`.pkg`, `.mpkg`), Xcode archives (`.xip`),
//! and `.iso` files are usually needed once. This is a report only: Diskray
//! never deletes them. Each file is matched against installed applications by
//! name, so the report can say "Firefox is installed" instead of guessing that
//! an installer is unneeded. A file some process holds open, such as a
//! mounted disk image, is reported as in use; when that check cannot run,
//! nothing is claimed about it.

use serde::Serialize;
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const EXTENSIONS: [&str; 5] = ["dmg", "pkg", "mpkg", "xip", "iso"];
/// Folders searched under the home folder, one level deep at most.
const FOLDERS: [&str; 2] = ["Downloads", "Desktop"];
const MAX_ENTRIES: usize = 20_000;
const MIN_REPORT_KB: u64 = 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Installer {
    pub path: PathBuf,
    pub kind: &'static str,
    pub size_kb: u64,
    /// Days since the file last changed, usually since it was downloaded.
    pub age_days: u64,
    /// An installed application whose name matches this installer.
    pub installed_app: Option<PathBuf>,
    /// `Some(true)` when a process holds the file open (a mounted image);
    /// `None` when the open-file check could not run.
    pub in_use: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub installers: Vec<Installer>,
    pub searched: Vec<PathBuf>,
    /// Every searched folder was read within the entry limit.
    pub complete: bool,
}

impl Report {
    pub fn total_kb(&self) -> u64 {
        self.installers.iter().map(|item| item.size_kb).sum()
    }

    /// Installers whose application is already installed.
    pub fn redundant_kb(&self) -> u64 {
        self.installers
            .iter()
            .filter(|item| item.installed_app.is_some())
            .map(|item| item.size_kb)
            .sum()
    }

    /// One line per installer; `display` renders each path.
    pub fn lines(&self, display: impl Fn(&Path) -> String, limit: usize) -> Vec<String> {
        self.installers
            .iter()
            .take(limit)
            .map(|item| {
                let mut parts = vec![
                    display(&item.path),
                    item.kind.to_string(),
                    crate::cache::format_kb(item.size_kb),
                    age_label(item.age_days),
                ];
                if let Some(app) = &item.installed_app {
                    parts.push(format!(
                        "{} is installed",
                        app.file_stem()
                            .map(|stem| crate::ai::display_text(&stem.to_string_lossy()))
                            .unwrap_or_default()
                    ));
                }
                if item.in_use == Some(true) {
                    parts.push("in use (mounted or open)".into());
                }
                parts.join(" · ")
            })
            .collect()
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": "diskray.installers/1",
            "complete": self.complete,
            "searched": self.searched,
            "total_kb": self.total_kb(),
            "redundant_kb": self.redundant_kb(),
            "installers": self.installers,
            "deleted": false,
        })
    }
}

fn age_label(days: u64) -> String {
    match days {
        0 => "today".into(),
        1 => "1 day old".into(),
        2..=59 => format!("{days} days old"),
        60..=729 => format!("{} months old", days / 30),
        _ => format!("{} years old", days / 365),
    }
}

/// Search the home folder's Downloads and Desktop for installers.
pub fn find(home: &Path) -> Report {
    let apps = installed_apps(&[
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        home.join("Applications"),
    ]);
    let folders: Vec<PathBuf> = FOLDERS
        .iter()
        .map(|name| home.join(name))
        .filter(|path| is_real_directory(path))
        .collect();
    find_in(&folders, &apps, SystemTime::now())
}

/// The search itself, with the installed application list supplied.
pub fn find_in(folders: &[PathBuf], apps: &[PathBuf], now: SystemTime) -> Report {
    let mut report = Report {
        searched: folders.to_vec(),
        complete: true,
        ..Report::default()
    };
    let mut seen = 0;
    for folder in folders {
        let open = crate::cache::open_paths_under(folder).ok();
        let mut candidates = Vec::new();
        for entry in read_dir(folder) {
            seen += 1;
            if seen > MAX_ENTRIES {
                report.complete = false;
                break;
            }
            // One level of subfolders, such as Downloads/Installers.
            if is_real_directory(&entry) && !is_bundle(&entry) {
                for child in read_dir(&entry) {
                    seen += 1;
                    if seen > MAX_ENTRIES {
                        report.complete = false;
                        break;
                    }
                    candidates.push(child);
                }
            } else {
                candidates.push(entry);
            }
        }
        for path in candidates {
            let Some(kind) = kind_of(&path) else {
                continue;
            };
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            let size_kb = if metadata.is_dir() {
                directory_kb(&path)
            } else {
                metadata.blocks() / 2
            };
            if size_kb < MIN_REPORT_KB {
                continue;
            }
            let age_days = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .unwrap_or(Duration::ZERO)
                .as_secs()
                / 86_400;
            let in_use = open.as_ref().map(|open| {
                open.iter()
                    .any(|held| held == &path || held.starts_with(&path))
            });
            report.installers.push(Installer {
                installed_app: matching_app(&path, apps),
                path,
                kind,
                size_kb,
                age_days,
                in_use,
            });
        }
    }
    report
        .installers
        .sort_by(|a, b| b.size_kb.cmp(&a.size_kb).then_with(|| a.path.cmp(&b.path)));
    report
}

fn read_dir(path: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| !name.to_string_lossy().starts_with('.'))
        })
        .collect();
    entries.sort();
    entries
}

fn is_real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// Package bundles (`Foo.mpkg/`) are directories but count as one file.
fn is_bundle(path: &Path) -> bool {
    kind_of(path).is_some()
}

fn kind_of(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let index = EXTENSIONS.iter().position(|known| *known == extension)?;
    Some(
        [
            "disk image",
            "installer package",
            "installer package",
            "Xcode archive",
            "ISO image",
        ][index],
    )
}

fn directory_kb(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    let mut seen = 0;
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(&folder).into_iter().flatten().flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return total;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
                continue;
            }
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                total += metadata.blocks() / 2;
            }
        }
    }
    total
}

/// `.app` bundles directly inside each folder.
pub fn installed_apps(folders: &[PathBuf]) -> Vec<PathBuf> {
    folders
        .iter()
        .flat_map(|folder| read_dir(folder))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        })
        .collect()
}

/// The words that name the product, lower-cased: "Firefox 131.0.2.dmg" and
/// "firefox-131.0.2-arm64.dmg" both give "firefox"; "Visual Studio Code.app"
/// gives "visualstudiocode". Version numbers, architectures, and "installer"
/// or "setup" are not part of the name.
fn product_key(name: &str) -> String {
    let mut key = String::new();
    for word in name.split(|c: char| !c.is_alphanumeric()) {
        let lower = word.to_ascii_lowercase();
        let starts_with_digit = lower.chars().next().is_some_and(|c| c.is_ascii_digit());
        let is_version = lower.starts_with('v')
            && lower.len() > 1
            && lower[1..].chars().all(|c| c.is_ascii_digit());
        if starts_with_digit || is_version {
            break;
        }
        if matches!(
            lower.as_str(),
            "arm64"
                | "x64"
                | "x86"
                | "intel"
                | "universal"
                | "mac"
                | "macos"
                | "osx"
                | "darwin"
                | "installer"
                | "install"
                | "setup"
                | "latest"
                | "stable"
        ) {
            continue;
        }
        key.push_str(&lower);
    }
    key
}

fn matching_app(installer: &Path, apps: &[PathBuf]) -> Option<PathBuf> {
    let key = product_key(&installer.file_stem()?.to_string_lossy());
    if key.len() < 3 {
        return None;
    }
    apps.iter()
        .find(|app| {
            app.file_stem()
                .is_some_and(|stem| product_key(&stem.to_string_lossy()) == key)
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_names_ignore_versions_architectures_and_installer_words() {
        assert_eq!(product_key("Firefox 131.0.2"), "firefox");
        assert_eq!(product_key("firefox-131.0.2-arm64"), "firefox");
        assert_eq!(product_key("Visual Studio Code"), "visualstudiocode");
        assert_eq!(
            product_key("VisualStudioCode-darwin-universal"),
            "visualstudiocode"
        );
        assert_eq!(product_key("Docker"), "docker");
        assert_eq!(product_key("Install Docker v4"), "docker");
        assert_eq!(product_key("Xcode_26.1"), "xcode");
    }

    #[test]
    fn reports_installers_with_age_and_matching_app_and_skips_other_files() {
        let home = tempfile::tempdir().unwrap();
        let downloads = home.path().join("Downloads");
        let nested = downloads.join("Installers");
        fs::create_dir_all(&nested).unwrap();
        let apps = home.path().join("Applications");
        fs::create_dir_all(apps.join("Firefox.app")).unwrap();
        fs::write(
            downloads.join("Firefox 131.0.dmg"),
            vec![1; 3 * 1024 * 1024],
        )
        .unwrap();
        fs::write(nested.join("Tool-2.1.pkg"), vec![1; 2 * 1024 * 1024]).unwrap();
        fs::write(downloads.join("photo.jpg"), vec![1; 4 * 1024 * 1024]).unwrap();
        fs::write(downloads.join("tiny.dmg"), vec![1; 16]).unwrap();
        let bundle = downloads.join("Suite.mpkg/Contents");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(bundle.join("payload"), vec![1; 3 * 512 * 1024]).unwrap();

        let installed = installed_apps(&[apps]);
        let later = SystemTime::now() + Duration::from_secs(40 * 86_400);
        let report = find_in(std::slice::from_ref(&downloads), &installed, later);
        let names: Vec<String> = report
            .installers
            .iter()
            .map(|item| {
                item.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["Firefox 131.0.dmg", "Tool-2.1.pkg", "Suite.mpkg"]);
        let firefox = &report.installers[0];
        assert_eq!(firefox.kind, "disk image");
        assert!(
            firefox
                .installed_app
                .as_ref()
                .unwrap()
                .ends_with("Firefox.app")
        );
        assert!((39..=41).contains(&firefox.age_days));
        assert!(report.installers[1].installed_app.is_none());
        assert!(report.complete);
        assert!(report.redundant_kb() >= 3 * 1024 && report.redundant_kb() < report.total_kb());
        let json = report.to_json();
        assert_eq!(json["schema"], "diskray.installers/1");
        assert_eq!(json["deleted"], false);
        let lines = report.lines(|path| path.display().to_string(), 10);
        assert!(lines[0].contains("Firefox is installed"), "{}", lines[0]);
        assert!(lines[0].contains("40 days old") || lines[0].contains("39 days old"));
    }

    #[test]
    fn symlinked_installers_are_not_followed() {
        let home = tempfile::tempdir().unwrap();
        let downloads = home.path().join("Downloads");
        fs::create_dir_all(&downloads).unwrap();
        let outside = home.path().join("big.dmg");
        fs::write(&outside, vec![1; 2 * 1024 * 1024]).unwrap();
        std::os::unix::fs::symlink(&outside, downloads.join("link.dmg")).unwrap();
        let report = find_in(&[downloads], &[], SystemTime::now());
        assert!(report.installers.is_empty());
    }
}
