use std::path::PathBuf;

use clap::{ArgAction, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Analyze,
    Clean,
}

#[derive(Debug, Parser)]
#[command(
    name = "mac-cleanup",
    version,
    about = "Safely inspect and clear known, regenerable macOS caches",
    after_help = "Cleanup is permanent. Close related apps first, and never run this command with sudo."
)]
pub struct Cli {
    /// Read-only report (the default)
    #[arg(long, conflicts_with = "clean")]
    pub analyze: bool,

    /// Select and clear eligible cache contents
    #[arg(long, conflicts_with = "analyze")]
    pub clean: bool,

    /// Include tools and runtimes that require a large download to restore
    #[arg(long)]
    pub include_reinstallable: bool,

    /// Scan known cache locations relative to this mounted volume
    #[arg(long, value_name = "PATH")]
    pub volume: Option<PathBuf>,

    /// Clear every eligible cache without interactive confirmation
    #[arg(long, requires = "clean")]
    pub yes: bool,

    /// Show missing candidates and exact paths in plain output
    #[arg(long)]
    pub verbose: bool,

    /// Disable colored output
    #[arg(long, action = ArgAction::SetTrue)]
    pub no_color: bool,

    /// Use line-oriented output even when attached to a terminal
    #[arg(long)]
    pub no_tui: bool,
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
