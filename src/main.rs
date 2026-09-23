// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::ExitCode;

use clap::Parser;
use diskray::{cache::validate_environment, cli::Cli, commands, migrate, plain, tui};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let home = match validate_environment() {
        Ok(home) => home,
        Err(error) => {
            print_error(&cli, &error);
            return ExitCode::FAILURE;
        }
    };

    for moved in migrate::run(&home) {
        if let migrate::Moved::Renamed { from, to } = moved {
            eprintln!("Moved {from} to {to} after the rename to Diskray.");
        }
    }

    if matches!(cli.command, Some(diskray::cli::Command::Review)) {
        let Some(proposal) = diskray::pending::newest(&home, diskray::care::timestamp()) else {
            eprintln!(
                "No cleanup proposal is waiting. Agents save proposals with the propose_cleanup MCP tool."
            );
            return ExitCode::SUCCESS;
        };
        if !tui::can_run() {
            print_error(&cli, "`diskray review` needs an interactive terminal");
            return ExitCode::FAILURE;
        }
        return match tui::run_review(&cli, &home, proposal) {
            Ok(code) => ExitCode::from(code as u8),
            Err(error) => {
                print_error(&cli, &error);
                ExitCode::FAILURE
            }
        };
    }

    if let Some(command) = &cli.command {
        return match commands::run(command, &home) {
            Ok(code) => ExitCode::from(code as u8),
            Err(error) => {
                print_error(&cli, &error);
                ExitCode::FAILURE
            }
        };
    }

    let result = if !cli.json && !cli.no_tui && !cli.yes && cli.relocate.is_none() && tui::can_run()
    {
        tui::run(&cli, &home)
    } else {
        plain::run(&cli, &home)
    };

    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            print_error(&cli, &error);
            ExitCode::FAILURE
        }
    }
}

fn print_error(cli: &Cli, error: &str) {
    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "schema_version": 5,
                "error": error,
            })
        );
    } else {
        eprintln!("Error: {error}.");
    }
}
