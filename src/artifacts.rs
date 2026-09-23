// SPDX-License-Identifier: GPL-3.0-or-later
//! Build output left behind in projects nobody has touched for a while.
//!
//! This is a report only: Diskray never deletes these folders. A project
//! counts as untouched when none of its own files (ignoring the build output
//! and `.git` internals other than `index` and `HEAD`) changed within the
//! threshold. Anything the bounded walk cannot fully inspect is treated as
//! recently used, so it is never reported as stale by mistake.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime},
};

const MAX_ENTRIES: u64 = 200_000;
const MAX_PROJECT_ENTRIES: u64 = 20_000;
const TIME_LIMIT: Duration = Duration::from_secs(20);
const MAX_DEPTH: usize = 6;
const MIN_REPORT_KB: u64 = 10 * 1024;

/// `~/.config/diskray/projects.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Folders that contain projects, relative to the home folder. Empty
    /// means every top-level home folder except media, apps, and sync folders.
    pub roots: Vec<String>,
    pub min_age_days: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            min_age_days: 60,
        }
    }
}

/// Top-level home folders that hold media, apps, or synced files rather than
/// projects. Everything else is searched when no roots are configured.
const NOT_PROJECT_FOLDERS: [&str; 9] = [
    "Library",
    "Applications",
    "Pictures",
    "Music",
    "Movies",
    "Public",
    "Dropbox",
    "OneDrive",
    "Google Drive",
];

impl Config {
    /// Configured roots, or every non-hidden top-level home folder that is
    /// not known to hold media, apps, or synced files.
    pub fn roots(&self, home: &Path) -> Vec<String> {
        if !self.roots.is_empty() {
            return self.roots.clone();
        }
        let mut roots: Vec<String> = fs::read_dir(home)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.') && !NOT_PROJECT_FOLDERS.contains(&name.as_str()))
            .collect();
        roots.sort();
        roots
    }

    pub fn load(home: &Path) -> Self {
        let path = home
            .join(crate::paths::CONFIG_SUBPATH)
            .join("projects.toml");
        fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Artifact {
    pub project: PathBuf,
    pub path: PathBuf,
    pub kind: &'static str,
    pub size_kb: u64,
    pub untouched_days: u64,
    /// How to get it back, for example `npm install`.
    pub regenerate: &'static str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub artifacts: Vec<Artifact>,
    /// The walk finished every configured root within its limits.
    pub complete: bool,
    pub roots: Vec<PathBuf>,
}

impl Report {
    pub fn total_kb(&self) -> u64 {
        self.artifacts.iter().map(|artifact| artifact.size_kb).sum()
    }

    /// One line per artifact; `display` renders each path.
    pub fn lines(&self, display: impl Fn(&Path) -> String, limit: usize) -> Vec<String> {
        self.artifacts
            .iter()
            .take(limit)
            .map(|artifact| {
                format!(
                    "{} · {} · {} · untouched {} days · rebuild: {}",
                    display(&artifact.path),
                    artifact.kind,
                    crate::cache::format_kb(artifact.size_kb),
                    artifact.untouched_days,
                    artifact.regenerate
                )
            })
            .collect()
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": "diskray.artifacts/1",
            "complete": self.complete,
            "roots": self.roots,
            "total_kb": self.total_kb(),
            "artifacts": self.artifacts,
            "deleted": false,
        })
    }
}

/// `(folder name, marker files in the project, kind, how to regenerate)`.
const KINDS: [(&str, &[&str], &str, &str); 6] = [
    (
        "node_modules",
        &["package.json"],
        "Node.js node_modules",
        "npm install (or your package manager)",
    ),
    ("target", &["Cargo.toml"], "Rust target", "cargo build"),
    (
        ".venv",
        &[],
        "Python .venv",
        "python3 -m venv .venv, then reinstall packages",
    ),
    (
        ".build",
        &["Package.swift"],
        "SwiftPM .build",
        "swift build",
    ),
    (
        ".gradle",
        &[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
        "Gradle .gradle",
        "gradle build",
    ),
    (".next", &["package.json"], "Next.js .next", "next build"),
];

fn kind_for(project: &Path, name: &str) -> Option<(&'static str, &'static str)> {
    KINDS
        .iter()
        .find_map(|(folder, markers, kind, regenerate)| {
            if name != *folder {
                return None;
            }
            let matches = if name == ".venv" {
                project.join(".venv/pyvenv.cfg").is_file()
            } else {
                markers.iter().any(|marker| project.join(marker).is_file())
            };
            matches.then_some((*kind, *regenerate))
        })
}

fn is_artifact_name(name: &str) -> bool {
    KINDS.iter().any(|(folder, ..)| *folder == name)
}

/// Seconds since the newest change to the project's own files, or `None`
/// when the walk could not finish.
fn project_age(project: &Path, now: SystemTime) -> Option<u64> {
    let mut newest = None::<SystemTime>;
    let mut seen = 0;
    let mut pending = vec![project.to_path_buf()];
    let mut note = |modified: SystemTime| {
        newest = Some(newest.map_or(modified, |current| current.max(modified)));
    };
    for git_file in [".git/index", ".git/HEAD"] {
        if let Ok(modified) =
            fs::symlink_metadata(project.join(git_file)).and_then(|m| m.modified())
        {
            note(modified);
        }
    }
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).ok()? {
            seen += 1;
            if seen > MAX_PROJECT_ENTRIES {
                return None;
            }
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if directory == project && (is_artifact_name(&name) || name == ".git") {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path()).ok()?;
            if let Ok(modified) = metadata.modified() {
                note(modified);
            }
            if metadata.is_dir() && !metadata.file_type().is_symlink() && !is_artifact_name(&name) {
                pending.push(entry.path());
            }
        }
    }
    newest.map(|newest| now.duration_since(newest).unwrap_or_default().as_secs())
}

