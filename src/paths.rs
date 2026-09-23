// SPDX-License-Identifier: GPL-3.0-or-later
//! Every per-user location Diskray reads or writes, relative to the account
//! home. Keeping them in one place makes renames and migrations auditable.

use std::path::{Path, PathBuf};

/// Private application data: sessions, settings, pending plans.
pub const APP_DATA_SUBPATH: &str = "Library/Application Support/diskray";
pub const HISTORY_SUBPATH: &str = "Library/Application Support/diskray/sessions";
pub const SETTINGS_SUBPATH: &str = "Library/Application Support/diskray/settings.json";
/// Append-only audit log of every cleanup attempt.
pub const LOG_SUBPATH: &str = "Library/Logs/diskray/deletions.log";
/// User configuration: whitelist and rule packs.
pub const CONFIG_SUBPATH: &str = ".config/diskray";
pub const WHITELIST_SUBPATH: &str = ".config/diskray/whitelist";
/// Name prefix for relocation staging and backup directories.
pub const RELOCATION_PREFIX: &str = ".diskray";

/// Locations used before the rename to Diskray, as `(old, new)` pairs of
/// directories relative to the account home.
pub const LEGACY_DIRECTORIES: [(&str, &str); 3] = [
    (
        "Library/Application Support/mac-cleanup",
        "Library/Application Support/diskray",
    ),
    ("Library/Logs/mac-cleanup", "Library/Logs/diskray"),
    (".config/mac-cleanup", ".config/diskray"),
];
/// Relocation leftovers from before the rename use this prefix.
pub const LEGACY_RELOCATION_PREFIX: &str = ".mac-cleanup";

pub fn in_home(home: &Path, subpath: &str) -> PathBuf {
    home.join(subpath)
}
