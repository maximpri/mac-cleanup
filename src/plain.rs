use std::{
    cmp::Reverse,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    cache::{
        CacheEntry, CacheStatus, CacheTarget, CacheTier, CleanupOutcome, CleanupStats, clean_cache,
        format_kb, free_kb, scan_cache, scan_location_kind, scan_specs, validate_scan_root,
    },
    cli::{Cli, Mode},
    downloads, history,
    processes::{ProcessEntry, ProcessHealth, review_unhealthy_processes},
    relocation::{self, RelocationReport, RelocationStatus},
    retention::TempRetentionScan,
    storage::{StorageInventory, StorageItemKind},
    whitelist::Whitelist,
};

pub fn run(cli: &Cli, home: &Path) -> Result<i32, String> {
    let scan_root = cli
        .volume
        .as_deref()
        .map(validate_scan_root)
        .transpose()?
        .unwrap_or_else(|| PathBuf::from("/"));
    if let Some(source) = cli.relocate.as_deref() {
        let destination_root = cli
            .relocate_to
            .as_deref()
            .ok_or_else(|| "--relocate requires --relocate-to".to_string())?;
        return run_relocation(cli, home, source, destination_root);
    }
    if cli.yes && !cli.clean {
        return Err("--yes requires --clean or --relocate".into());
    }
    let whitelist = Whitelist::load(home);
    let storage_inventory = StorageInventory::scan(&scan_root, home);
    let temp_retention =
        TempRetentionScan::discover_for_scan(&scan_root, home, cli.tmp_retention_days);
    let mut specs = scan_specs(&scan_root, home);
    specs.extend(temp_retention.cache_specs());
    if scan_root == Path::new("/") || scan_root == home {
        specs.extend(downloads::discover_specs(home, cli.tmp_retention_days));
    }
    let mut entries: Vec<_> = specs
        .iter()
        .map(|spec| scan_cache(spec, cli.include_reinstallable))
        .collect();
    for entry in &mut entries {
        if whitelist.protects(&entry.spec.path) {
            entry.status = CacheStatus::Whitelisted;
        }
    }
    entries.sort_by_key(|entry| Reverse(entry.size_kb));
    let process_review = review_unhealthy_processes();

    if !cli.json {
        print_report(
            cli,
            &scan_root,
            &storage_inventory,
            &temp_retention,
            &whitelist,
            &entries,
            &process_review,
        );
    }

    if cli.mode() == Mode::Analyze {
        if cli.json {
            print_json_report(
                cli,
                &scan_root,
                &storage_inventory,
                &temp_retention,
                &entries,
                &process_review,
                None,
            )?;
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
                &storage_inventory,
                &temp_retention,
                &entries,
                &process_review,
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
                &storage_inventory,
                &temp_retention,
                &entries,
                &process_review,
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
        let label = entry_display_label(entry);
        let outcome = clean_cache(entry, &allowlist, cli.include_reinstallable, &whitelist);
        history::record(home, entry, &outcome);
        match outcome {
            CleanupOutcome::Cleared { removed_kb, method } => {
                stats.cleared += 1;
                stats.measured_removed_kb += removed_kb;
                if !cli.json {
                    println!(
                        "DONE       {:<25} reclaimed about {} ({})",
                        label,
                        format_kb(removed_kb),
                        method.explanation(),
                    );
                }
            }
            CleanupOutcome::SafetySkipped(reason) => {
                stats.safety_skipped += 1;
                if !cli.json {
                    println!("SKIPPED    {:<25} {reason}", label);
                }
            }
            CleanupOutcome::Failed { error, removed_kb } => {
                stats.failed += 1;
                stats.measured_removed_kb += removed_kb;
                if !cli.json && removed_kb > 0 {
                    eprintln!(
                        "PARTIAL    {:<25} removed about {} before failing: {error}",
                        label,
                        format_kb(removed_kb)
                    );
                } else if !cli.json {
                    eprintln!("FAILED     {:<25} {error}", label);
                }
            }
        }
    }
    stats.filesystem_change_kb = free_kb(&scan_root).saturating_sub(before);

    if cli.json {
        print_json_report(
            cli,
            &scan_root,
            &storage_inventory,
            &temp_retention,
            &entries,
            &process_review,
            Some(JsonCleanup::performed(stats)),
        )?;
    } else {
        print_summary(stats, home);
    }

    Ok(i32::from(stats.failed > 0))
}

fn run_relocation(
    cli: &Cli,
    home: &Path,
    source: &Path,
    destination_root: &Path,
) -> Result<i32, String> {
    let canonical_source = source.canonicalize().ok();
    let process_pattern = canonical_source.as_deref().and_then(|source| {
        scan_specs(Path::new("/"), home)
            .into_iter()
            .find(|spec| spec.path.canonicalize().ok().as_deref() == Some(source))
            .map(|spec| spec.process_pattern)
    });
    let plan = relocation::plan(source, destination_root, home, process_pattern)?;
    let report = if cli.yes {
        relocation::execute(&plan)
    } else {
        relocation::preview(&plan)
    };

    if cli.json {
        let json = serde_json::json!({
            "schema_version": 4,
            "operation": "relocate",
            "relocation": report,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json)
                .map_err(|error| format!("could not serialize relocation report: {error}"))?
        );
    } else {
        print_relocation_report(&report);
    }
    if cli.yes && report.status == RelocationStatus::Failed {
        Ok(1)
    } else {
        Ok(0)
    }
}

fn print_relocation_report(report: &RelocationReport) {
    println!("Mac Cleanup — relocation");
    println!("Source:      {}", report.source.display());
    println!("Destination: {}", report.destination.display());
    println!(
        "Data:        {} across {} file(s) and {} directory(s)",
        format_kb(report.size_kb),
        report.file_count,
        report.directory_count
    );
    println!("Status:      {:?}", report.status);
    if report.backup_removed {
        println!("Original:    replaced with a verified symlink; backup removed");
    } else if report.status == RelocationStatus::Relocated {
        println!("Original:    replaced with a verified symlink; backup retained for safety");
    }
    if let Some(message) = &report.message {
        println!("Details:     {message}");
    }
}

fn print_report(
    cli: &Cli,
    scan_root: &Path,
    storage_inventory: &StorageInventory,
    temp_retention: &TempRetentionScan,
    whitelist: &Whitelist,
    entries: &[CacheEntry],
    process_review: &Result<Vec<ProcessEntry>, String>,
) {
    println!("Mac Cleanup");
    println!(
        "Mode: {}",
        match cli.mode() {
            Mode::Analyze => "analysis only (no changes)",
            Mode::Clean => "cleanup with safety checks",
        }
    );
    println!("Scan location: {}", scan_root.display());
    println!("Storage type: {}", scan_location_kind(scan_root).label());
    if !whitelist.is_empty() {
        println!(
            "User whitelist: {} pattern(s) from {}",
            whitelist.pattern_count(),
            whitelist
                .source_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "whitelist".into())
        );
    }
    print_storage_inventory(storage_inventory);
    print_temp_retention(temp_retention);
    println!("\nCLEANUP FINDINGS");
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
        let label = entry_display_label(entry);
        println!(
            "{:<10} {:<25} {:>10}  {}",
            entry.status.label(),
            label,
            format_kb(entry.size_kb),
            entry.spec.note
        );
        if cli.verbose {
            println!("             {}", entry.spec.path.display());
        }
    }
    if shown == 0 {
        println!(
            "No allowlisted cleanup findings were found; review DISK USAGE above for other consumers."
        );
    }

    print_totals(entries);
    print_process_review(process_review);
}

