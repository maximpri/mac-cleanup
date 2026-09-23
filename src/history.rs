// SPDX-License-Identifier: GPL-3.0-or-later
//! Append-only audit log for cleanup actions.
//!
//! Cleanup is permanent, so every attempted deletion is appended to
//! `~/Library/Logs/diskray/deletions.log` with the outcome, reclaimed
//! size, and exact path. Logging is best-effort: a logging failure never
//! blocks or changes a cleanup outcome.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::cache::{CacheEntry, CleanupOutcome};

pub fn log_path(account_home: &Path) -> PathBuf {
    account_home.join(crate::paths::LOG_SUBPATH)
}

/// Append one cleanup attempt to the deletions log.
pub fn record(account_home: &Path, entry: &CacheEntry, outcome: &CleanupOutcome) {
    let (result, removed_kb, detail) = match outcome {
        CleanupOutcome::Cleared { removed_kb, method } => {
            ("cleared", *removed_kb, method.explanation())
        }
        CleanupOutcome::Failed { error, removed_kb } => ("failed", *removed_kb, error.clone()),
        CleanupOutcome::SafetySkipped(reason) => ("skipped", 0, reason.clone()),
    };
    let line = format!(
        "{}\t{result}\t{removed_kb}\t{}\t{}\t{detail}\n",
        format_timestamp(SystemTime::now()),
        entry.spec.label,
        entry.spec.path.display(),
    );
    let path = log_path(account_home);
    if let Some(parent) = path.parent()
        && let Err(_) = fs::create_dir_all(parent)
    {
        return;
    }
    if let Ok(mut log) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = log.write_all(line.as_bytes());
    }
}

/// UTC timestamp in ISO 8601 form (`2026-09-04T12:34:56Z`) without pulling in
/// a date-time dependency.
pub fn format_timestamp(time: SystemTime) -> String {
    let epoch = time
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = (epoch / 86_400) as i64;
    let seconds_of_day = epoch % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60
    )
}

/// Days since 1970-01-01 to a civil UTC date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shift = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_shift + 2) / 5 + 1) as u32;
    let month = if month_shift < 10 {
        month_shift + 3
    } else {
        month_shift - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_are_iso8601_utc() {
        assert_eq!(format_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            format_timestamp(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            "2023-11-14T22:13:20Z"
        );
        // Pre-epoch times are pathological for this log and clamp to the epoch.
        assert_eq!(
            format_timestamp(UNIX_EPOCH - Duration::from_secs(86_400)),
            "1970-01-01T00:00:00Z"
        );
    }

    #[test]
    fn records_cleanup_attempts_with_path_and_outcome() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Library/Caches/pip");
        fs::create_dir_all(&path).unwrap();
        let entry = CacheEntry {
            spec: crate::cache::CacheSpec {
                home: temp.path().to_path_buf(),
                path: path.clone(),
                label: "pip cache",
                tier: crate::cache::CacheTier::Routine,
                process_pattern: "",
                note: "test",
                target: crate::cache::CacheTarget::DirectoryContents,
            },
            status: crate::cache::CacheStatus::Ready,
            size_kb: 16,
            outcome: None,
            identity: None,
        };

        record(
            temp.path(),
            &entry,
            &CleanupOutcome::Cleared {
                removed_kb: 16,
                method: crate::cache::CleanupMethod::ExactPath,
            },
        );
        record(
            temp.path(),
            &entry,
            &CleanupOutcome::Failed {
                error: "test failure".into(),
                removed_kb: 4,
            },
        );

        let log = fs::read_to_string(log_path(temp.path())).unwrap();
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\tcleared\t16\tpip cache\t"));
        assert!(lines[0].contains(&path.to_string_lossy().to_string()));
        assert!(lines[1].contains("\tfailed\t4\tpip cache\t"));
    }
}
