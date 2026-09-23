// SPDX-License-Identifier: GPL-3.0-or-later
//! What grew between two complete assessments of the same volume.
//!
//! Sizes wobble between scans, so a change is reported only when it is at
//! least 500 MB and 5% of the earlier size, on an item of at least 100 MB.
//! When a folder and something inside it both grew, the deeper path is named
//! unless the folder grew clearly more than the child explains.

use crate::care::Session;
use std::path::Path;

const MIN_CHANGE_KB: u64 = 500 * 1024;
const MIN_ITEM_KB: u64 = 100 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub path: String,
    pub before_kb: u64,
    pub after_kb: u64,
}

impl Delta {
    pub fn change_kb(&self) -> i64 {
        self.after_kb as i64 - self.before_kb as i64
    }
}

fn significant(before: u64, after: u64) -> bool {
    let change = before.abs_diff(after);
    before.max(after) >= MIN_ITEM_KB && change >= MIN_CHANGE_KB.max(before / 20)
}

/// Whether two sessions describe the same volume well enough to compare.
pub fn comparable(old: &Session, new: &Session) -> bool {
    old.complete
        && new.complete
        && old.root.is_some()
        && old.root == new.root
        && match (&old.volume_id, &new.volume_id) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
}

/// Significant changes from `old` to `new`, largest first. Returns nothing
/// when the sessions are not comparable.
pub fn diff(old: &Session, new: &Session) -> Vec<Delta> {
    if !comparable(old, new) {
        return Vec::new();
    }
    let mut deltas: Vec<Delta> = new
        .measurements
        .iter()
        .filter_map(|(path, after)| {
            let before = old.measurements.get(path)?;
            significant(*before, *after).then(|| Delta {
                path: path.clone(),
                before_kb: *before,
                after_kb: *after,
            })
        })
        .collect();
    // Prefer the deepest explanation: drop a folder when a significant change
    // inside it, in the same direction, explains at least half of it.
    let snapshot = deltas.clone();
    deltas.retain(|parent| {
        !snapshot.iter().any(|child| {
            child.path != parent.path
                && Path::new(&child.path).starts_with(&parent.path)
                && child.change_kb().signum() == parent.change_kb().signum()
                && child.change_kb().abs() * 2 >= parent.change_kb().abs()
        })
    });
    deltas.sort_by_key(|delta| std::cmp::Reverse(delta.change_kb().abs()));
    deltas
}

/// The newest complete session of the same root at least `days` old.
pub fn baseline<'a>(history: &'a [Session], current: &Session, days: u64) -> Option<&'a Session> {
    let cutoff = current.updated.saturating_sub(days.saturating_mul(86_400));
    history
        .iter()
        .filter(|session| session.id != current.id && session.updated <= cutoff)
        .filter(|session| comparable(session, current))
        .max_by_key(|session| session.updated)
}

/// The previous complete session of the same root, for "since last check".
pub fn previous<'a>(history: &'a [Session], current: &Session) -> Option<&'a Session> {
    baseline(history, current, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const GB: u64 = 1_048_576;

    fn session(id: u64, updated: u64, sizes: &[(&str, u64)]) -> Session {
        Session {
            id,
            updated,
            complete: true,
            root: Some("/".into()),
            volume_id: Some("VOLUME".into()),
            measurements: sizes
                .iter()
                .map(|(path, kb)| (path.to_string(), *kb))
                .collect::<HashMap<_, _>>(),
            ..Session::default()
        }
    }

    #[test]
    fn reports_significant_changes_and_ignores_noise() {
        let old = session(
            1,
            100,
            &[
                ("/a", 10 * GB),
                ("/b", 10 * GB),
                ("/tiny", 50 * 1024),
                ("/gone", GB),
            ],
        );
        let new = session(
            2,
            200,
            &[
                ("/a", 12 * GB),
                ("/b", 10 * GB + 300 * 1024),
                ("/tiny", 60 * 1024),
                ("/new", 5 * GB),
            ],
        );
        let deltas = diff(&old, &new);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].path, "/a");
        assert_eq!(deltas[0].change_kb(), 2 * GB as i64);
    }

    #[test]
    fn names_the_deepest_path_that_explains_the_growth() {
        let old = session(
            1,
            100,
            &[
                ("/Users/me/Library", 50 * GB),
                ("/Users/me/Library/Caches", 5 * GB),
            ],
        );
        let new = session(
            2,
            200,
            &[
                ("/Users/me/Library", 60 * GB),
                ("/Users/me/Library/Caches", 14 * GB),
            ],
        );
        let deltas = diff(&old, &new);
        assert_eq!(
            deltas.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            vec!["/Users/me/Library/Caches"]
        );
    }

    #[test]
    fn only_complete_sessions_of_the_same_volume_compare() {
        let old = session(1, 100, &[("/a", 10 * GB)]);
        let mut new = session(2, 200, &[("/a", 20 * GB)]);
        new.volume_id = Some("OTHER".into());
        assert!(diff(&old, &new).is_empty());
        new.volume_id = Some("VOLUME".into());
        new.complete = false;
        assert!(diff(&old, &new).is_empty());
    }

    #[test]
    fn baseline_picks_the_newest_session_old_enough() {
        let day = 86_400;
        let history = vec![
            session(3, 29 * day, &[]),
            session(2, 20 * day, &[]),
            session(1, 10 * day, &[]),
        ];
        let current = session(4, 30 * day, &[]);
        assert_eq!(baseline(&history, &current, 7).map(|s| s.id), Some(2));
        assert_eq!(previous(&history, &current).map(|s| s.id), Some(3));
        assert!(baseline(&history, &current, 60).is_none());
    }
}