fn entry_display_label(entry: &CacheEntry) -> String {
    if entry.spec.target == CacheTarget::ExactPath {
        entry.spec.path.display().to_string()
    } else {
        entry.spec.label.to_string()
    }
}

fn print_storage_inventory(inventory: &StorageInventory) {
    println!("\nDISK USAGE (read-only inventory)");
    if let Some(volume) = &inventory.volume {
        println!(
            "Filesystem: {} ({})",
            volume.filesystem,
            volume.accounting_path.display()
        );
        println!(
            "Capacity: {}   Used: {}   Free: {}",
            format_kb(volume.capacity_kb),
            format_kb(volume.disk_used_kb()),
            format_kb(volume.disk_free_kb())
        );
        if let Some(container_free_kb) = volume.container_free_kb {
            println!(
                "APFS container free: {} · accounted volume: {} · other volumes / metadata: {}",
                format_kb(container_free_kb),
                format_kb(volume.used_kb),
                format_kb(volume.other_volume_kb())
            );
        }
        println!(
            "Directory inventory: {} on the accounted filesystem; {} item(s) inspected overall",
            format_kb(inventory.scanned_on_volume_kb),
            inventory.scanned_items
        );
        if !inventory.local_snapshots.is_empty() {
            println!(
                "Local Time Machine snapshots: {} (latest {}; report only — `tmutil deletelocalsnapshots <date>` removes one)",
                inventory.local_snapshots.len(),
                inventory.local_snapshots.last().expect("non-empty list")
            );
        }
        if inventory.unaccounted_kb > 0 {
            println!(
                "Not covered by the directory walk: {} (other directories, APFS snapshots, protected/system-managed data, or unreadable paths may contribute)",
                format_kb(inventory.unaccounted_kb)
            );
        } else if inventory.inventory_overage_kb > 0 {
            println!(
                "Directory inventory exceeds df by {} (APFS volume accounting or concurrent activity can cause this)",
                format_kb(inventory.inventory_overage_kb)
            );
        }
    } else if let Some(error) = &inventory.volume_error {
        println!("Filesystem accounting unavailable: {error}");
    } else {
        println!("Filesystem accounting unavailable.");
    }
    println!(
        "Scan status: {} • {} item(s) inspected • {} error(s)",
        if inventory.complete {
            "complete"
        } else {
            "incomplete"
        },
        inventory.scanned_items,
        inventory.scan_errors
    );
    if !inventory.scan_error_paths.is_empty() {
        println!(
            "Unreadable paths (first {}):",
            inventory.scan_error_paths.len()
        );
        for path in &inventory.scan_error_paths {
            println!("  {}", path.display());
        }
    }

    if !inventory.top_level.is_empty() {
        println!("\nTOP-LEVEL STORAGE");
        for item in &inventory.top_level {
            println!(
                "{:>10}  {:<15}  {}",
                format_kb(item.size_kb),
                item.category.label(),
                item.path.display()
            );
        }
    }
    if !inventory.largest.is_empty() {
        println!(
            "\nLARGEST STORAGE CONSUMERS (REVIEW ONLY; never auto-deleted; nested sizes overlap)"
        );
        for item in &inventory.largest {
            let kind = match item.kind {
                StorageItemKind::Directory => "dir",
                StorageItemKind::File => "file",
                StorageItemKind::Symlink => "link",
            };
            println!(
                "{:>10}  {:<4} {:<15}  {}",
                format_kb(item.size_kb),
                kind,
                item.category.label(),
                item.path.display()
            );
        }
    }
    if inventory.top_level.is_empty() && inventory.largest.is_empty() {
        println!("No directory entries were available to inventory.");
    }
}

