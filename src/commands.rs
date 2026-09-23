// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only subcommands. None of them can change files.

use crate::{
    agent, ai,
    cache::{CacheTier, format_kb, validate_scan_root},
    care::{self, Target},
    cli::{Command, RulesAction},
    growth,
    headless::{self, Options, Snapshot},
    investigation::{self, CasePhase},
    rules,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// Exit codes for `ask`.
pub const EXIT_AI_UNAVAILABLE: i32 = 3;
pub const EXIT_TIMED_OUT: i32 = 4;

/// Run a subcommand and return the process exit code.
pub fn run(command: &Command, home: &Path) -> Result<i32, String> {
    match command {
        Command::Why {
            volume,
            json,
            quick,
            no_save,
            since,
        } => why(home, volume.as_deref(), *json, *quick, *no_save, *since),
        Command::Ask {
            question,
            volume,
            json,
            timeout,
            home: evaluation_home,
        } => {
            // Evaluation fixtures stand in for the home folder. `ask` is
            // read-only, so this changes only what is measured and where the
            // History record is written.
            let evaluation_home = evaluation_home
                .as_deref()
                .map(validate_scan_root)
                .transpose()?;
            ask(
                evaluation_home.as_deref().unwrap_or(home),
                question,
                volume.as_deref(),
                *json,
                *timeout,
            )
        }
        Command::Mcp { root } => {
            let root = root
                .as_deref()
                .map(validate_scan_root)
                .transpose()?
                .unwrap_or_else(|| PathBuf::from("/"));
            crate::mcp::run(home, &root)
        }
        Command::Review => Err("`diskray review` needs an interactive terminal".into()),
        Command::Artifacts { volume, json, days } => {
            let base = volume.as_deref().map(validate_scan_root).transpose()?;
            Ok(artifacts(home, base.as_deref(), *json, *days))
        }
        Command::Rules { action } => match action {
            RulesAction::List => Ok(list_rules(home)),
            RulesAction::Check { file } => check_rules(file),
        },
    }
}

fn artifacts(home: &Path, base: Option<&Path>, as_json: bool, days: Option<u64>) -> i32 {
    let mut config = crate::artifacts::Config::load(home);
    let search = base.unwrap_or(home);
    if let Some(days) = days {
        config.min_age_days = days;
    }
    if !as_json && io::stderr().is_terminal() {
        eprint!("Looking for build output in untouched projects…\r");
    }
    let report =
        crate::artifacts::find(search, &config, &std::sync::atomic::AtomicBool::new(false));
    clear_progress(as_json);
    if as_json {
        println!("{}", report.to_json());
        return 0;
    }
    if report.roots.is_empty() {
        println!("No project folders found.\nSet `roots` in ~/.config/diskray/projects.toml.");
        return 0;
    }
    if report.artifacts.is_empty() {
        println!(
            "No build output over 10 MB in projects untouched for {}+ days.",
            config.min_age_days
        );
    } else {
        println!(
            "{} of build output in projects untouched for {}+ days:\n",
            format_kb(report.total_kb()),
            config.min_age_days
        );
        let display = |path: &Path| crate::why::display_path(path, search, home);
        for line in report.lines(display, 50) {
            println!("  {line}");
        }
    }
    if !report.complete {
        println!("\nPartial: the search stopped at its time or size limit.");
    }
    println!(
        "\nNothing was deleted. Diskray reports these; remove them yourself if you no longer need them."
    );
    0
}

fn tier_label(tier: CacheTier) -> &'static str {
    match tier {
        CacheTier::Routine => "routine",
        CacheTier::Reinstallable => "reinstallable",
        CacheTier::ReviewOnly => "review",
    }
}

