use std::path::{Path, PathBuf};

use crate::{
    cache::{
        CacheEntry, CacheStatus, CleanupOutcome, CleanupStats, clean_cache, format_kb, free_kb,
        scan_cache, scan_specs, validate_scan_root,
    },
    cli::{Cli, Mode},
};

pub fn run(cli: &Cli, home: &Path) -> Result<i32, String> {
    let scan_root = cli
        .volume
        .as_deref()
        .map(validate_scan_root)
        .transpose()?
        .unwrap_or_else(|| home.to_path_buf());
    let specs = scan_specs(&scan_root, home);
    let mut entries: Vec<_> = specs
        .iter()
        .map(|spec| scan_cache(spec, cli.include_reinstallable))
        .collect();

    println!("Mac Cleanup");
    println!(
        "Mode: {}",
        match cli.mode() {
            Mode::Analyze => "analysis only (no changes)",
            Mode::Clean => "cleanup with safety checks",
        }
    );
    println!("Scan location: {}\n", scan_root.display());
    println!("STATUS     REMOVABLE DATA                  SIZE  DETAILS");

    let mut shown = 0;
    for entry in &entries {
        let should_show = entry.size_kb > 0
            || cli.verbose
            || matches!(
                entry.status,
                CacheStatus::InUse | CacheStatus::Symlink | CacheStatus::Invalid
            );
        if !should_show {
            continue;
        }
        shown += 1;
        println!(
            "{:<10} {:<25} {:>10}  {}",
            entry.status.label(),
            entry.spec.label,
            format_kb(entry.size_kb),
            entry.spec.note
        );
        if cli.verbose {
            println!("             {}", entry.spec.path.display());
        }
    }
    if shown == 0 {
        println!("No removable data was found in known waste locations.");
    }

    print_totals(&entries);

    if cli.mode() == Mode::Analyze {
        println!("\nNo files were changed. Run with --clean to select eligible items.");
        return Ok(0);
    }

    if !cli.yes {
        println!(
            "\nNo files were changed. Interactive cleanup needs the TUI; remove --no-tui or use --yes."
        );
        return Ok(0);
    }

    let ready_count = entries
        .iter()
        .filter(|entry| entry.status == CacheStatus::Ready && entry.size_kb > 0)
        .count();
    if ready_count == 0 {
        println!("\nNothing is currently eligible for cleanup. No files were changed.");
        return Ok(0);
    }

    println!("\nCleaning {ready_count} eligible item(s)...");
    let allowlist: Vec<PathBuf> = specs.iter().map(|spec| spec.path.clone()).collect();
    let before = free_kb(&scan_root);
    let mut stats = CleanupStats::default();
    for entry in &mut entries {
        if entry.status != CacheStatus::Ready || entry.size_kb == 0 {
            continue;
        }
        match clean_cache(entry, &allowlist, cli.include_reinstallable) {
            CleanupOutcome::Cleared(kb) => {
                stats.cleared += 1;
                stats.measured_removed_kb += kb;
                println!(
                    "DONE       {:<25} reclaimed about {}",
                    entry.spec.label,
                    format_kb(kb)
                );
            }
            CleanupOutcome::SafetySkipped(reason) => {
                stats.safety_skipped += 1;
                println!("SKIPPED    {:<25} {reason}", entry.spec.label);
            }
            CleanupOutcome::Failed(error) => {
                stats.failed += 1;
                eprintln!("FAILED     {:<25} {error}", entry.spec.label);
            }
        }
    }
    stats.filesystem_change_kb = free_kb(&scan_root).saturating_sub(before);
    print_summary(stats);

    Ok(i32::from(stats.failed > 0))
}

fn print_totals(entries: &[CacheEntry]) {
    let found_kb: u64 = entries.iter().map(|entry| entry.size_kb).sum();
    let found_count = entries.iter().filter(|entry| entry.size_kb > 0).count();
    let ready_kb: u64 = entries
        .iter()
        .filter(|entry| entry.status == CacheStatus::Ready)
        .map(|entry| entry.size_kb)
        .sum();
    let ready_count = entries
        .iter()
        .filter(|entry| entry.status == CacheStatus::Ready && entry.size_kb > 0)
        .count();
    let optional_kb: u64 = entries
        .iter()
        .filter(|entry| entry.status == CacheStatus::Optional)
        .map(|entry| entry.size_kb)
        .sum();
    let optional_count = entries
        .iter()
        .filter(|entry| entry.status == CacheStatus::Optional && entry.size_kb > 0)
        .count();

    println!(
        "\nFound:          {} in {found_count} item(s)",
        format_kb(found_kb)
    );
    println!(
        "Ready to clean: {} in {ready_count} item(s)",
        format_kb(ready_kb)
    );
    if optional_count > 0 {
        println!(
            "Needs opt-in:   {} in {optional_count} item(s)",
            format_kb(optional_kb)
        );
    }
}

fn print_summary(stats: CleanupStats) {
    println!("\nSummary");
    println!(
        "Cache data removed:      about {}",
        format_kb(stats.measured_removed_kb)
    );
    println!(
        "Filesystem space change: about {}",
        format_kb(stats.filesystem_change_kb)
    );
    println!("Cleared:                  {}", stats.cleared);
    println!("Skipped for safety:       {}", stats.safety_skipped);
    println!("Failed:                   {}", stats.failed);
}
