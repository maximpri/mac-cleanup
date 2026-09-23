// SPDX-License-Identifier: GPL-3.0-or-later
//! One-time move of user data from the pre-rename `mac-cleanup` locations.
//!
//! A directory is moved only when the old location is a real directory owned
//! by this account and the new location does not exist yet. Nothing is copied
//! or merged, and symlinks are never followed or created, so a failed or
//! skipped migration leaves the old data exactly where it was.

use crate::paths::{LEGACY_DIRECTORIES, in_home};
use std::{fs, os::unix::fs::MetadataExt, path::Path};

/// What happened to one legacy directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moved {
    Renamed { from: String, to: String },
    Skipped { from: String, reason: String },
}

/// Move each legacy directory that can be moved safely. Returns only the
/// directories that existed, so an empty result means nothing to migrate.
pub fn run(home: &Path) -> Vec<Moved> {
    let uid = fs::symlink_metadata(home)
        .map(|metadata| metadata.uid())
        .ok();
    LEGACY_DIRECTORIES
        .iter()
        .filter_map(|(old, new)| {
            let from = in_home(home, old);
            let to = in_home(home, new);
            let metadata = fs::symlink_metadata(&from).ok()?;
            let label = from.display().to_string();
            let skip = |reason: &str| {
                Some(Moved::Skipped {
                    from: label.clone(),
                    reason: reason.into(),
                })
            };
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return skip("not a real directory");
            }
            if uid.is_some_and(|uid| metadata.uid() != uid) {
                return skip("owned by another account");
            }
            if fs::symlink_metadata(&to).is_ok() {
                return skip("the new location already exists");
            }
            if let Some(parent) = to.parent()
                && fs::create_dir_all(parent).is_err()
            {
                return skip("could not create the new parent directory");
            }
            match fs::rename(&from, &to) {
                Ok(()) => Some(Moved::Renamed {
                    from: label,
                    to: to.display().to_string(),
                }),
                Err(error) => skip(&error.to_string()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_legacy_directories_once_and_never_overwrites() {
        let home = tempfile::tempdir().unwrap();
        let old_sessions = home
            .path()
            .join("Library/Application Support/mac-cleanup/sessions");
        fs::create_dir_all(&old_sessions).unwrap();
        fs::write(old_sessions.join("1.json"), b"{}").unwrap();
        fs::create_dir_all(home.path().join(".config/mac-cleanup")).unwrap();
        fs::write(
            home.path().join(".config/mac-cleanup/whitelist"),
            b"Library/Keep",
        )
        .unwrap();
        // The new log directory already exists, so the old one must stay.
        fs::create_dir_all(home.path().join("Library/Logs/mac-cleanup")).unwrap();
        fs::create_dir_all(home.path().join("Library/Logs/diskray")).unwrap();

        let moved = run(home.path());

        assert!(
            home.path()
                .join("Library/Application Support/diskray/sessions/1.json")
                .exists()
        );
        assert_eq!(
            fs::read(home.path().join(".config/diskray/whitelist")).unwrap(),
            b"Library/Keep"
        );
        assert!(home.path().join("Library/Logs/mac-cleanup").exists());
        assert_eq!(
            moved
                .iter()
                .filter(|m| matches!(m, Moved::Renamed { .. }))
                .count(),
            2
        );
        assert!(moved.iter().any(
            |m| matches!(m, Moved::Skipped { reason, .. } if reason.contains("already exists"))
        ));
        assert!(
            run(home.path())
                .iter()
                .all(|m| matches!(m, Moved::Skipped { .. })),
            "a second run moves nothing"
        );
    }

    #[test]
    fn symlinked_legacy_directories_are_left_alone() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = home.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::create_dir_all(home.path().join(".config")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, home.path().join(".config/mac-cleanup")).unwrap();

        let moved = run(home.path());

        assert!(
            matches!(&moved[..], [Moved::Skipped { reason, .. }] if reason.contains("not a real"))
        );
        assert!(!home.path().join(".config/diskray").exists());
    }
}