fn list_rules(home: &Path) -> i32 {
    let set = rules::load_for(home);
    let disabled = crate::care::disabled_packs(home);
    println!("{} rules in effect", set.rules.len());
    let mut pack = "";
    for rule in &set.rules {
        if rule.pack != pack {
            pack = rule.pack;
            println!("\n{pack}{}", if rule.user { " (your pack)" } else { "" });
        }
        let mut flags = vec![tier_label(rule.tier)];
        if rule.quick_win {
            flags.push("quick win");
        }
        if let Some(native) = rule.native {
            flags.push(native.display);
        }
        println!("  {:<24} ~/{}  [{}]", rule.id, rule.path, flags.join(" · "));
    }
    if !disabled.is_empty() {
        println!("\nDisabled bundled packs: {}", disabled.join(", "));
    }
    if !set.warnings.is_empty() {
        println!("\nWarnings");
        for warning in &set.warnings {
            println!("  {warning}");
        }
    }
    0
}

/// Validate a user pack as it would be loaded, including overlaps with the
/// bundled rules. Exit code 1 means the pack has problems.
fn check_rules(file: &Path) -> Result<i32, String> {
    let text = fs::read_to_string(file).map_err(|error| format!("{}: {error}", file.display()))?;
    // An unmodified bundled pack is checked with bundled permissions (it may
    // name native commands); anything else is checked as a user pack.
    let bundled = rules::BUILTIN_PACKS
        .iter()
        .any(|(_, builtin)| *builtin == text);
    let (pack, parsed, mut problems) = rules::parse_pack(&text, !bundled)
        .map_err(|error| format!("{}: {error}", file.display()))?;
    for rule in &parsed {
        // A pack never conflicts with its own bundled version.
        if let Some(existing) = rules::builtin().rules.iter().find(|existing| {
            existing.pack != pack
                && (Path::new(existing.path).starts_with(rule.path)
                    || Path::new(rule.path).starts_with(existing.path))
        }) {
            problems.push(format!(
                "{}: overlaps bundled rule {} ({})",
                rule.full_id(),
                existing.full_id(),
                existing.path
            ));
        }
    }
    println!(
        "{pack}: {} valid rule(s)",
        parsed.len() - problems.len().min(parsed.len())
    );
    for problem in &problems {
        println!("  problem: {problem}");
    }
    Ok(if problems.is_empty() { 0 } else { 1 })
}

fn scan_root(volume: Option<&Path>) -> Result<PathBuf, String> {
    volume
        .map(validate_scan_root)
        .transpose()
        .map(|root| root.unwrap_or_else(|| PathBuf::from("/")))
}

/// Progress on stderr, only when a person is watching.
fn progress(quiet: bool) -> impl FnMut(&str) {
    let show = !quiet && io::stderr().is_terminal();
    move |line: &str| {
        if show {
            let mut stderr = io::stderr();
            let _ = write!(
                stderr,
                "\r\x1b[2K{}",
                ai::display_text(line).chars().take(100).collect::<String>()
            );
            let _ = stderr.flush();
        }
    }
}

fn clear_progress(quiet: bool) {
    if !quiet && io::stderr().is_terminal() {
        let _ = write!(io::stderr(), "\r\x1b[2K");
    }
}

fn display_path(path: &Path, snapshot: &Snapshot) -> String {
    crate::why::display_path(path, &snapshot.root, &snapshot.home)
}

fn whole_volume(snapshot: &Snapshot) -> bool {
    crate::why::whole_volume(&snapshot.root)
}

fn record(snapshot: &Snapshot, source: &str) -> care::Session {
    let mut session = snapshot.session(source);
    session.root = Some(snapshot.root.display().to_string());
    session.volume_id = care::volume_id(&snapshot.root);
    session.complete = snapshot.complete;
    session.source = source.into();
    session
}

/// The facts behind the `why` card, shared by the text and JSON forms.
struct WhyReport {
    overview: crate::why::Overview,
    growth: Vec<growth::Delta>,
    growth_since: Option<u64>,
    artifacts: Option<crate::artifacts::Report>,
}

impl std::ops::Deref for WhyReport {
    type Target = crate::why::Overview;
    fn deref(&self) -> &Self::Target {
        &self.overview
    }
}

