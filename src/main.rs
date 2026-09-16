use std::process::ExitCode;

use clap::Parser;
use mac_cleanup::{cache::validate_environment, cli::Cli, plain, tui};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let home = match validate_environment() {
        Ok(home) => home,
        Err(error) => {
            print_error(&cli, &error);
            return ExitCode::FAILURE;
        }
    };

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
