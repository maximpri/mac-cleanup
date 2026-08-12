use std::{
    cmp::Reverse,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    cache::{
        CacheEntry, CacheStatus, CacheTier, CleanupOutcome, CleanupStats, clean_cache, format_kb,
        free_kb, scan_cache, scan_location_kind, scan_specs, validate_scan_root,
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
    entries.sort_by_key(|entry| Reverse(entry.size_kb));

    if !cli.json {
        print_report(cli, &scan_root, &entries);
    }

    if cli.mode() == Mode::Analyze {
        if cli.json {
            print_json_report(cli, &scan_root, &entries, None)?;
        } else {
            println!(
                "\nNo files were changed. REVIEW items are excluded from ordinary and unattended cleanup.\nRun the clean-mode TUI for guarded single-item review deletion."
            );
        }
        return Ok(0);
    }

    if !cli.yes {
        if cli.json {
            print_json_report(
                cli,
                &scan_root,
                &entries,
                Some(JsonCleanup::not_performed(
                    "unattended cleanup requires --yes",
                )),
            )?;
        } else {
            println!(
                "\nNo files were changed. Interactive cleanup needs the TUI; remove --no-tui or use --yes."
            );
        }
        return Ok(0);
    }

    let ready_count = entries
        .iter()
        .filter(|entry| entry.status == CacheStatus::Ready && entry.size_kb > 0)
        .count();
    if ready_count == 0 {
        if cli.json {
            print_json_report(
                cli,
                &scan_root,
                &entries,
                Some(JsonCleanup::not_performed(
                    "nothing is currently eligible for cleanup",
                )),
            )?;
        } else {
            println!("\nNothing is currently eligible for cleanup. No files were changed.");
        }
        return Ok(0);
    }

    if !cli.json {
        println!("\nCleaning {ready_count} eligible item(s)...");
    }
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
                if !cli.json {
                    println!(
                        "DONE       {:<25} reclaimed about {}",
                        entry.spec.label,
                        format_kb(kb)
                    );
                }
            }
            CleanupOutcome::SafetySkipped(reason) => {
                stats.safety_skipped += 1;
                if !cli.json {
                    println!("SKIPPED    {:<25} {reason}", entry.spec.label);
                }
            }
            CleanupOutcome::Failed { error, removed_kb } => {
                stats.failed += 1;
                stats.measured_removed_kb += removed_kb;
                if !cli.json && removed_kb > 0 {
                    eprintln!(
                        "PARTIAL    {:<25} removed about {} before failing: {error}",
                        entry.spec.label,
                        format_kb(removed_kb)
                    );
                } else if !cli.json {
                    eprintln!("FAILED     {:<25} {error}", entry.spec.label);
                }
            }
        }
    }
    stats.filesystem_change_kb = free_kb(&scan_root).saturating_sub(before);

    if cli.json {
        print_json_report(
            cli,
            &scan_root,
            &entries,
            Some(JsonCleanup::performed(stats)),
        )?;
    } else {
        print_summary(stats);
    }

    Ok(i32::from(stats.failed > 0))
}

fn print_report(cli: &Cli, scan_root: &Path, entries: &[CacheEntry]) {
    println!("Mac Cleanup");
    println!(
        "Mode: {}",
        match cli.mode() {
            Mode::Analyze => "analysis only (no changes)",
            Mode::Clean => "cleanup with safety checks",
        }
    );
    println!("Scan location: {}", scan_root.display());
    println!("Storage type: {}\n", scan_location_kind(scan_root).label());
    println!("STATUS     STORAGE ITEM                    SIZE  DETAILS");

    let mut shown = 0;
    for entry in entries {
        let should_show = entry.size_kb > 0
            || cli.verbose
            || matches!(
                entry.status,
                CacheStatus::InUse
                    | CacheStatus::ScanError
                    | CacheStatus::Symlink
                    | CacheStatus::Invalid
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
        println!("No reclaimable or app-managed storage was found.");
    }

    print_totals(entries);
}

#[derive(Debug, Clone, Copy, Serialize)]
struct Totals {
    identified_kb: u64,
    identified_items: usize,
    cleanable_kb: u64,
    cleanable_items: usize,
    opt_in_kb: u64,
    opt_in_items: usize,
    review_kb: u64,
    review_items: usize,
}

impl Totals {
    fn from_entries(entries: &[CacheEntry]) -> Self {
        Self {
            identified_kb: entries.iter().map(|entry| entry.size_kb).sum(),
            identified_items: entries.iter().filter(|entry| entry.size_kb > 0).count(),
            cleanable_kb: entries
                .iter()
                .filter(|entry| entry.status == CacheStatus::Ready)
                .map(|entry| entry.size_kb)
                .sum(),
            cleanable_items: entries
                .iter()
                .filter(|entry| entry.status == CacheStatus::Ready && entry.size_kb > 0)
                .count(),
            opt_in_kb: entries
                .iter()
                .filter(|entry| entry.status == CacheStatus::Optional)
                .map(|entry| entry.size_kb)
                .sum(),
            opt_in_items: entries
                .iter()
                .filter(|entry| entry.status == CacheStatus::Optional && entry.size_kb > 0)
                .count(),
            review_kb: entries
                .iter()
                .filter(|entry| entry.status == CacheStatus::Review)
                .map(|entry| entry.size_kb)
                .sum(),
            review_items: entries
                .iter()
                .filter(|entry| entry.status == CacheStatus::Review && entry.size_kb > 0)
                .count(),
        }
    }
}

fn print_totals(entries: &[CacheEntry]) {
    let totals = Totals::from_entries(entries);
    println!(
        "\nIdentified:     {} in {} item(s)",
        format_kb(totals.identified_kb),
        totals.identified_items
    );
    println!(
        "Ready to clean: {} in {} item(s)",
        format_kb(totals.cleanable_kb),
        totals.cleanable_items
    );
    if totals.opt_in_items > 0 {
        println!(
            "Needs opt-in:   {} in {} item(s)",
            format_kb(totals.opt_in_kb),
            totals.opt_in_items
        );
    }
    if totals.review_items > 0 {
        println!(
            "Review in apps: {} in {} item(s)",
            format_kb(totals.review_kb),
            totals.review_items
        );
    }
}

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u8,
    mode: &'static str,
    scan_location: String,
    storage_type: &'static str,
    summary: Totals,
    items: Vec<JsonItem<'a>>,
    cleanup: Option<JsonCleanup>,
}

