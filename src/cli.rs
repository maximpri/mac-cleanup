use std::path::PathBuf;

use clap::{ArgAction, Parser};

use crate::retention::DEFAULT_RETENTION_DAYS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Analyze,
    Clean,
}

#[derive(Debug, Parser)]
#[command(
    name = "mac-cleanup",
    version,
    about = "Clear known removable data and review unhealthy macOS processes",
    after_help = "Cleanup and process termination can lose data. Review confirmations carefully, and never run this command with sudo."
)]
pub struct Cli {
    /// Keep the entire run read-only (the TUI defaults to analysis but can opt into cleanup)
    #[arg(long, conflicts_with_all = ["clean", "relocate"])]
    pub analyze: bool,

    /// Select and clear eligible removable-data contents
    #[arg(long, conflicts_with_all = ["analyze", "relocate"])]
    pub clean: bool,

    /// Relocate one large user-owned directory and replace it with a symlink
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with_all = ["analyze", "clean", "volume"],
        requires = "relocate_to"
    )]
    pub relocate: Option<PathBuf>,

    /// Existing directory on an external mounted volume for relocation data
    #[arg(long, value_name = "PATH", requires = "relocate")]
    pub relocate_to: Option<PathBuf>,

    /// Include items that may require a large download to restore
    #[arg(long)]
    pub include_reinstallable: bool,

    /// Consider user-owned /private/tmp entries stale after this many days
    #[arg(
        long,
        value_name = "DAYS",
        value_parser = parse_retention_days,
        default_value_t = DEFAULT_RETENTION_DAYS
    )]
    pub tmp_retention_days: u64,

    /// Inventory this volume or home directory and scan its known waste locations
    #[arg(long, value_name = "PATH")]
    pub volume: Option<PathBuf>,

    /// Confirm cleanup or relocation without interactive confirmation
    #[arg(long)]
    pub yes: bool,

    /// Show missing candidates and exact paths in plain output
    #[arg(long, conflicts_with = "json")]
    pub verbose: bool,

    /// Emit a stable machine-readable report (implies non-interactive output)
    #[arg(long, conflicts_with = "verbose")]
    pub json: bool,

    /// Disable colored output
    #[arg(long, action = ArgAction::SetTrue)]
    pub no_color: bool,

    /// Use line-oriented output even when attached to a terminal
    #[arg(long)]
    pub no_tui: bool,
}

fn parse_retention_days(value: &str) -> Result<u64, String> {
    let days = value
        .parse::<u64>()
        .map_err(|_| format!("retention days must be a positive integer, got {value:?}"))?;
    if days == 0 {
        return Err("retention days must be at least 1".into());
    }
    Ok(days)
}

impl Cli {
    pub fn mode(&self) -> Mode {
        if self.clean {
            Mode::Clean
        } else {
            Mode::Analyze
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_days_have_a_safe_positive_default_and_parser() {
        let defaults = Cli::try_parse_from(["mac-cleanup"]).unwrap();
        assert_eq!(defaults.tmp_retention_days, DEFAULT_RETENTION_DAYS);

        let configured =
            Cli::try_parse_from(["mac-cleanup", "--tmp-retention-days", "30"]).unwrap();
        assert_eq!(configured.tmp_retention_days, 30);

        assert!(Cli::try_parse_from(["mac-cleanup", "--tmp-retention-days", "0"]).is_err());

        let relocation = Cli::try_parse_from([
            "mac-cleanup",
            "--relocate",
            "/Users/me/Library/Data",
            "--relocate-to",
            "/Volumes/EXT_DISK/MacCleanup",
            "--yes",
        ])
        .unwrap();
        assert!(relocation.relocate.is_some());
        assert!(relocation.relocate_to.is_some());
    }
}
