// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Maxim Priezjev and contributors
use std::process::Command;

#[test]
fn license_is_available_without_a_terminal_or_environment() {
    let working_directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_diskray"))
        .arg("license")
        .env_clear()
        .current_dir(working_directory.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(diskray::licensing::NOTICE));
    assert!(text.contains(diskray::licensing::TEXT));
    assert_eq!(working_directory.path().read_dir().unwrap().count(), 0);
}

#[test]
fn command_help_exposes_license() {
    let output = Command::new(env!("CARGO_BIN_EXE_diskray"))
        .arg("--help")
        .env_clear()
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("license"));
    assert!(text.contains("GPL-3.0-or-later"));
}
