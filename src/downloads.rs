//! Incomplete-download candidates in the account's Downloads folder.
//!
//! Browsers leave `*.crdownload`, `*.part`, and `*.download` entries behind
//! when a download never finishes. Entries become exact-path cleanup
//! candidates only when they are owned by the current account, untouched past
//! the retention age, and not open by any process. Discovery fails closed to
//! no candidates whenever the open-file check cannot run.

use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::cache::{self, CacheSpec, CacheTarget, CacheTier, effective_user_id};

const INCOMPLETE_EXTENSIONS: [&str; 3] = ["crdownload", "part", "download"];

/// Discover stale incomplete downloads for an account home.
///
/// `retention_days` is the same threshold used for `/private/tmp`: an
/// unfinished download untouched for that long is treated as abandoned.
pub fn discover_specs(account_home: &Path, retention_days: u64) -> Vec<CacheSpec> {
    let downloads = account_home.join("Downloads");
    if !is_real_directory(&downloads) {
        return Vec::new();
    }
    let Some(uid) = effective_user_id() else {
        return Vec::new();
    };
    let Ok(open_paths) = cache::open_paths_under(&downloads) else {
        return Vec::new();
    };
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(
            retention_days.saturating_mul(24 * 60 * 60),
        ))
        .unwrap_or(UNIX_EPOCH);
    let Ok(reader) = fs::read_dir(&downloads) else {
        return Vec::new();
    };

    let mut specs = Vec::new();
    for entry in reader.flatten() {
        let path = entry.path();
        if !has_incomplete_extension(&path) {
            continue;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink() || metadata.uid() != uid {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if modified > cutoff {
            continue;
        }
        if open_paths
            .iter()
            .any(|open| open == &path || open.starts_with(&path))
        {
            continue;
        }
        specs.push(CacheSpec {
            home: account_home.to_path_buf(),
            path,
            label: "Incomplete download",
            tier: CacheTier::Routine,
            process_pattern: "",
            note: "abandoned partial download; the browser downloads it again on request",
            target: CacheTarget::ExactPath,
        });
    }
    specs.sort_by(|left, right| left.path.cmp(&right.path));
    specs
}

fn has_incomplete_extension(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        INCOMPLETE_EXTENSIONS.contains(&extension.to_string_lossy().to_lowercase().as_str())
    })
}

fn is_real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_home() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn backdate(path: &Path, days: u64) {
        let file = fs::File::open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60))
            .unwrap();
    }

    #[test]
    fn finds_only_stale_incomplete_downloads() {
        let home = make_home();
        let downloads = home.path().join("Downloads");
        fs::create_dir(&downloads).unwrap();
        fs::write(downloads.join("installer.pkg.crdownload"), [1_u8; 8_192]).unwrap();
        fs::write(downloads.join("movie.part"), [2_u8; 4_096]).unwrap();
        fs::create_dir(downloads.join("page.safari.download")).unwrap();
        fs::write(downloads.join("done.zip"), [3_u8; 2_048]).unwrap();
        fs::write(downloads.join("fresh.crdownload"), [4_u8; 1_024]).unwrap();
        fs::write(downloads.join("notes.txt.part"), [5_u8; 512]).unwrap();
        for name in [
            "installer.pkg.crdownload",
            "movie.part",
            "page.safari.download",
            "notes.txt.part",
        ] {
            backdate(&downloads.join(name), 10);
        }
        std::os::unix::fs::symlink(
            downloads.join("done.zip"),
            downloads.join("linked.crdownload"),
        )
        .unwrap();
        backdate(&downloads.join("linked.crdownload"), 10);

        let specs = discover_specs(home.path(), 7);
        let names: Vec<String> = specs
            .iter()
            .map(|spec| {
                spec.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        assert_eq!(
            names,
            vec![
                "installer.pkg.crdownload",
                "movie.part",
                "notes.txt.part",
                "page.safari.download"
            ]
        );
        assert!(
            specs
                .iter()
                .all(|spec| spec.target == CacheTarget::ExactPath)
        );
        assert!(specs.iter().all(|spec| spec.home == home.path()));
    }

    #[test]
    fn missing_or_linked_downloads_folder_yields_nothing() {
        let home = make_home();
        assert!(discover_specs(home.path(), 7).is_empty());

        let linked = home.path().join("Downloads");
        std::os::unix::fs::symlink(home.path(), &linked).unwrap();
        assert!(discover_specs(home.path(), 7).is_empty());
    }
}