fn print_temp_retention(report: &TempRetentionScan) {
    println!("\nTEMP RETENTION POLICY");
    if !report.enabled {
        println!(
            "Not active for this scan location; this policy applies to direct children of /private/tmp."
        );
        return;
    }

    let root = report
        .root
        .as_deref()
        .map_or_else(|| "/private/tmp".into(), |path| path.display().to_string());
    println!(
        "Scope: direct children of {root} older than {} day(s); only fully user-owned, unopened entries qualify.",
        report.retention_days
    );
    if let Some(error) = &report.open_check_error {
        println!("Eligibility check unavailable; nothing will be removed: {error}");
    }
    println!(
        "Eligible: {} in {} item(s) • skipped recent: {} • open: {} • not user-owned: {} • symlinks: {} • errors: {}",
        format_kb(report.candidate_kb()),
        report.candidates.len(),
        report.skipped_recent,
        report.skipped_open,
        report.skipped_not_user_owned,
        report.skipped_symlink,
        report.scan_errors
    );
    for candidate in &report.candidates {
        let kind = match candidate.kind {
            crate::retention::TempRetentionKind::File => "file",
            crate::retention::TempRetentionKind::Directory => "dir",
        };
        println!(
            "  {:>10}  {:>3} day(s)  {:<4}  {}",
            format_kb(candidate.size_kb),
            candidate.age_days,
            kind,
            candidate.path.display()
        );
    }
    if !report.complete && report.open_check_error.is_none() {
        println!(
            "Some temporary entries could not be evaluated; those entries were left untouched."
        );
    }
}

fn print_process_review(process_review: &Result<Vec<ProcessEntry>, String>) {
    println!("\nPROCESS     PID      ELAPSED  COMMAND");
    match process_review {
        Ok(entries) if entries.is_empty() => {
            println!(
                "HEALTHY       -            -  No explicitly stuck, stopped, or dead processes found."
            );
        }
        Ok(entries) => {
            for entry in entries {
                println!(
                    "{:<11} {:>6} {:>12}  {}",
                    entry.health.label(),
                    entry.pid,
                    entry.elapsed,
                    entry.command
                );
                println!("                         {}", entry.health.explanation());
            }
            println!(
                "Review only: process signals are never sent by unattended cleanup; use p in the TUI."
            );
        }
        Err(error) => println!("SCAN ERROR    -            -  {error}"),
    }
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
    protected_kb: u64,
    protected_items: usize,
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
            protected_kb: entries
                .iter()
                .filter(|entry| entry.status == CacheStatus::Whitelisted)
                .map(|entry| entry.size_kb)
                .sum(),
            protected_items: entries
                .iter()
                .filter(|entry| entry.status == CacheStatus::Whitelisted && entry.size_kb > 0)
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
    if totals.protected_items > 0 {
        println!(
            "Protected:      {} in {} item(s) matched the user whitelist",
            format_kb(totals.protected_kb),
            totals.protected_items
        );
    }
}

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u8,
    mode: &'static str,
    scan_location: String,
    storage_type: &'static str,
    storage_inventory: &'a StorageInventory,
    temp_retention: &'a TempRetentionScan,
    summary: Totals,
    items: Vec<JsonItem<'a>>,
    process_review: JsonProcessReview<'a>,
    cleanup: Option<JsonCleanup>,
}