#[derive(Serialize)]
struct JsonItem<'a> {
    label: &'a str,
    path: String,
    tier: &'static str,
    status: &'static str,
    size_kb: u64,
    note: &'a str,
    outcome: Option<JsonOutcome<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
enum JsonOutcome<'a> {
    Cleared { removed_kb: u64 },
    SafetySkipped { reason: &'a str },
    Failed { error: &'a str, removed_kb: u64 },
}

#[derive(Serialize)]
struct JsonCleanup {
    performed: bool,
    reason: Option<String>,
    stats: Option<JsonCleanupStats>,
}

impl JsonCleanup {
    fn not_performed(reason: &str) -> Self {
        Self {
            performed: false,
            reason: Some(reason.into()),
            stats: None,
        }
    }

    fn performed(stats: CleanupStats) -> Self {
        Self {
            performed: true,
            reason: None,
            stats: Some(stats.into()),
        }
    }
}

#[derive(Serialize)]
struct JsonCleanupStats {
    measured_removed_kb: u64,
    filesystem_change_kb: u64,
    cleared: usize,
    safety_skipped: usize,
    failed: usize,
}

impl From<CleanupStats> for JsonCleanupStats {
    fn from(stats: CleanupStats) -> Self {
        Self {
            measured_removed_kb: stats.measured_removed_kb,
            filesystem_change_kb: stats.filesystem_change_kb,
            cleared: stats.cleared,
            safety_skipped: stats.safety_skipped,
            failed: stats.failed,
        }
    }
}

fn print_json_report(
    cli: &Cli,
    scan_root: &Path,
    entries: &[CacheEntry],
    cleanup: Option<JsonCleanup>,
) -> Result<(), String> {
    let report = build_json_report(cli, scan_root, entries, cleanup);
    let json = serde_json::to_string_pretty(&report)
        .map_err(|error| format!("could not serialize JSON report: {error}"))?;
    println!("{json}");
    Ok(())
}

fn build_json_report<'a>(
    cli: &Cli,
    scan_root: &Path,
    entries: &'a [CacheEntry],
    cleanup: Option<JsonCleanup>,
) -> JsonReport<'a> {
    JsonReport {
        schema_version: 1,
        mode: match cli.mode() {
            Mode::Analyze => "analyze",
            Mode::Clean => "clean",
        },
        scan_location: scan_root.to_string_lossy().into_owned(),
        storage_type: match scan_location_kind(scan_root).label() {
            "LOCAL" => "local",
            "USB" => "usb",
            "NETWORK" => "network",
            _ => unreachable!("all storage kinds have stable labels"),
        },
        summary: Totals::from_entries(entries),
        items: entries
            .iter()
            .map(|entry| JsonItem {
                label: entry.spec.label,
                path: entry.spec.path.to_string_lossy().into_owned(),
                tier: match entry.spec.tier {
                    CacheTier::Routine => "routine",
                    CacheTier::Reinstallable => "reinstallable",
                    CacheTier::ReviewOnly => "review_only",
                },
                status: match entry.status {
                    CacheStatus::Ready => "ready",
                    CacheStatus::Optional => "optional",
                    CacheStatus::Review => "review",
                    CacheStatus::InUse => "in_use",
                    CacheStatus::ScanError => "scan_error",
                    CacheStatus::Symlink => "symlink",
                    CacheStatus::Invalid => "invalid",
                    CacheStatus::Missing => "missing",
                },
                size_kb: entry.size_kb,
                note: entry.spec.note,
                outcome: entry.outcome.as_ref().map(|outcome| match outcome {
                    CleanupOutcome::Cleared(removed_kb) => JsonOutcome::Cleared {
                        removed_kb: *removed_kb,
                    },
                    CleanupOutcome::SafetySkipped(reason) => JsonOutcome::SafetySkipped { reason },
                    CleanupOutcome::Failed { error, removed_kb } => JsonOutcome::Failed {
                        error,
                        removed_kb: *removed_kb,
                    },
                }),
            })
            .collect(),
        cleanup,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheSpec;

    #[test]
    fn json_report_has_a_versioned_stable_shape() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: true,
            clean: false,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            json: true,
            no_color: true,
            no_tui: false,
        };
        let entries = vec![CacheEntry {
            spec: CacheSpec {
                home: temp.path().to_path_buf(),
                path: temp.path().join("cache"),
                label: "Test cache",
                tier: CacheTier::Routine,
                process_pattern: "",
                note: "regenerated test data",
            },
            status: CacheStatus::Ready,
            size_kb: 42,
            outcome: None,
        }];

        let value =
            serde_json::to_value(build_json_report(&cli, temp.path(), &entries, None)).unwrap();

        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["mode"], "analyze");
        assert_eq!(value["summary"]["identified_kb"], 42);
        assert_eq!(value["summary"]["cleanable_items"], 1);
        assert_eq!(value["items"][0]["tier"], "routine");
        assert_eq!(value["items"][0]["status"], "ready");
        assert!(value["cleanup"].is_null());
    }
}
