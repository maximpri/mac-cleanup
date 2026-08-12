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
    about = "Find and safely clear known removable data on macOS",
    after_help = "Cleanup is permanent. Close related apps first, and never run this command with sudo."
)]
pub struct Cli {
    /// Keep the entire run read-only (the TUI defaults to analysis but can opt into cleanup)
    #[arg(long, conflicts_with = "clean")]
    pub analyze: bool,

    /// Select and clear eligible removable-data contents
    #[arg(long, conflicts_with = "analyze")]
    pub clean: bool,

    /// Include items that may require a large download to restore
    #[arg(long)]
    pub include_reinstallable: bool,

    /// Scan known waste locations on this mounted volume or home directory
    #[arg(long, value_name = "PATH")]
    pub volume: Option<PathBuf>,

    /// Clear every eligible item without interactive confirmation
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