fn why_report(snapshot: &Snapshot, current: &care::Session, since: Option<u64>) -> WhyReport {
    let base = match since {
        Some(days) => growth::baseline(&snapshot.history, current, days),
        None => growth::previous(&snapshot.history, current),
    };
    WhyReport {
        overview: crate::why::overview(
            &snapshot.root,
            &snapshot.home,
            snapshot.volume.as_ref(),
            snapshot.inventory.as_ref(),
            &snapshot.findings,
            4,
        ),
        growth: base
            .map(|base| growth::diff(base, current))
            .unwrap_or_default(),
        growth_since: base.map(|base| base.updated),
        artifacts: None,
    }
}

fn bar(used: u64, capacity: u64, width: usize) -> String {
    let filled = if capacity == 0 {
        0
    } else {
        ((used as f64 / capacity as f64) * width as f64).round() as usize
    };
    format!(
        "{}{}",
        "█".repeat(filled.min(width)),
        "░".repeat(width.saturating_sub(filled))
    )
}

fn date(seconds: u64) -> String {
    crate::history::format_timestamp(std::time::UNIX_EPOCH + Duration::from_secs(seconds))
}

fn why(
    home: &Path,
    volume: Option<&Path>,
    as_json: bool,
    quick: bool,
    no_save: bool,
    since: Option<u64>,
) -> Result<i32, String> {
    let root = scan_root(volume)?;
    let mut options = Options::new(root.clone());
    options.quick = quick;
    // The project search is independent of the assessment; run it alongside.
    // Projects are searched in the home folder for the startup volume, and
    // inside the scanned folder or volume otherwise.
    let artifacts = (!quick).then(|| {
        let config = crate::artifacts::Config::load(home);
        let base = if root == Path::new("/") {
            home.to_path_buf()
        } else {
            root.clone()
        };
        thread::spawn(move || {
            crate::artifacts::find(&base, &config, &std::sync::atomic::AtomicBool::new(false))
        })
    });
    let snapshot = headless::collect(home, &options, progress(as_json));
    clear_progress(as_json);
    let current = record(&snapshot, "why");
    let mut report = why_report(&snapshot, &current, since);
    report.artifacts = artifacts.and_then(|search| search.join().ok());
    if !no_save && snapshot.complete {
        care::save_session(home, &current)
            .map_err(|error| format!("could not save history: {error}"))?;
    }
    if as_json {
        println!("{}", why_json(&snapshot, &report));
        return Ok(0);
    }
    let hidden = report.unaccounted_kb + report.other_volumes_kb;
    let mut lines = vec![
        format!(
            "Diskray · {} · {}",
            display_path(&root, &snapshot),
            date(care::timestamp())
        ),
        "─".repeat(64),
        format!(
            "Disk          {}  {} used · {} free",
            bar(report.used_kb, report.capacity_kb, 20),
            format_kb(report.used_kb),
            format_kb(report.free_kb)
        ),
    ];
    if snapshot.inventory.is_some() {
        lines.push(format!(
            "Measured      {} in folders the scan could read",
            format_kb(report.measured_kb)
        ));
        lines.push(if whole_volume(&snapshot) {
            format!(
                "Hidden        {}  not visible to the scan {} · other volumes {} · {} local snapshot(s)",
                format_kb(hidden),
                format_kb(report.unaccounted_kb),
                format_kb(report.other_volumes_kb),
                report.snapshots
            )
        } else {
            "Hidden        not shown: the scan covered a folder, not a whole volume".into()
        });
    } else {
        lines.push(if quick {
            "Measured      folder walk skipped (--quick)".into()
        } else {
            format!(
                "Measured      folder walk not finished within {}s; hidden space unknown",
                snapshot.elapsed.as_secs()
            )
        });
    }
    if !report.largest.is_empty() {
        lines.push(format!(
            "Largest       {}",
            report
                .largest
                .iter()
                .map(|(path, kb)| format!("{} {}", display_path(path, &snapshot), format_kb(*kb)))
                .collect::<Vec<_>>()
                .join(" · ")
        ));
    }
    let quick_total = report.quick_win_kb();
    lines.push(if report.quick_wins.is_empty() {
        "Quick wins    none ready right now".into()
    } else {
        format!(
            "Quick wins    {}  {}",
            format_kb(quick_total),
            report
                .quick_wins
                .iter()
                .take(4)
                .map(|item| format!("{} {}", item.label, format_kb(item.size_kb)))
                .collect::<Vec<_>>()
                .join(" · ")
        )
    });
    for (index, item) in report.review.iter().enumerate() {
        let (label, kb, reason) = (&item.label, &item.size_kb, &item.reason);
        let size = if *kb > 0 {
            format!(" {}", format_kb(*kb))
        } else {
            String::new()
        };
        lines.push(format!(
            "{}{label}{size} — {}",
            if index == 0 {
                "Look at       "
            } else {
                "              "
            },
            reason.chars().take(90).collect::<String>()
        ));
    }
    if let Some(artifacts) = report
        .artifacts
        .as_ref()
        .filter(|a| !a.artifacts.is_empty())
    {
        lines.push(format!(
            "Stale builds  {} in {} project folder(s) untouched for a while — `diskray artifacts`",
            format_kb(artifacts.total_kb()),
            artifacts.artifacts.len()
        ));
    }
    match report.growth_since {
        Some(when) if report.growth.is_empty() => lines.push(format!(
            "Grew          nothing significant since {}",
            date(when)
        )),
        Some(when) => {
            lines.push(format!("Grew since {}:", date(when)));
            for delta in report.growth.iter().take(5) {
                lines.push(format!(
                    "              {} {}{} → {}",
                    display_path(Path::new(&delta.path), &snapshot),
                    if delta.change_kb() >= 0 { "+" } else { "−" },
                    format_kb(delta.change_kb().unsigned_abs()),
                    format_kb(delta.after_kb)
                ));
            }
        }
        None => {
            lines.push("Grew          no earlier complete assessment to compare with yet".into())
        }
    }
    if !snapshot.complete && !quick {
        lines.push(format!(
            "Partial       stopped after {}s; results cover what was measured",
            snapshot.elapsed.as_secs()
        ));
    }
    lines.push("─".repeat(64));
    lines.push("Nothing was deleted. Run `diskray` to review and act.".into());
    println!("{}", lines.join("\n"));
    Ok(0)
}

