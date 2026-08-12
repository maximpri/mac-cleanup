use std::process::ExitCode;

use clap::Parser;
use mac_cleanup::{cache::validate_environment, cli::Cli, plain, tui};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let home = match validate_environment() {
        Ok(home) => home,
        Err(error) => {
            eprintln!("Error: {error}.");
            return ExitCode::FAILURE;
        }
    };

    let result = if !cli.no_tui && !cli.yes && tui::can_run() {
        tui::run(&cli, &home)
    } else {
        plain::run(&cli, &home)
    };

    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            eprintln!("Error: {error}.");
            ExitCode::FAILURE
        }
    }
}