#[derive(Serialize)]
struct JsonProcessReview<'a> {
    error: Option<&'a str>,
    unhealthy_count: usize,
    items: Vec<JsonProcessItem<'a>>,
}

#[derive(Serialize)]
struct JsonProcessItem<'a> {
    pid: u32,
    parent_pid: u32,
    state: &'a str,
    health: ProcessHealth,
    elapsed: &'a str,
    cpu_percent: &'a str,
    command: &'a str,
    signalable: bool,
    signal_block_reason: Option<&'a str>,
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
    storage_inventory: &StorageInventory,
    temp_retention: &TempRetentionScan,
    entries: &[CacheEntry],
    process_review: &Result<Vec<ProcessEntry>, String>,
    cleanup: Option<JsonCleanup>,
) -> Result<(), String> {
    let report = build_json_report(
        cli,
        scan_root,
        storage_inventory,
        temp_retention,
        entries,
        process_review,
        cleanup,
    );
    let json = serde_json::to_string_pretty(&report)
        .map_err(|error| format!("could not serialize JSON report: {error}"))?;
    println!("{json}");
    Ok(())
}

fn build_json_report<'a>(
    cli: &Cli,
    scan_root: &Path,
    storage_inventory: &'a StorageInventory,
    temp_retention: &'a TempRetentionScan,
    entries: &'a [CacheEntry],
    process_review: &'a Result<Vec<ProcessEntry>, String>,
    cleanup: Option<JsonCleanup>,
) -> JsonReport<'a> {
    JsonReport {
        schema_version: 5,
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
        storage_inventory,
        temp_retention,
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
                    CacheStatus::Whitelisted => "protected",
                },
                size_kb: entry.size_kb,
                note: entry.spec.note,
                outcome: entry.outcome.as_ref().map(|outcome| match outcome {
                    CleanupOutcome::Cleared { removed_kb, .. } => JsonOutcome::Cleared {
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
        process_review: match process_review {
            Ok(processes) => JsonProcessReview {
                error: None,
                unhealthy_count: processes.len(),
                items: processes
                    .iter()
                    .map(|process| JsonProcessItem {
                        pid: process.pid,
                        parent_pid: process.parent_pid,
                        state: &process.state,
                        health: process.health,
                        elapsed: &process.elapsed,
                        cpu_percent: &process.cpu_percent,
                        command: &process.command,
                        signalable: process.signalable,
                        signal_block_reason: process.signal_block_reason.as_deref(),
                    })
                    .collect(),
            },
            Err(error) => JsonProcessReview {
                error: Some(error),
                unhealthy_count: 0,
                items: Vec::new(),
            },
        },
        cleanup,
    }
}

fn print_summary(stats: CleanupStats, home: &Path) {
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
    println!(
        "Deletions log:            {}",
        history::log_path(home).display()
    );
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
            tmp_retention_days: 7,
            volume: None,
            relocate: None,
            relocate_to: None,
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
                target: crate::cache::CacheTarget::DirectoryContents,
            },
            status: CacheStatus::Ready,
            size_kb: 42,
            outcome: None,
            identity: None,
        }];

        let process_review = Ok(Vec::new());
        let storage_inventory = StorageInventory::scan(temp.path(), temp.path());
        let temp_retention = TempRetentionScan::discover(temp.path(), 7);
        let value = serde_json::to_value(build_json_report(
            &cli,
            temp.path(),
            &storage_inventory,
            &temp_retention,
            &entries,
            &process_review,
            None,
        ))
        .unwrap();

        assert_eq!(value["schema_version"], 5);
        assert_eq!(value["mode"], "analyze");
        assert!(value["storage_inventory"]["volume"].is_object());
        assert!(!value["temp_retention"]["enabled"].as_bool().unwrap());
        assert_eq!(value["summary"]["identified_kb"], 42);
        assert_eq!(value["summary"]["cleanable_items"], 1);
        assert_eq!(value["items"][0]["tier"], "routine");
        assert_eq!(value["items"][0]["status"], "ready");
        assert_eq!(value["process_review"]["unhealthy_count"], 0);
        assert!(value["cleanup"].is_null());
    }
}
