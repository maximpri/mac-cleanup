// SPDX-License-Identifier: GPL-3.0-or-later
use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};

use crate::retention::DEFAULT_RETENTION_DAYS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Analyze,
    Clean,
}

#[derive(Debug, Parser)]
#[command(
    name = "diskray",
    version,
    args_conflicts_with_subcommands = true,
    about = "Understand what fills your Mac's disk, then clear only reviewed, known-removable data",
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

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Read-only commands. Running `diskray` with no command opens the
/// interactive workspace exactly as before.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Explain what fills the disk on one screen: capacity, hidden space,
    /// the largest folders, quick wins, and what to review first
    Why {
        /// Volume or home folder to explain (default: the startup volume)
        #[arg(long, value_name = "PATH")]
        volume: Option<PathBuf>,
        /// Emit machine-readable JSON (schema `diskray.why/1`)
        #[arg(long)]
        json: bool,
        /// Skip the full folder walk; report capacity and cleanup targets only
        #[arg(long)]
        quick: bool,
        /// Do not save this assessment to History
        #[arg(long)]
        no_save: bool,
        /// Compare with the newest complete assessment at least this many days old
        #[arg(long, value_name = "DAYS")]
        since: Option<u64>,
    },
    /// Ask the on-device model one question; it answers with read-only tools
    Ask {
        /// The question, for example "why is my disk almost full?"
        question: String,
        /// Volume or home folder to examine (default: the startup volume)
        #[arg(long, value_name = "PATH")]
        volume: Option<PathBuf>,
        /// Emit machine-readable JSON (schema `diskray.ask/1`)
        #[arg(long)]
        json: bool,
        /// Give up after this many seconds
        #[arg(long, default_value_t = 240, value_name = "SECONDS")]
        timeout: u64,
        /// Evaluation only: treat this folder as the home folder
        #[arg(long, hide = true, value_name = "PATH")]
        home: Option<PathBuf>,
    },
    /// Serve Diskray's read-only tools to coding agents over the Model Context Protocol (stdio)
    Mcp {
        /// Also allow paths under this folder or volume (the home folder is always allowed)
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
    /// Review the newest cleanup proposal saved by an agent, in the interactive app
    Review,
    /// Report build output (node_modules, target, .venv, ...) in projects
    /// untouched for a while; nothing is deleted
    Artifacts {
        /// Search this folder or volume instead of the home folder
        #[arg(long, value_name = "PATH")]
        volume: Option<PathBuf>,
        /// Emit machine-readable JSON (schema `diskray.artifacts/1`)
        #[arg(long)]
        json: bool,
        /// Report projects untouched for at least this many days
        /// (default: `min_age_days` in ~/.config/diskray/projects.toml, or 60)
        #[arg(long, value_name = "DAYS")]
        days: Option<u64>,
    },
    /// List or validate cleanup rule packs
    Rules {
        #[command(subcommand)]
        action: RulesAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum RulesAction {
    /// Show every rule in effect, which pack it came from, and any warnings
    List,
    /// Validate one rule pack file without installing it
    Check {
        /// Path to a `.toml` rule pack
        file: PathBuf,
    },
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
        let defaults = Cli::try_parse_from(["diskray"]).unwrap();
        assert_eq!(defaults.tmp_retention_days, DEFAULT_RETENTION_DAYS);

        let configured = Cli::try_parse_from(["diskray", "--tmp-retention-days", "30"]).unwrap();
        assert_eq!(configured.tmp_retention_days, 30);

        assert!(Cli::try_parse_from(["diskray", "--tmp-retention-days", "0"]).is_err());

        let relocation = Cli::try_parse_from([
            "diskray",
            "--relocate",
            "/Users/me/Library/Data",
            "--relocate-to",
            "/Volumes/EXT_DISK/Diskray",
            "--yes",
        ])
        .unwrap();
        assert!(relocation.relocate.is_some());
        assert!(relocation.relocate_to.is_some());
    }
}