/// Find stale build output under the configured roots.
pub fn find(home: &Path, config: &Config, cancel: &AtomicBool) -> Report {
    let started = Instant::now();
    let now = SystemTime::now();
    let mut report = Report {
        complete: true,
        ..Report::default()
    };
    let mut seen = 0;
    for root in &config.roots(home) {
        if crate::rules::valid_relative_path(root).is_err() {
            continue;
        }
        let root = home.join(root);
        let Ok(metadata) = fs::symlink_metadata(&root) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        report.roots.push(root.clone());
        let device = metadata.dev();
        let mut pending = vec![(root, 0_usize)];
        while let Some((directory, depth)) = pending.pop() {
            if cancel.load(Ordering::Relaxed)
                || started.elapsed() > TIME_LIMIT
                || seen > MAX_ENTRIES
            {
                report.complete = false;
                break;
            }
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                seen += 1;
                let path = entry.path();
                let Ok(metadata) = fs::symlink_metadata(&path) else {
                    continue;
                };
                if !metadata.is_dir()
                    || metadata.file_type().is_symlink()
                    || metadata.dev() != device
                {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some((kind, regenerate)) = kind_for(&directory, &name) {
                    if let Some(age) = project_age(&directory, now) {
                        let days = age / 86_400;
                        let size_kb = crate::cache::directory_kb(&path);
                        if days >= config.min_age_days && size_kb >= MIN_REPORT_KB {
                            report.artifacts.push(Artifact {
                                project: directory.clone(),
                                path,
                                kind,
                                size_kb,
                                untouched_days: days,
                                regenerate,
                            });
                        }
                    }
                    continue;
                }
                if name == ".git"
                    || (name.starts_with('.') && name != ".config")
                    || depth >= MAX_DEPTH
                {
                    continue;
                }
                pending.push((path, depth + 1));
            }
        }
    }
    report
        .artifacts
        .sort_by_key(|artifact| std::cmp::Reverse(artifact.size_kb));
    report.artifacts.truncate(50);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backdate(path: &Path, days: u64) {
        let when = SystemTime::now() - Duration::from_secs(days * 86_400);
        fs::File::open(path).unwrap().set_modified(when).unwrap();
    }

    fn project(root: &Path, name: &str, marker: &str, artifact: &str, days: u64) -> PathBuf {
        let project = root.join(name);
        fs::create_dir_all(project.join(artifact)).unwrap();
        fs::write(project.join(marker), b"{}").unwrap();
        fs::write(
            project.join(artifact).join("blob"),
            vec![1; 11 * 1024 * 1024],
        )
        .unwrap();
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(project.join("src/main"), b"code").unwrap();
        for path in [
            project.join(marker),
            project.join("src/main"),
            project.join("src"),
            project.clone(),
        ] {
            backdate(&path, days);
        }
        project
    }

    #[test]
    fn reports_only_build_output_in_projects_untouched_long_enough() {
        let home = tempfile::tempdir().unwrap();
        let code = home.path().join("code");
        let old_node = project(&code, "old-web", "package.json", "node_modules", 120);
        let old_rust = project(&code, "old-rust", "Cargo.toml", "target", 90);
        let fresh = project(&code, "fresh-web", "package.json", "node_modules", 1);
        let unmarked = code.join("not-a-project/target");
        fs::create_dir_all(&unmarked).unwrap();
        fs::write(unmarked.join("blob"), vec![1; 11 * 1024 * 1024]).unwrap();
        let recently_committed = project(&code, "old-but-committed", "Cargo.toml", "target", 200);
        fs::create_dir_all(recently_committed.join(".git")).unwrap();
        fs::write(recently_committed.join(".git/index"), b"i").unwrap();

        let report = find(home.path(), &Config::default(), &AtomicBool::new(false));
        let projects: Vec<_> = report.artifacts.iter().map(|a| a.project.clone()).collect();
        assert!(projects.contains(&old_node));
        assert!(projects.contains(&old_rust));
        assert!(
            !projects.contains(&fresh),
            "recently edited projects are kept"
        );
        assert!(
            !projects.contains(&recently_committed),
            "a recent git index counts as activity"
        );
        assert!(
            !report.artifacts.iter().any(|a| a.path == unmarked),
            "no marker, no artifact"
        );
        let rust = report
            .artifacts
            .iter()
            .find(|a| a.project == old_rust)
            .unwrap();
        assert_eq!(rust.kind, "Rust target");
        assert!(rust.untouched_days >= 89);
        assert!(report.complete);
        assert!(
            old_node.join("node_modules/blob").exists(),
            "nothing is deleted"
        );
    }

    #[test]
    fn configuration_limits_roots_and_age() {
        let home = tempfile::tempdir().unwrap();
        project(
            &home.path().join("work"),
            "app",
            "package.json",
            "node_modules",
            40,
        );
        project(
            &home.path().join("Music"),
            "old",
            "package.json",
            "node_modules",
            400,
        );
        let default = find(home.path(), &Config::default(), &AtomicBool::new(false));
        assert!(
            default.artifacts.is_empty(),
            "40 days is under the default age"
        );
        assert_eq!(
            default.roots,
            vec![home.path().join("work")],
            "media folders are skipped"
        );
        let custom = Config {
            roots: vec!["work".into(), "../escape".into()],
            min_age_days: 30,
        };
        let report = find(home.path(), &custom, &AtomicBool::new(false));
        assert_eq!(report.artifacts.len(), 1);
        assert_eq!(report.roots, vec![home.path().join("work")]);
    }
}