fn why_json(snapshot: &Snapshot, report: &WhyReport) -> Value {
    json!({
        "schema": "diskray.why/1",
        "root": snapshot.root,
        "complete": snapshot.complete,
        "elapsed_seconds": snapshot.elapsed.as_secs(),
        "capacity_kb": report.capacity_kb,
        "used_kb": report.used_kb,
        "free_kb": report.free_kb,
        "measured_kb": report.measured_kb,
        "hidden": {
            "not_visible_to_scan_kb": report.unaccounted_kb,
            "other_volumes_kb": report.other_volumes_kb,
            "local_snapshots": report.snapshots,
        },
        "largest": report.largest.iter().map(|(path, kb)| json!({"path": display_path(path, snapshot), "size_kb": kb})).collect::<Vec<_>>(),
        "quick_wins": report.quick_wins.iter().map(|item| json!({"label": item.label, "size_kb": item.size_kb})).collect::<Vec<_>>(),
        "review_first": report.review.iter().map(|item| json!({"label": item.label, "size_kb": item.size_kb, "reason": item.reason})).collect::<Vec<_>>(),
        "growth": {
            "since": report.growth_since,
            "changes": report.growth.iter().map(|delta| json!({
                "path": delta.path,
                "before_kb": delta.before_kb,
                "after_kb": delta.after_kb,
            })).collect::<Vec<_>>(),
        },
        "stale_artifacts": report.artifacts.as_ref().map(|artifacts| json!({
            "total_kb": artifacts.total_kb(),
            "count": artifacts.artifacts.len(),
            "complete": artifacts.complete,
        })),
        "deleted": false,
    })
}

