// SPDX-License-Identifier: GPL-3.0-or-later
//! Cleanup plans proposed by an outside agent (through MCP), waiting for the
//! user. A proposal never runs by itself: `diskray review` loads it into the
//! normal review screen, every target is measured and checked again, and the
//! usual typed confirmation is required.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

const MAX_PLANS: usize = 5;
const LIFETIME_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProposedAction {
    /// The plan-action id, for example `clean:/Users/me/Library/Caches/pip`.
    pub id: String,
    pub path: String,
    pub label: String,
    pub size_kb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingPlan {
    pub schema: String,
    pub created: u64,
    /// The MCP client that proposed it, as it identified itself.
    pub client: String,
    pub reason: String,
    pub actions: Vec<ProposedAction>,
}

fn directory(home: &Path) -> io::Result<PathBuf> {
    let directory = crate::care::app_data_directory(home)?.join("pending");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(io::Error::other(
                "pending-plan folder redirects or is not a folder",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&directory)?,
        Err(error) => return Err(error),
    }
    Ok(directory)
}

fn plan_files(directory: &Path) -> Vec<(PathBuf, u64)> {
    let mut files: Vec<(PathBuf, u64)> = fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .filter_map(|path| {
                    let created = path.file_stem()?.to_str()?.parse::<u64>().ok()?;
                    Some((path, created))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|(_, created)| *created);
    files
}

/// Save a proposal (mode 0600, written atomically). Keeps the newest few and
/// drops expired ones.
pub fn save(home: &Path, plan: &PendingPlan) -> io::Result<PathBuf> {
    let directory = directory(home)?;
    let target = directory.join(format!("{}.json", plan.created));
    let temporary = directory.join(format!(".{}-{}.tmp", plan.created, std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let bytes = serde_json::to_vec(plan)?;
    if let Err(error) = file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| fs::rename(&temporary, &target))
    {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let files = plan_files(&directory);
    let excess = files.len().saturating_sub(MAX_PLANS);
    for (index, (path, created)) in files.iter().enumerate() {
        if (index < excess || created + LIFETIME_SECS < plan.created) && path != &target {
            let _ = fs::remove_file(path);
        }
    }
    Ok(target)
}

/// The newest proposal that has not expired, with its file.
pub fn newest(home: &Path, now: u64) -> Option<(PathBuf, PendingPlan)> {
    let directory = home.join(crate::paths::APP_DATA_SUBPATH).join("pending");
    plan_files(&directory)
        .into_iter()
        .rev()
        .filter(|(_, created)| created + LIFETIME_SECS >= now)
        .find_map(|(path, _)| {
            let metadata = fs::symlink_metadata(&path).ok()?;
            if !metadata.is_file() || metadata.len() > 256 * 1024 {
                return None;
            }
            let plan: PendingPlan = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
            (plan.schema == "diskray.pending/1").then_some((path, plan))
        })
}

/// Remove a proposal once it has been loaded into the review screen.
pub fn consume(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(created: u64) -> PendingPlan {
        PendingPlan {
            schema: "diskray.pending/1".into(),
            created,
            client: "test".into(),
            reason: "free space".into(),
            actions: vec![ProposedAction {
                id: "clean:/x".into(),
                path: "/x".into(),
                label: "X".into(),
                size_kb: 1,
            }],
        }
    }

    #[test]
    fn keeps_the_newest_valid_plans_and_expires_old_ones() {
        let home = tempfile::tempdir().unwrap();
        for created in 1_000..1_008 {
            save(home.path(), &plan(created)).unwrap();
        }
        let (path, newest_plan) = newest(home.path(), 1_010).unwrap();
        assert_eq!(newest_plan.created, 1_007);
        let directory = home
            .path()
            .join(crate::paths::APP_DATA_SUBPATH)
            .join("pending");
        assert_eq!(plan_files(&directory).len(), MAX_PLANS);
        assert!(
            newest(home.path(), 1_007 + LIFETIME_SECS + 1).is_none(),
            "expired plans are ignored"
        );
        consume(&path);
        assert_eq!(newest(home.path(), 1_010).unwrap().1.created, 1_006);
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(directory.join("1006.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
