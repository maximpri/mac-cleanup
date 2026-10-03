// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Maxim Priezjev and contributors
//! License information available without scanning, migration, or network access.

use std::io::{self, Write};

pub const NOTICE: &str = "Diskray · Copyright (C) 2026 Maxim Priezjev and contributors\n\
Free software under GPL-3.0-or-later; redistribution is permitted under its terms.\n\
ABSOLUTELY NO WARRANTY. Run `diskray license` for the full license.";

pub const TEXT: &str = include_str!("../LICENSE");

pub fn write_license(mut output: impl Write) -> io::Result<()> {
    writeln!(output, "{NOTICE}\n\n{TEXT}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn license_output_contains_notice_and_unmodified_license() {
        let mut output = Vec::new();
        write_license(&mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.starts_with(NOTICE));
        assert!(text.contains(TEXT));
        assert!(text.contains("Version 3, 29 June 2007"));
    }

    #[test]
    fn output_errors_are_reported_without_panicking() {
        struct BrokenPipe;
        impl Write for BrokenPipe {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(
            write_license(BrokenPipe).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }
}