fn ask(
    home: &Path,
    question: &str,
    volume: Option<&Path>,
    as_json: bool,
    timeout: u64,
) -> Result<i32, String> {
    let question: String = ai::display_text(question.trim())
        .chars()
        .take(agent::QUESTION_LIMIT)
        .collect();
    if question.is_empty() {
        return Err("ask needs a question".into());
    }
    let started = Instant::now();
    let deadline = Duration::from_secs(timeout.max(10));
    let framework = ai::framework_status();
    let use_model = matches!(framework, ai::FrameworkStatus::Available { .. });
    let mut options = Options::new(scan_root(volume)?);
    options.deadline = deadline
        .saturating_sub(Duration::from_secs(20))
        .min(Duration::from_secs(120));
    let mut show = progress(as_json);
    let snapshot = headless::collect(home, &options, &mut show);
    let mut case = investigation::InvestigationCase::new_question(&question, 0);
    let mut run = agent::start(&mut case, agent::Subject::default(), false, use_model);
    let suggest = |target: &Target| snapshot.suggestion(target);
    let eligible: Vec<String> = snapshot
        .entries
        .iter()
        .filter_map(|entry| snapshot.suggestion(&Target::Cache(entry.spec.path.clone())))
        .chain(snapshot.processes.iter().filter_map(|process| {
            snapshot.suggestion(&Target::Process(process.pid, process.start_time.clone()))
        }))
        .collect();
    let world = snapshot.world(&suggest, care::online_research_enabled(home));
    let mut finish = None;
    let mut timed_out = false;
    while finish.is_none() {
        if started.elapsed() > deadline {
            timed_out = true;
            break;
        }
        for event in run.step(&mut case, &world, &|id| {
            eligible.iter().any(|known| known == id)
        }) {
            match event {
                agent::Event::Finished(done) => finish = Some(done),
                agent::Event::ApprovalNeeded => run.decline(
                    &mut case,
                    "Administrator-assisted tracing needs the interactive app.",
                ),
                agent::Event::Merge(_) => {}
            }
        }
        if let Some(label) = run.activity() {
            show(&format!("Local AI → {label}"));
        }
        thread::sleep(Duration::from_millis(50));
    }
    clear_progress(as_json);
    let mut session = record(&snapshot, "ask");
    session.investigations.push(case.clone());
    let _ = care::save_session(home, &session);
    let code = if timed_out {
        EXIT_TIMED_OUT
    } else if finish.as_ref().is_some_and(|finish| finish.by_model) {
        0
    } else {
        EXIT_AI_UNAVAILABLE
    };
    if as_json {
        println!(
            "{}",
            ask_json(
                &question,
                &case,
                finish.as_ref(),
                &framework,
                timed_out,
                &snapshot.eligible_suggestions()
            )
        );
        return Ok(code);
    }
    let mut out = vec![format!("Q: {question}")];
    match &finish {
        Some(finish) => {
            out.push(String::new());
            out.push(finish.summary.clone());
            if !finish.by_model {
                out.push(format!(
                    "(Measured result without local AI: {})",
                    ai::display_text(&framework.description())
                ));
            }
            if !finish.suggestions.is_empty() {
                out.push(String::new());
                out.push("Suggested, not executed (review with `diskray`):".into());
                for id in &finish.suggestions {
                    out.push(format!("  {}", suggestion_label(&snapshot, id)));
                }
            }
            for note in &finish.notes {
                out.push(format!("Note: {note}"));
            }
        }
        None => out.push("\nNo answer before the time limit. Evidence collected so far:".into()),
    }
    out.push(String::new());
    out.push("How it was checked".into());
    let cited = finish.as_ref().map(|f| f.cited.clone()).unwrap_or_default();
    for call in &case.tool_calls {
        let evidence = call
            .evidence_id
            .as_deref()
            .and_then(|id| case.evidence_by_id(id));
        out.push(format!(
            "  {} {} {}{}",
            call.evidence_id.as_deref().unwrap_or("—"),
            call.label,
            call.rejected
                .as_deref()
                .map(|reason| format!("· rejected: {reason}"))
                .unwrap_or_else(|| evidence
                    .map(|e| format!("· {}", e.status.label()))
                    .unwrap_or_default()),
            if call
                .evidence_id
                .as_ref()
                .is_some_and(|id| cited.contains(id))
            {
                " · cited"
            } else {
                ""
            }
        ));
        if let Some(evidence) = evidence {
            out.push(format!("      {}", evidence.summary));
        }
    }
    out.push(String::new());
    out.push("Nothing was deleted.".into());
    println!("{}", out.join("\n"));
    Ok(code)
}

fn suggestion_label(snapshot: &Snapshot, id: &str) -> String {
    snapshot
        .findings
        .iter()
        .find(|finding| snapshot.suggestion(&finding.target).as_deref() == Some(id))
        .map(|finding| {
            format!(
                "{} ({})",
                ai::display_text(&finding.title),
                format_kb(finding.size_kb)
            )
        })
        .unwrap_or_else(|| ai::display_text(id))
}

fn ask_json(
    question: &str,
    case: &investigation::InvestigationCase,
    finish: Option<&agent::Finish>,
    framework: &ai::FrameworkStatus,
    timed_out: bool,
    eligible: &[String],
) -> Value {
    json!({
        "schema": "diskray.ask/1",
        "question": question,
        "answer": finish.map(|f| f.summary.clone()),
        "by_model": finish.is_some_and(|f| f.by_model),
        "ai": framework.description(),
        "phase": finish.map(|f| match f.phase { CasePhase::Complete => "complete", _ => "inconclusive" }),
        "timed_out": timed_out,
        "cited": finish.map(|f| f.cited.clone()).unwrap_or_default(),
        "suggested_actions": finish.map(|f| f.suggestions.clone()).unwrap_or_default(),
        "eligible_actions": eligible,
        "notes": finish.map(|f| f.notes.clone()).unwrap_or_default(),
        "tool_calls": case.tool_calls,
        "evidence": case.evidence,
        "deleted": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_volume() -> tempfile::TempDir {
        let volume = tempfile::tempdir().unwrap();
        fs::create_dir_all(volume.path().join(".Trash")).unwrap();
        fs::write(volume.path().join(".Trash/old.bin"), vec![1; 256 * 1024]).unwrap();
        fs::create_dir_all(volume.path().join("Media")).unwrap();
        fs::write(volume.path().join("Media/video.mov"), vec![2; 512 * 1024]).unwrap();
        volume
    }

    #[test]
    fn why_reports_a_fixture_volume_without_deleting_anything() {
        let volume = fixture_volume();
        let home = tempfile::tempdir().unwrap();
        let root = volume.path().canonicalize().unwrap();
        let mut options = Options::new(root.clone());
        options.deadline = Duration::from_secs(60);
        let snapshot = headless::collect(home.path(), &options, |_| {});
        assert!(snapshot.complete);
        let current = record(&snapshot, "why");
        assert_eq!(current.root.as_deref(), Some(root.to_str().unwrap()));
        assert!(current.complete);
        let report = why_report(&snapshot, &current, None);
        assert!(
            report
                .largest
                .iter()
                .any(|(path, _)| path.starts_with(root.join("Media"))),
            "the breakdown follows Media down to the file that fills it"
        );
        assert_eq!(report.unaccounted_kb, 0, "a folder is not a whole volume");
        let json = why_json(&snapshot, &report);
        assert_eq!(json["schema"], "diskray.why/1");
        assert_eq!(json["deleted"], false);
        assert!(root.join(".Trash/old.bin").exists());
    }

    #[test]
    fn ask_without_a_model_reports_measured_facts_and_exit_code_three() {
        let volume = fixture_volume();
        let home = tempfile::tempdir().unwrap();
        let code = ask(
            home.path(),
            "what is using space?",
            Some(&volume.path().canonicalize().unwrap()),
            true,
            60,
        )
        .unwrap();
        assert_eq!(code, EXIT_AI_UNAVAILABLE);
        let saved = care::sessions(home.path());
        let case = &saved[0].investigations[0];
        assert!(
            case.tool_calls
                .iter()
                .any(|call| call.tool == "disk_accounting")
        );
        assert!(volume.path().join(".Trash/old.bin").exists());
    }
}
