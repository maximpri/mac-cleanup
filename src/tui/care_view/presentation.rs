// SPDX-License-Identifier: GPL-3.0-or-later
//! Presentation for the care workspace: evidence first, progressive detail.
use super::*;

pub(in crate::tui) fn render(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    app.hit_regions.borrow_mut().clear();
    app.map_paths.borrow_mut().clear();
    w.hits.borrow_mut().clear();
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(app.color(INK))
                .bg(app.color(BACKGROUND)),
        ),
        area,
    );
    let notice_height = if w.note.is_some() && !w.help { 2 } else { 0 };
    let regions = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(notice_height),
        Constraint::Length(2),
    ])
    .split(area);
    render_navigation(frame, regions[0], app, w);
    render_status(frame, regions[1], app, w);
    let body = regions[2];
    if w.help {
        render_help(frame, body, app, w);
    } else if w.legacy {
        match app.phase {
            Phase::Processes => {
                let split =
                    Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                        .split(body);
                render_process_table(frame, split[0], app);
                render_process_details(frame, split[1], app);
            }
            Phase::RelocationSources => render_relocation_sources(frame, body, app),
            Phase::RelocationDestination => render_relocation_destination(frame, body, app),
            Phase::RelocationPlanning | Phase::Relocating => {
                render_relocation_progress(frame, body, app)
            }
            Phase::RelocationResult => render_relocation_result(frame, body, app),
            Phase::ReviewConfirm => render_review_confirmation(frame, body, app),
            Phase::ProcessConfirm => render_process_confirmation(frame, body, app),
            _ => render_scan_page(frame, body, app),
        }
    } else if w.work.is_some() {
        render_running(frame, body, app, w);
    } else if let Some(text) = &w.asking {
        text_panel(
            frame,
            body,
            app,
            " ASK ABOUT THIS MAC · Enter asks · Esc cancels ",
            format!(
                "Ask one question. Local AI answers on this Mac using read-only tools and the measurements shown here. It cannot change files, run commands, or add anything to your plan.\n\n› {}▏\n\nFor example: Why is my disk almost full? · What is using memory right now? · Which caches can I clear safely?",
                text
            ),
            0,
        );
    } else if w.clearing_history {
        text_panel(
            frame,
            body,
            app,
            " CLEAR LOCAL SESSION HISTORY ",
            format!(
                "Remove saved assessment and action-session records from this Mac.\nThe separate deletion audit log is retained.\n\nType CLEAR and Enter: {}\nEsc cancels.",
                w.acknowledgement
            ),
            0,
        );
    } else if w.reviewing {
        render_review(frame, body, app, w);
    } else if let Some(label) = w.agent.as_ref().and_then(|run| run.approval_label()) {
        text_panel(
            frame,
            body,
            app,
            " APPROVAL NEEDED · nothing runs until you approve ",
            format!(
                "Local AI asked for a read-only filesystem trace\n\n{}\n\nAn 8-second fs_usage trace of fseventsd can separate disk activity from paging. macOS may ask for administrator authorization; this app never sees your password.\n\nTarget: {}\n\nLocal paths and process activity may appear in the diagnostic evidence.\n\n[a] Approve this trace    [Esc] Skip and continue",
                ai::display_text(label),
                w.investigation_case
                    .as_ref()
                    .map(|c| ai::display_text(&c.target))
                    .unwrap_or_else(|| "Current investigation".into()),
            ),
            w.detail_scroll,
        );
    } else if w.answer_open {
        render_answer(frame, body, app, w);
    } else if w.detail || w.coverage {
        render_evidence(frame, body, app, w);
    } else if w.screen == Screen::Overview && body.width >= SPLIT_WIDTH {
        let split = Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)])
            .spacing(1)
            .split(body);
        render_list(frame, split[0], app, w);
        render_evidence(frame, split[1], app, w);
    } else if w.screen == Screen::Overview || body.width < 100 {
        render_list(frame, body, app, w);
    } else {
        let split = Layout::horizontal([Constraint::Percentage(43), Constraint::Percentage(57)])
            .spacing(1)
            .split(body);
        render_list(frame, split[0], app, w);
        render_evidence(frame, split[1], app, w);
    }
    if notice_height > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" › ", Style::default().fg(app.color(BLUE)).bold()),
                Span::raw(ai::display_text(w.note.as_deref().unwrap_or_default())),
            ]))
            .style(Style::default().fg(app.color(AMBER)))
            .wrap(Wrap { trim: false }),
            regions[3],
        );
    }
    render_footer(frame, regions[4], app, w);
    if w.palette.is_some() {
        render_palette(frame, body, app, w);
    }
}

fn render_navigation(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let compact = area.width < 90;
    let plan = if compact {
        format!(" Plan ({}) [p]", w.plan.len())
    } else {
        format!(" Review plan ({}) [p]", w.plan.len())
    };
    let brand = if compact { 3 } else { 10 };
    let tabs = 11 + 10 + 10;
    let free = w.volume.as_ref().map(|v| {
        let ratio = v.disk_used_kb() as f64 / v.capacity_kb.max(1) as f64;
        (
            format!("{} free  ", format_kb(v.disk_free_kb())),
            disk_usage_color(app, ratio),
        )
    });
    let free_width = free
        .as_ref()
        .map(|(text, _)| text.chars().count() as u16)
        .filter(|width| brand + tabs + width + plan.chars().count() as u16 <= area.width)
        .unwrap_or(0);
    let regions = Layout::horizontal([
        Constraint::Length(brand),
        Constraint::Length(11),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Min(0),
        Constraint::Length(free_width),
        Constraint::Length(plan.chars().count() as u16 + 1),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(if compact { " D " } else { " DISKRAY" })
            .style(Style::default().fg(app.color(INK)).bold()),
        regions[0],
    );
    for (i, label) in [" Overview ", " Explore ", " History "].iter().enumerate() {
        button(
            frame,
            regions[i + 1],
            app,
            w,
            label.to_string(),
            Control::Nav(i),
            w.nav == i,
        );
    }
    if let Some((text, color)) = free.filter(|_| free_width > 0) {
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(color)),
            regions[5],
        );
    }
    button(
        frame,
        regions[6],
        app,
        w,
        plan,
        Control::Key(KeyCode::Char('p')),
        w.reviewing || !w.plan.is_empty(),
    );
}

fn meter(ratio: f64, width: usize) -> String {
    let filled = (ratio.clamp(0., 1.) * width as f64).round() as usize;
    format!(
        "{}{}",
        "━".repeat(filled),
        "┄".repeat(width.saturating_sub(filled))
    )
}

/// "3217270" → "3.2M", for counts of scanned files.
fn count_label(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{}K", count / 1_000),
        _ => format!("{:.1}M", count as f64 / 1_000_000.),
    }
}

fn duration_label(seconds: u64) -> String {
    if seconds < 90 {
        format!("{seconds} s")
    } else {
        format!("{} min", (seconds + 30) / 60)
    }
}

/// One status line: what is happening now, with quiet context on the right.
/// Scans report measured work, never an invented percentage.
fn render_status(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let spin = if w.motion && w.focused && !app.no_color {
        ["◐", "◓", "◑", "◒"][(w.started.elapsed().as_millis() / 200) as usize % 4]
    } else {
        "…"
    };
    let elapsed = w
        .assessment_elapsed
        .unwrap_or_else(|| w.started.elapsed())
        .as_secs();
    let (text, color) = if w.work.is_some() {
        (
            format!(
                "{spin} Running your plan · {} · Esc stops safely",
                w.session.state
            ),
            BLUE,
        )
    } else if w.reviewing {
        (
            "Reviewing your plan · nothing has run yet".to_string(),
            AMBER,
        )
    } else if w.stage.starts_with("Results ready") {
        (
            "✓ Plan finished · results are saved in History · r scans again".to_string(),
            MINT,
        )
    } else if let Some(activity) = activity_status(w) {
        (format!("{spin} {activity}"), VIOLET)
    } else if w.complete {
        let items = app.inventory.as_ref().map_or(0, |i| i.scanned_items);
        let unreadable = app.inventory.as_ref().map_or(0, |i| i.scan_errors);
        let mut text = if items > 0 {
            format!(
                "✓ Scanned {} files in {}",
                count_label(items),
                duration_label(elapsed)
            )
        } else {
            "✓ Scan finished".to_string()
        };
        if unreadable > 0 {
            text.push_str(&format!(" · {} unreadable (v)", count_label(unreadable)));
        }
        (text, MINT)
    } else {
        (
            match &w.progress {
                care::AssessmentProgress::Starting => format!("{spin} Starting the scan…"),
                care::AssessmentProgress::Cleanup { completed, total } => {
                    format!("{spin} Checking cleanup targets {completed}/{total}")
                }
                care::AssessmentProgress::Discovering => format!("{spin} Checking file ages"),
                care::AssessmentProgress::Inventory(p) => format!(
                    "{spin} Measuring folders · {} files · {} · {}",
                    count_label(p.items),
                    format_kb(p.size_kb),
                    duration_label(elapsed)
                ),
            },
            BLUE,
        )
    };
    let mut context = Vec::new();
    if app.analysis_only {
        context.push("read-only".to_string());
    }
    if w.metrics.pressure.is_some_and(|p| p == 2 || p == 4) {
        context.push(format!("memory {}", w.metrics.pressure_label()));
    }
    context.push(
        match w.ai_framework {
            ai::FrameworkStatus::Detecting => "AI checking",
            ai::FrameworkStatus::Available { .. } => "AI ready",
            _ => "AI off",
        }
        .to_string(),
    );
    let context = format!("{} ", context.join(" · "));
    let right = (context.chars().count() as u16).min(area.width / 2);
    let parts = Layout::horizontal([Constraint::Min(0), Constraint::Length(right)]).split(area);
    frame.render_widget(
        Paragraph::new(format!(
            " {}",
            truncate_end(&text, parts[0].width.saturating_sub(2) as usize)
        ))
        .style(Style::default().fg(app.color(color))),
        parts[0],
    );
    frame.render_widget(
        Paragraph::new(context)
            .alignment(ratatui::layout::Alignment::Right)
            .style(Style::default().fg(app.color(MUTED))),
        parts[1],
    );
}

/// Cut a sentence at the end with an ellipsis; paths use `truncate_middle`.
fn truncate_end(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

fn activity_status(w: &Workspace) -> Option<String> {
    if let Some(run) = &w.agent {
        let target = w
            .agent_subject
            .as_ref()
            .map(|subject| ai::display_text(&subject.title))
            .or_else(|| {
                w.investigation_case
                    .as_ref()
                    .and_then(|case| case.question.as_deref())
                    .map(|question| format!("“{}”", ai::display_text(question)))
            })
            .unwrap_or_else(|| "measured evidence".into());
        let who = if run.by_model() {
            "Local AI"
        } else {
            "Measured checks"
        };
        let doing = run
            .approval_label()
            .map(|label| format!("waiting for approval · {label}"))
            .or_else(|| run.activity().map(|label| format!("→ {label}")))
            .unwrap_or_else(|| {
                if run.writing_report() {
                    "writing the report".into()
                } else if run.by_model() {
                    "deciding what to check".into()
                } else {
                    "next check".into()
                }
            });
        return Some(format!(
            "{who} · {target} · {}/{} tools · {doing} · {}s",
            run.calls,
            run.budget,
            run.started.elapsed().as_secs()
        ));
    }
    if w.measure.is_some() {
        Some("Measuring the selected folder · read-only".into())
    } else if w.triage_work.is_some() {
        Some("Local AI · prioritizing measured findings · browsing remains available".into())
    } else {
        None
    }
}

/// The model suggested this finding's action and it is still eligible now.
fn suggested(app: &App, w: &Workspace, f: &Finding) -> bool {
    w.report.as_ref().is_some_and(|report| {
        !report.suggestions.is_empty()
            && suggestion_id(app, &f.target).is_some_and(|id| report.suggestions.contains(&id))
    })
}

/// Tool timeline, hypotheses, and evidence for one case.
/// Evidence summaries appear only when `expanded`; otherwise one line per call.
fn investigation_text(
    w: &Workspace,
    case: &investigation::InvestigationCase,
    expanded: bool,
) -> String {
    let cited: &[String] = w
        .report
        .as_ref()
        .filter(|report| report.case_id == case.id)
        .map(|report| report.cited.as_slice())
        .unwrap_or_default();
    let mut lines = vec![format!(
        "{} · {} of {} tool calls",
        case.phase.label(),
        case.decision_count,
        case.decision_budget
    )];
    for call in &case.tool_calls {
        let evidence = call
            .evidence_id
            .as_deref()
            .and_then(|id| case.evidence_by_id(id));
        let mark = match (&call.rejected, evidence) {
            (Some(_), _) => "✗",
            (None, Some(evidence)) if evidence.status.can_support_hypothesis() => "✓",
            _ => "!",
        };
        let chooser = if call.chosen_by_model {
            ""
        } else {
            " · measured"
        };
        lines.push(format!(
            "{mark} {} {}{}{chooser}",
            call.evidence_id.as_deref().unwrap_or("—"),
            call.label,
            match (&call.rejected, evidence) {
                (Some(reason), _) => format!(" · rejected: {reason}"),
                (None, Some(evidence)) => format!(
                    " · {} · {:.1}s{}",
                    evidence.status.label(),
                    call.elapsed_ms as f64 / 1000.,
                    if cited.contains(&evidence.id) {
                        " · cited"
                    } else {
                        ""
                    }
                ),
                _ => String::new(),
            }
        ));
        if let Some(evidence) = evidence.filter(|_| expanded) {
            lines.push(format!("   {}", evidence.summary));
        }
    }
    if let Some(run) = w.agent.as_ref().filter(|run| run.case_id == case.id) {
        for label in run.running_labels() {
            lines.push(format!("… {label} · running"));
        }
        if let Some(label) = run.approval_label() {
            lines.push(format!("⏸ {label} · waiting for your approval"));
        }
    }
    if !expanded
        && !w.detail
        && case
            .tool_calls
            .iter()
            .any(|call| call.evidence_id.is_some())
    {
        lines.push("Enter shows what each call found".into());
    }
    if expanded && !case.hypotheses.is_empty() {
        lines.push(String::new());
        lines.push("Explanations considered:".into());
        for hypothesis in &case.hypotheses {
            lines.push(format!(
                "{} · {}",
                hypothesis.status.label(),
                hypothesis.label
            ));
        }
    }
    lines.join("\n")
}

/// Plain names for the read-only tools.
fn tool_name(tool: &str) -> &str {
    match tool {
        "list_children" => "contents",
        "folder_age" => "file ages",
        "open_handles" => "open files",
        "cleanup_rule" => "cleanup rule",
        "growth_history" => "history",
        "identify_owner" => "owning app",
        "process_details" => "process details",
        "sample_process" => "process sample",
        "memory_state" => "memory",
        "top_processes" => "busy processes",
        "disk_accounting" => "disk accounting",
        "mounted_volumes" => "volumes",
        "reference" | "research" => "Apple notes",
        "list_findings" => "findings",
        other => other,
    }
}

/// "5 checks: contents, cleanup rule, open files, file ages, history".
fn checks_summary(w: &Workspace, case: &investigation::InvestigationCase) -> String {
    let mut names: Vec<&str> = Vec::new();
    for call in &case.tool_calls {
        let name = tool_name(&call.tool);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let running = w
        .agent
        .as_ref()
        .filter(|run| run.case_id == case.id)
        .and_then(|run| run.activity())
        .map(|label| format!("\nChecking now: {label}"))
        .unwrap_or_default();
    if names.is_empty() {
        return format!("Starting…{running}");
    }
    format!(
        "{} check(s): {}{running}\nEnter shows each result.",
        case.tool_calls.len(),
        names.join(", ")
    )
}

/// Evidence summaries without internal IDs or folder handles.
fn findings_text(case: &investigation::InvestigationCase) -> String {
    case.tool_calls
        .iter()
        .filter_map(|call| call.evidence_id.as_deref())
        .filter_map(|id| case.evidence_by_id(id))
        .filter(|evidence| evidence.status.can_support_hypothesis())
        .map(|evidence| format!("• {}", strip_handles(&evidence.summary)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drop handles such as `n2 ` and `p1 ` that only the model needs.
fn strip_handles(text: &str) -> String {
    text.split(' ')
        .filter(|word| {
            let mut chars = word.chars();
            !(matches!(chars.next(), Some('n' | 'p'))
                && word.len() > 1
                && chars.all(|c| c.is_ascii_digit()))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Plain-language names for the suggested actions in a report.
fn suggestion_text(app: &App, w: &Workspace, suggestions: &[String]) -> String {
    let names: Vec<String> = w
        .findings
        .iter()
        .filter(|f| suggestion_id(app, &f.target).is_some_and(|id| suggestions.contains(&id)))
        .map(|f| ai::display_text(&f.title))
        .collect();
    if names.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nAI suggests reviewing: {}. Each is marked AI SUGGESTS in Findings; Space adds it and nothing runs until you confirm.",
            names.join(", ")
        )
    }
}

fn render_answer(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let Some(case) = w
        .investigation_case
        .as_ref()
        .filter(|case| case.question.is_some())
    else {
        text_panel(
            frame,
            area,
            app,
            " ANSWER · Esc back ",
            "No question has been asked in this session. Press / to ask one.".into(),
            0,
        );
        return;
    };
    let mut text = format!(
        "QUESTION\n{}\n",
        ai::display_text(case.question.as_deref().unwrap_or_default())
    );
    match w.report.as_ref().filter(|report| report.case_id == case.id) {
        Some(report) => text.push_str(&format!(
            "\n{}\n{}{}\n",
            if report.by_model {
                "LOCAL AI ANSWER · interpretation of measured evidence"
            } else {
                "MEASURED RESULT · no AI"
            },
            report.summary,
            suggestion_text(app, w, &report.suggestions)
        )),
        None if w.agent.is_some() => {
            text.push_str("\nLocal AI is gathering evidence with read-only tools…\n")
        }
        None => text.push_str("\nNo answer was produced. The evidence below is still measured.\n"),
    }
    text.push_str(&format!(
        "\nHOW IT WAS CHECKED\n{}",
        investigation_text(w, case, true)
    ));
    let follow = follow_ups(case.question.as_deref().unwrap_or_default());
    if w.model_ready() && w.agent.is_none() && !follow.is_empty() {
        text.push_str("\n\nASK NEXT · press a number to edit and ask\n");
        for (index, question) in follow.iter().enumerate() {
            text.push_str(&format!("{}  {question}\n", index + 1));
        }
    }
    if let Some(error) = &w.ai_error {
        text.push_str(&format!("\n\nAI STATUS\n{error}"));
    }
    text_panel(
        frame,
        area,
        app,
        " ANSWER · Esc back · ↑↓ scroll ",
        text,
        w.detail_scroll,
    );
}

fn render_list(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    match w.screen {
        Screen::Overview => render_overview(frame, area, app, w),
        Screen::Explore => render_folders(frame, area, app, w),
        Screen::History => render_history_list(frame, area, app, w),
    }
}

fn queued(w: &Workspace, f: &Finding) -> bool {
    w.plan.iter().any(|a| match (&f.target, a) {
        (Target::Cache(p), Action::Clean(e) | Action::ReviewClean(e)) => p == &e.spec.path,
        (Target::Process(pid, _), Action::Signal(p, _)) => *pid == p.pid,
        _ => false,
    })
}

/// The measured storage item behind a path, when the folder walk saw it.
/// Looks only among the parent's children: the full index can hold
/// millions of entries, and this runs for every row on every frame.
fn storage_item<'a>(app: &'a App, path: &Path) -> Option<&'a StorageItem> {
    let inventory = app.inventory.as_ref()?;
    path.parent()
        .and_then(|parent| inventory.children.get(parent))
        .into_iter()
        .flatten()
        .chain(inventory.top_level.iter())
        .find(|item| item.path == path)
}

/// Who blocks an in-use cache, for example `npx (PID 812)`.
fn blocker_names(w: &Workspace, path: &Path) -> Option<String> {
    w.blockers
        .get(path)
        .filter(|names| !names.is_empty())
        .map(|names| names.join(", "))
}

/// A short, plain verdict for a list row, and its color. Color only
/// repeats what the words say.
fn verdict(app: &App, w: &Workspace, f: &Finding) -> (String, Color) {
    if queued(w, f) {
        return ("✓ In plan".into(), MINT);
    }
    if suggested(app, w, f) {
        return ("AI suggests clearing".into(), VIOLET);
    }
    match &f.target {
        Target::Cache(path) => match app
            .entries
            .iter()
            .find(|e| &e.spec.path == path)
            .map(|e| e.status)
        {
            Some(CacheStatus::Ready) => ("Safe to clear".into(), MINT),
            Some(CacheStatus::Optional) => ("Safe · downloads again".into(), MINT),
            Some(CacheStatus::InUse) => (
                match w.blockers.get(path).and_then(|names| names.first()) {
                    Some(name) => {
                        format!("In use by {}", name.split(" (PID").next().unwrap_or(name))
                    }
                    None => "In use".into(),
                },
                AMBER,
            ),
            Some(CacheStatus::Review) => ("App data · review".into(), AMBER),
            Some(CacheStatus::Whitelisted) => ("Protected by you".into(), MUTED),
            _ => ("Can't read".into(), MUTED),
        },
        Target::Folder(path) => {
            if f.id == care::MACOS_FINDING_ID {
                ("macOS".into(), MUTED)
            } else if f.id.starts_with("growth:") {
                ("Grew".into(), AMBER)
            } else {
                (
                    storage_item(app, path)
                        .map_or("Folder", |item| item.category.verdict())
                        .into(),
                    BLUE,
                )
            }
        }
        Target::Process(..) => {
            let state = f.observation.split(" · ").next().unwrap_or("Running");
            let mut words = state.to_lowercase();
            if let Some(first) = words.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            (format!("{words} app"), BLUE)
        }
        Target::System => ("Needs attention".into(), CORAL),
    }
}

/// What the user can do next, in plain words.
fn next_step(app: &App, w: &Workspace, f: &Finding) -> String {
    if queued(w, f) {
        return "In your plan. Space takes it out; p reviews the plan.".into();
    }
    let act = |text: &str| {
        if app.analysis_only {
            "This session is read-only, so nothing can be added to a plan.".to_string()
        } else {
            text.to_string()
        }
    };
    match &f.target {
        Target::Cache(path) => match app
            .entries
            .iter()
            .find(|e| &e.spec.path == path)
            .map(|e| e.status)
        {
            Some(CacheStatus::Ready | CacheStatus::Optional) => {
                act("Space adds it to your plan. Nothing runs until you review and confirm.")
            }
            Some(CacheStatus::InUse) => format!(
                "Quit {} or wait until {} finished, then press r to scan again.",
                blocker_names(w, path).unwrap_or_else(|| "the related app".into()),
                if w.blockers.get(path).is_some_and(|names| names.len() > 1) {
                    "they have"
                } else {
                    "it has"
                }
            ),
            Some(CacheStatus::Review) => act(
                "Reduce it from the app that owns it. To delete it here anyway, Space adds it as a separate plan that needs a typed DELETE.",
            ),
            Some(CacheStatus::Whitelisted) => {
                "Remove it from ~/.config/diskray/whitelist if you want Diskray to clear it."
                    .into()
            }
            _ => "Give your terminal Full Disk Access (A opens Settings), then press r to scan again."
                .into(),
        },
        Target::Folder(_) => {
            "e browses what is inside · o shows it in Finder · i investigates it.".into()
        }
        Target::Process(..) => {
            act("i investigates it first. Space adds a graceful stop to your plan.")
        }
        Target::System => "i looks for the likely cause.".into(),
    }
}

/// One plain summary of what was measured.
fn summary(app: &App, w: &Workspace, f: &Finding) -> String {
    let Target::Cache(path) = &f.target else {
        return f.observation.clone();
    };
    let size = format_kb(f.size_kb);
    match app
        .entries
        .iter()
        .find(|e| &e.spec.path == path)
        .map(|e| e.status)
    {
        Some(CacheStatus::Ready) => format!(
            "{size}. Nothing related is running, so Diskray can clear it safely."
        ),
        Some(CacheStatus::Optional) => format!(
            "{size}. Safe to clear, but the tool downloads it again the next time it needs it."
        ),
        Some(CacheStatus::InUse) => format!(
            "{size}. Diskray will not clear it while {} {} running.",
            blocker_names(w, path).unwrap_or_else(|| "a related app or tool".into()),
            if w.blockers.get(path).is_some_and(|names| names.len() > 1) {
                "are"
            } else {
                "is"
            }
        ),
        Some(CacheStatus::Review) => format!(
            "{size}. This holds data an app manages, not a plain cache."
        ),
        Some(CacheStatus::Whitelisted) => {
            format!("{size}. You protected this location in your whitelist.")
        }
        _ => "Diskray could not read this location, so its size is unknown and it will not be cleared."
            .into(),
    }
}

/// Capacity as four parts that always add up: space in folders the scan
/// measured, used space it could not see, macOS and other volumes, and free
/// space. Each part has its own glyph, so the bar reads without color.
fn capacity_bar(app: &App, o: &crate::why::Overview, width: usize) -> (Line<'static>, String) {
    let capacity = o.capacity_kb.max(1);
    let system = o.other_volumes_kb.min(o.used_kb);
    let data = o.used_kb - system;
    let (measured, unseen) = if o.folder_walk {
        let unseen = o.unaccounted_kb.min(data);
        (data - unseen, unseen)
    } else {
        (0, data)
    };
    let cells = |kb: u64| ((kb as f64 / capacity as f64) * width as f64).round() as usize;
    let (a, b, c) = (cells(measured), cells(unseen), cells(system));
    let free_cells = width.saturating_sub(a + b + c);
    let bar = Line::from(vec![
        Span::styled("█".repeat(a), Style::default().fg(app.color(BLUE))),
        Span::styled("▓".repeat(b), Style::default().fg(app.color(AMBER))),
        Span::styled("▒".repeat(c), Style::default().fg(app.color(MUTED))),
        Span::styled("░".repeat(free_cells), Style::default().fg(app.color(MINT))),
    ]);
    let legend = if !o.folder_walk {
        format!(
            "{} used · {} free · measuring folders…",
            format_kb(o.used_kb),
            format_kb(o.free_kb)
        )
    } else if o.whole_volume {
        let mut parts = vec![format!("█ folders {}", format_kb(measured))];
        if unseen >= 1_048_576 {
            parts.push(format!("▓ unseen {}", format_kb(unseen)));
        }
        if system >= 1_048_576 {
            parts.push(format!("▒ system {}", format_kb(system)));
        }
        parts.push(format!("░ free {}", format_kb(o.free_kb)));
        parts.join("  ")
    } else {
        format!(
            "█ this folder {}  ▒ rest of disk {}  ░ free {}",
            format_kb(o.measured_kb),
            format_kb(o.used_kb.saturating_sub(o.measured_kb)),
            format_kb(o.free_kb)
        )
    };
    (bar, legend)
}

fn render_palette(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let Some((query, cursor)) = &w.palette else {
        return;
    };
    let matches = palette_matches(query);
    let width = area.width.min(64);
    let height = area.height.min(matches.len() as u16 + 4).max(5);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + 1.min(area.height.saturating_sub(height)),
        width,
        height,
    );
    frame.render_widget(ratatui::widgets::Clear, rect);
    let block = popup_panel(app, " COMMANDS · type to filter · Enter runs · Esc ", BLUE);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let mut lines = vec![Line::styled(
        format!(": {query}▏"),
        Style::default().fg(app.color(INK)).bold(),
    )];
    let visible = inner.height.saturating_sub(1) as usize;
    let start = cursor.saturating_sub(visible.saturating_sub(1));
    if matches.is_empty() {
        lines.push(Line::styled(
            "No matching command",
            Style::default().fg(app.color(MUTED)),
        ));
    }
    for (index, (key, label, _)) in matches.iter().enumerate().skip(start).take(visible) {
        lines.push(
            Line::from(vec![
                Span::styled(
                    format!("{key:>6}  "),
                    Style::default().fg(app.color(BLUE)).bold(),
                ),
                Span::raw(label.to_string()),
            ])
            .style(if index == *cursor {
                selected_row_style(app)
            } else {
                Style::default().fg(app.color(INK))
            }),
        );
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The row name: a cache's label, a folder's readable path.
fn row_name(app: &App, f: &Finding) -> String {
    match &f.target {
        Target::Folder(path) if f.id != care::MACOS_FINDING_ID && !f.id.starts_with("growth:") => {
            crate::why::display_path(path, &app.scan_root, &app.account_home)
        }
        _ => ai::display_text(&f.title),
    }
}

fn render_overview(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let o = w.overview(app);
    let visible = w.visible();
    let title = if w.volume.is_some() {
        format!(" WHERE YOUR {} WENT ", format_kb(o.used_kb))
    } else {
        " WHERE THE SPACE WENT ".to_string()
    };
    let block = panel(app, title).title_bottom(Line::from(format!(
        " {} of {} · f {} ",
        if visible.is_empty() { 0 } else { w.cursor + 1 },
        visible.len(),
        if w.show_all { "fewer" } else { "all" }
    )));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let muted = Style::default().fg(app.color(MUTED));

    let mut header: Vec<Line<'static>> = Vec::new();
    if w.volume.is_some() {
        let (bar, legend) = capacity_bar(app, &o, width.min(72));
        header.push(bar);
        header.push(Line::styled(truncate_end(&legend, width), muted));
    } else {
        header.push(Line::styled(
            if w.complete {
                "Disk capacity unavailable for this location."
            } else {
                "Reading disk capacity…"
            },
            muted,
        ));
    }
    header.push(Line::raw(""));

    let mut footer: Vec<Line<'static>> = Vec::new();
    let wins: Vec<&Finding> = w
        .findings
        .iter()
        .filter(|f| f.quick_win && !w.kept.contains(&f.id))
        .collect();
    if !wins.is_empty() {
        let waiting: Vec<&&Finding> = wins.iter().filter(|f| !queued(w, f)).collect();
        let total: u64 = waiting.iter().map(|f| f.size_kb).sum();
        footer.push(Line::styled(
            truncate_end(
                &if waiting.is_empty() {
                    "✓ All quick wins are in your plan · p reviews it".to_string()
                } else {
                    format!(
                        "Quick wins: {} in {} rebuildable cache(s){}",
                        format_kb(total),
                        waiting.len(),
                        if app.analysis_only {
                            ""
                        } else {
                            " · a adds all"
                        }
                    )
                },
                width,
            ),
            Style::default().fg(app.color(MINT)),
        ));
    }
    match w.growth_since {
        Some(when) if w.growth.is_empty() => footer.push(Line::styled(
            format!(
                "Grew: nothing significant since {}",
                crate::history::format_timestamp(std::time::UNIX_EPOCH + Duration::from_secs(when))
            ),
            muted,
        )),
        Some(_) => footer.push(Line::styled(
            truncate_end(
                &format!(
                    "Grew since last scan: {}",
                    w.growth
                        .iter()
                        .filter(|delta| delta.change_kb() > 0)
                        .take(3)
                        .map(|delta| format!(
                            "{} +{}",
                            crate::why::display_path(
                                Path::new(&delta.path),
                                &app.scan_root,
                                &app.account_home
                            ),
                            format_kb(delta.change_kb().unsigned_abs())
                        ))
                        .collect::<Vec<_>>()
                        .join(" · ")
                ),
                width,
            ),
            Style::default().fg(app.color(AMBER)),
        )),
        None => {}
    }
    let unreadable_targets = app
        .entries
        .iter()
        .filter(|e| e.status == CacheStatus::ScanError)
        .count();
    let unreadable_folders = app.inventory.as_ref().map_or(0, |i| i.scan_errors as usize);
    if unreadable_targets + unreadable_folders > 0 {
        footer.push(Line::styled(
            truncate_end(
                &format!(
                    "Couldn't read {} locations · A: Full Disk Access · v: which",
                    count_label((unreadable_targets + unreadable_folders) as u64)
                ),
                width,
            ),
            muted,
        ));
    }

    let rows_height = (inner.height as usize).saturating_sub(header.len() + footer.len() + 1);
    let mut lines = header.clone();
    let rows_top = inner.y + header.len() as u16;
    if visible.is_empty() {
        lines.push(Line::styled(
            if w.complete {
                if w.kept.is_empty() {
                    "Nothing large enough to list. f shows everything that was measured."
                } else {
                    "You hid the remaining items. r scans again."
                }
            } else {
                "Measuring… items appear here as each check finishes."
            },
            muted,
        ));
    }
    let capacity = rows_height.max(1);
    let start = w.cursor.saturating_sub(capacity - 1);
    for (row, (index, f)) in visible
        .iter()
        .enumerate()
        .skip(start)
        .take(capacity)
        .enumerate()
    {
        let (verdict_text, color) = verdict(app, w, f);
        let verdict_width = if width >= 70 {
            24
        } else if width >= 56 {
            20
        } else {
            15
        };
        let verdict_text = truncate_end(&verdict_text, verdict_width);
        let size = if f.size_kb > 0 {
            format_kb(f.size_kb)
        } else {
            String::new()
        };
        let name_width = width.saturating_sub(2 + 10 + 2 + verdict_width);
        let name = truncate_middle(&row_name(app, f), name_width);
        let selected = index == w.cursor;
        let base = if selected {
            selected_row_style(app)
        } else {
            Style::default().fg(app.color(INK))
        };
        lines.push(Line::from(vec![
            Span::styled(if selected { "› " } else { "  " }, base),
            Span::styled(format!("{name:<name_width$}"), base),
            Span::styled(format!("{size:>10}"), base),
            Span::styled("  ", base),
            Span::styled(verdict_text, base.fg(app.color(color))),
        ]));
        w.hits.borrow_mut().push((
            Rect::new(inner.x, rows_top + row as u16, inner.width, 1),
            Control::Row(index),
        ));
    }
    let used = lines.len();
    let spacer = (inner.height as usize).saturating_sub(used + footer.len());
    lines.extend(std::iter::repeat_n(Line::raw(""), spacer));
    lines.extend(footer);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_folders(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let path = app
        .explorer_path
        .as_ref()
        .map(|p| ai::display_text(&p.display().to_string()))
        .unwrap_or_else(|| "Storage · largest first".into());
    let block = panel(app, " EXPLORE · measured space, not waste ")
        .title_bottom(Line::from(" Enter opens · ← parent · d details "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(truncate_middle(&path, inner.width as usize))
            .style(Style::default().fg(app.color(BLUE))),
        Rect::new(inner.x, inner.y, inner.width, inner.height.min(1)),
    );
    let rows = Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    );
    let items = app.explorer_items();
    if items.is_empty() {
        let text = if !w.complete || w.measure.is_some() {
            "Measuring folder contents…\nTotals will appear when the walk finishes."
        } else {
            "No measured children here.\n← returns to the parent. i measures a selected folder."
        };
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(app.color(MUTED))),
            rows,
        );
        return;
    }
    let max = items.iter().map(|i| i.size_kb).max().unwrap_or(1).max(1);
    let capacity = (rows.height as usize / 2).max(1);
    let start = app.explorer_cursor.saturating_sub(capacity - 1);
    for (row, (index, item)) in items
        .iter()
        .enumerate()
        .skip(start)
        .take(capacity)
        .enumerate()
    {
        let rect = Rect::new(
            rows.x,
            rows.y + row as u16 * 2,
            rows.width,
            2.min(rows.height.saturating_sub(row as u16 * 2)),
        );
        let name = ai::display_text(
            &item
                .path
                .file_name()
                .unwrap_or(item.path.as_os_str())
                .to_string_lossy(),
        );
        let size = format_kb(item.size_kb);
        let lines = vec![
            Line::styled(
                format!(
                    "{} {}  {}",
                    if item.kind == StorageItemKind::Directory {
                        "▸"
                    } else {
                        "·"
                    },
                    truncate_middle(
                        &name,
                        rect.width.saturating_sub(size.len() as u16 + 4) as usize
                    ),
                    size
                ),
                Style::default().fg(app.color(INK)).bold(),
            ),
            Line::styled(
                format!(
                    "  {}  {}",
                    meter(item.size_kb as f64 / max as f64, 12),
                    item.category.verdict()
                ),
                Style::default().fg(app.color(BLUE)),
            ),
        ];
        frame.render_widget(
            Paragraph::new(lines).style(if index == app.explorer_cursor {
                selected_row_style(app)
            } else {
                Style::default()
            }),
            rect,
        );
        w.hits.borrow_mut().push((rect, Control::Row(index)));
    }
}

fn render_history_list(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let block = panel(app, " HISTORY · stored on this Mac ")
        .title_bottom(Line::from(" Enter results · r recheck "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if w.history.is_empty() {
        frame.render_widget(Paragraph::new("No saved sessions yet.\n\nCompleted assessments and action results will appear here.").wrap(Wrap {trim:false}).style(Style::default().fg(app.color(MUTED))), inner);
    }
    let capacity = (inner.height as usize / 3).max(1);
    let start = w.history_cursor.saturating_sub(capacity - 1);
    for (row, (index, s)) in w
        .history
        .iter()
        .enumerate()
        .skip(start)
        .take(capacity)
        .enumerate()
    {
        let rect = Rect::new(
            inner.x,
            inner.y + row as u16 * 3,
            inner.width,
            3.min(inner.height.saturating_sub(row as u16 * 3)),
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    format!(
                        "{} · {} actions",
                        crate::history::format_timestamp(
                            std::time::UNIX_EPOCH + Duration::from_secs(s.updated)
                        ),
                        s.actions.len()
                    ),
                    Style::default().fg(app.color(INK)).bold(),
                ),
                Line::styled(
                    truncate_middle(
                        &match crate::growth::previous(&w.history, s)
                            .map(|base| crate::growth::diff(base, s))
                            .filter(|deltas| !deltas.is_empty())
                        {
                            Some(deltas) => format!(
                                "{} · grew +{} in {} place(s)",
                                s.state,
                                format_kb(
                                    deltas
                                        .iter()
                                        .map(|delta| delta.change_kb().max(0) as u64)
                                        .sum()
                                ),
                                deltas.len()
                            ),
                            None => s.state.clone(),
                        },
                        inner.width as usize,
                    ),
                    Style::default().fg(app.color(MUTED)),
                ),
            ])
            .style(if index == w.history_cursor {
                selected_row_style(app)
            } else {
                Style::default()
            }),
            rect,
        );
        w.hits.borrow_mut().push((rect, Control::Row(index)));
    }
}

fn section(lines: &mut Vec<Line<'static>>, app: &App, label: &str, text: String) {
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        label.to_string(),
        Style::default().fg(app.color(BLUE)).bold(),
    ));
    lines.extend(text.lines().map(|line| Line::raw(ai::display_text(line))));
}

/// A section written by the model, in the AI accent color.
fn ai_section(lines: &mut Vec<Line<'static>>, app: &App, label: &str, text: String) {
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        label.to_string(),
        Style::default().fg(app.color(VIOLET)).bold(),
    ));
    lines.extend(text.lines().map(|line| Line::raw(ai::display_text(line))));
}

fn render_evidence(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    if w.coverage {
        let lines = if app.inventory.is_none() {
            vec![
                Line::raw("Folder-map coverage is not available yet."),
                Line::raw(""),
                Line::raw(
                    "The scan is still measuring. The progress band shows current work; Esc returns to your findings.",
                ),
            ]
        } else {
            coverage_lines(app)
                .into_iter()
                .map(|mut line| {
                    for span in &mut line.spans {
                        span.content = span
                            .content
                            .replace(
                                "Press p to open Full Disk Access settings.",
                                "Press A for macOS Settings, then find Full Disk Access.",
                            )
                            .into();
                    }
                    line
                })
                .collect()
        };
        frame.render_widget(
            Paragraph::new(lines)
                .block(panel(app, " SCAN COVERAGE · Esc back · PgUp/PgDn scroll "))
                .wrap(Wrap { trim: false })
                .scroll((w.detail_scroll, 0)),
            area,
        );
        return;
    }
    if w.screen == Screen::History {
        render_results(frame, area, app, w);
        return;
    }
    let selected_item = if w.screen == Screen::Explore {
        app.explorer_items()
            .get(app.explorer_cursor)
            .map(|i| (*i).clone())
    } else {
        w.selected().and_then(|f| match &f.target {
            Target::Cache(path) | Target::Folder(path) => storage_item(app, path).cloned(),
            _ => None,
        })
    };
    let mut lines = Vec::new();
    if w.screen == Screen::Explore {
        if let Some(item) = &selected_item {
            lines.push(Line::styled(
                ai::display_text(
                    &item
                        .path
                        .file_name()
                        .unwrap_or(item.path.as_os_str())
                        .to_string_lossy(),
                ),
                Style::default().fg(app.color(INK)).bold(),
            ));
            lines.push(Line::styled(
                format!("{} · {}", format_kb(item.size_kb), item.category.verdict()),
                Style::default().fg(app.color(MINT)),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::raw(item.category.description().to_string()));
            section(
                &mut lines,
                app,
                "WHAT YOU CAN DO",
                if item.kind == StorageItemKind::Directory {
                    format!(
                        "Enter opens it · ← goes up · o shows it in Finder.\n{}",
                        item.category.advice()
                    )
                } else {
                    format!("o shows it in Finder.\n{}", item.category.advice())
                },
            );
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("Location: {}", item.path.display()),
                Style::default().fg(app.color(MUTED)),
            ));
        } else {
            lines.push(Line::raw("Select a measured folder to see its details."));
        }
    } else if let Some(f) = w.selected() {
        let (verdict_text, color) = verdict(app, w, f);
        lines.push(Line::styled(
            row_name(app, f),
            Style::default().fg(app.color(INK)).bold(),
        ));
        lines.push(Line::styled(
            verdict_text,
            Style::default().fg(app.color(color)),
        ));
        lines.push(Line::raw(""));
        lines.extend(
            summary(app, w, f)
                .lines()
                .map(|line| Line::raw(ai::display_text(line))),
        );
        if matches!(f.target, Target::Process(..)) || f.id == "system:memory" {
            lines.extend(
                w.metrics.describe().lines().map(|line| {
                    Line::styled(line.to_string(), Style::default().fg(app.color(MUTED)))
                }),
            );
        }
        let case = w
            .investigation_case
            .as_ref()
            .filter(|_| w.agent_subject.as_ref().is_some_and(|s| s.id == f.id));
        let report = w.report.as_ref().filter(|report| {
            report
                .subject
                .as_ref()
                .is_some_and(|subject| subject.id == f.id)
        });
        match (report, case) {
            (Some(report), _) if report.by_model => ai_section(
                &mut lines,
                app,
                "LOCAL AI · AN INTERPRETATION OF THE CHECKS",
                format!(
                    "{}{}",
                    report.summary,
                    suggestion_text(app, w, &report.suggestions)
                ),
            ),
            (_, Some(case)) if !case.evidence.is_empty() => section(
                &mut lines,
                app,
                "WHAT THE CHECKS FOUND",
                findings_text(case),
            ),
            _ => {}
        }
        let clear = match &f.target {
            Target::Cache(_) => format!("\nGood to know: {}", f.consequence),
            _ => format!("\n{}", f.consequence),
        };
        section(
            &mut lines,
            app,
            "WHAT YOU CAN DO",
            format!("{}{clear}", next_step(app, w, f)),
        );
        if let Some(case) = case {
            section(
                &mut lines,
                app,
                "HOW THIS WAS CHECKED",
                if w.detail {
                    investigation_text(w, case, true)
                } else {
                    checks_summary(w, case)
                },
            );
            if let Some(error) = &w.ai_error {
                lines.push(Line::styled(
                    error.clone(),
                    Style::default().fg(app.color(MUTED)),
                ));
            }
        }
        if let Target::Cache(path) | Target::Folder(path) = &f.target
            && f.id != care::MACOS_FINDING_ID
        {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("Location: {}", path.display()),
                Style::default().fg(app.color(MUTED)),
            ));
        }
    } else {
        lines.push(Line::raw(
            "Choose a finding to understand the evidence and next step.",
        ));
    }
    // Compact screens keep every explanation scrollable. Larger panes earn a
    // real measured visualization; never reserve space for an empty map.
    // Folder maps belong to Explore; in the Overview the explanation keeps
    // all the room.
    let disk = false;
    let activity = false;
    let visual_height =
        if area.height >= 25 && w.screen == Screen::Explore && selected_item.is_some() {
            9
        } else {
            0
        };
    let parts =
        Layout::vertical([Constraint::Min(5), Constraint::Length(visual_height)]).split(area);
    let text = Paragraph::new(lines).wrap(Wrap { trim: false });
    let block = panel(
        app,
        if w.detail {
            " DETAILS · Esc back "
        } else {
            " DETAILS · Enter expands "
        },
    );
    let inner = block.inner(parts[0]);
    let line_count = text.line_count(inner.width) as u16;
    let max_scroll = line_count.saturating_sub(inner.height);
    let scroll = w.detail_scroll.min(max_scroll);
    let block = block.title_bottom(Line::from(if max_scroll > 0 {
        format!(
            " PgUp/PgDn · {}–{} of {} lines ",
            scroll + 1,
            (scroll + inner.height).min(line_count),
            line_count
        )
    } else {
        " Measured · you decide ".into()
    }));
    frame.render_widget(text.block(block).scroll((scroll, 0)), parts[0]);
    if visual_height > 0
        && let Some(item) = selected_item
    {
        folder_map::render_care_map(frame, parts[1], app, &item);
    } else if visual_height > 0 && disk {
        if let Some(v) = &w.volume {
            let ratio = (v.disk_used_kb() as f64 / v.capacity_kb.max(1) as f64).clamp(0., 1.);
            frame.render_widget(
                Gauge::default()
                    .block(panel(app, " DISK CAPACITY · not a cleanup estimate "))
                    .gauge_style(Style::default().fg(disk_usage_color(app, ratio)))
                    .ratio(ratio)
                    .label(format!(
                        "{} used · {} free",
                        format_kb(v.disk_used_kb()),
                        format_kb(v.disk_free_kb())
                    )),
                parts[1],
            );
        }
    } else if visual_height > 0 && activity {
        let data: Vec<_> = w.trend.iter().copied().collect();
        frame.render_widget(
            ratatui::widgets::Sparkline::default()
                .block(
                    panel(app, " CPU HISTORY · sampled account processes ")
                        .title_bottom(Line::from(" 100% = one core · includes scan activity ")),
                )
                .data(&data)
                .style(Style::default().fg(app.color(BLUE))),
            parts[1],
        );
    }
}

fn render_results(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let text = w
        .history
        .get(w.history_cursor)
        .map(|s| {
            let removed: u64 = s.actions.iter().map(|a| a.removed_kb).sum();
            let mut text = String::new();
            if !s.actions.is_empty() {
                text.push_str(&format!("Freed {}", format_kb(removed)));
                if let (Some(before), Some(after)) = (s.free_before_kb, s.free_after_kb) {
                    text.push_str(&format!(
                        " · free space {} → {}",
                        format_kb(before),
                        format_kb(after)
                    ));
                    if removed > 0 && after <= before {
                        text.push_str("\nmacOS can take a moment to report freed space, and data shared with snapshots or clones is only freed when every copy is gone.");
                    }
                }
                text.push_str("\n\n");
                for action in &s.actions {
                    text.push_str(&format!("{}\n{}\n\n", action.target, action.result));
                }
            } else {
                text.push_str(&format!("{}\n\n", s.state));
            }
            if let (Some(before), Some(after)) = (&s.before, &s.after) {
                text.push_str(&format!(
                    "ACTIVITY BEFORE\n{}\n\nACTIVITY AFTER\n{}\n\nObservations, not proof of a performance change.\n\n",
                    before.describe(),
                    after.describe()
                ));
            }
            if !s.investigations.is_empty() {
                text.push_str("INVESTIGATIONS\n");
                for case in &s.investigations {
                    text.push_str(&format!(
                        "{}\n{}\n\n",
                        case.question
                            .as_deref()
                            .map(|question| format!("Q: {}", ai::display_text(question)))
                            .unwrap_or_else(|| ai::display_text(&case.target)),
                        case.conclusion
                            .as_deref()
                            .map(ai::display_text)
                            .unwrap_or_else(|| "No conclusion recorded.".into())
                    ));
                }
            }
            if let Some(insight) = w
                .result_insight
                .as_ref()
                .filter(|_| w.result_summary_session == Some(s.id))
            {
                text.push_str(&format!("LOCAL AI SUMMARY\n{}", insight.summary));
            }
            text
        })
        .unwrap_or_else(|| "No saved scan selected.".into());
    text_panel(
        frame,
        area,
        app,
        " RESULTS · ↑↓ scroll ",
        text,
        w.detail_scroll,
    );
}

fn render_review(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    if w.plan.is_empty() {
        text_panel(frame, area, app, " YOUR PLAN IS EMPTY ", "Nothing is queued.\n\n1. Return to Findings with Esc.\n2. Read a finding and its tradeoff.\n3. Space adds a supported action to the plan.\n4. p opens this review before anything runs.\n\nAdding to a plan never changes files.".into(), 0);
        return;
    }
    let word = if w.plan.iter().any(|a| matches!(a, Action::ReviewClean(_))) {
        "DELETE"
    } else if w
        .plan
        .iter()
        .any(|a| matches!(a, Action::Signal(..) | Action::Move(_)))
    {
        "APPLY"
    } else {
        "CLEAN"
    };
    let parts = Layout::vertical([Constraint::Min(3), Constraint::Length(4)]).split(area);
    let reclaim: u64 = w
        .plan
        .iter()
        .map(|action| match action {
            Action::Clean(entry) | Action::ReviewClean(entry) => entry.size_kb,
            _ => 0,
        })
        .sum();
    let mut text = format!(
        "{} action(s){} · nothing has run yet",
        w.plan.len(),
        if reclaim > 0 {
            format!(" · up to {} to reclaim", format_kb(reclaim))
        } else {
            String::new()
        }
    );
    let suggested_ids: &[String] = w
        .report
        .as_ref()
        .map(|report| report.suggestions.as_slice())
        .unwrap_or_default();
    type Group = (&'static str, fn(&Action) -> bool);
    let groups: [Group; 4] = [
        ("CLEAR REBUILDABLE CONTENTS", |a| {
            matches!(a, Action::Clean(_))
        }),
        ("PERMANENTLY DELETE REVIEWED DATA", |a| {
            matches!(a, Action::ReviewClean(_))
        }),
        ("SIGNAL PROCESSES", |a| matches!(a, Action::Signal(..))),
        ("MOVE TO ANOTHER VOLUME", |a| matches!(a, Action::Move(_))),
    ];
    let mut number = 0;
    for (heading, belongs) in groups {
        let actions: Vec<&Action> = w.plan.iter().filter(|a| belongs(a)).collect();
        if actions.is_empty() {
            continue;
        }
        text.push_str(&format!("\n\n{heading}"));
        for action in actions {
            number += 1;
            let why = if suggested_ids.contains(&action.id()) {
                "\nAI suggested this from cited evidence; you added it."
            } else {
                ""
            };
            text.push_str(&format!("\n\n{number}. {}{why}", action.description()));
        }
    }
    text_panel(
        frame,
        parts[0],
        app,
        " REVIEW EXACT ACTIONS · ↑↓ scroll ",
        text,
        w.review_scroll,
    );
    let mut acknowledgements = Vec::new();
    if w.plan.iter().any(|a| matches!(a, Action::Signal(..))) {
        acknowledgements.push(if w.signals_ack {
            "✓ signals acknowledged"
        } else {
            "s acknowledge signal consequences"
        });
    }
    if w.plan.iter().any(|a| matches!(a, Action::Move(..))) {
        acknowledgements.push(if w.moves_ack {
            "✓ relocation acknowledged"
        } else {
            "m acknowledge move consequences"
        });
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format!(" Type {word} then Enter: {}▏", w.acknowledgement),
                Style::default().fg(app.color(CORAL)).bold(),
            ),
            Line::styled(
                acknowledgements.join(" · "),
                Style::default().fg(app.color(AMBER)),
            ),
            Line::styled(
                " Esc returns without changes · Delete clears the plan",
                Style::default().fg(app.color(MUTED)),
            ),
        ])
        .wrap(Wrap { trim: false }),
        parts[1],
    );
}

fn render_running(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let verifying = w.session.state.starts_with("Verifying");
    let executing = w.session.state.starts_with("Running");
    let stopped = w
        .work
        .as_ref()
        .is_some_and(|work| work.cancel.load(Ordering::Relaxed));
    let phase = if stopped {
        "STOP REQUESTED"
    } else if verifying {
        "3 / 3 · VERIFY RESULTS"
    } else if executing {
        "2 / 3 · APPLY REVIEWED ACTIONS"
    } else {
        "1 / 3 · MEASURE BEFORE CHANGES"
    };
    let parts = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(area);
    frame.render_widget(
        Gauge::default()
            .block(panel(app, " ACTIONS REPORTED · verification follows "))
            .gauge_style(Style::default().fg(app.color(BLUE)))
            .ratio(w.session.actions.len() as f64 / w.plan.len().max(1) as f64)
            .label(format!("{} / {}", w.session.actions.len(), w.plan.len())),
        parts[0],
    );
    text_panel(
        frame,
        parts[1],
        app,
        phase,
        format!(
            "{}\n\n{}\n\n{}",
            w.session.state,
            if stopped {
                "Waiting for the current action to finish safely. Already completed actions are not undone."
            } else {
                "Esc requests a safe stop. Completed actions are recorded in History."
            },
            w.session
                .actions
                .iter()
                .map(|a| format!("{}\n{}", a.target, a.result))
                .collect::<Vec<_>>()
                .join("\n\n")
        ),
        0,
    );
}

fn render_help(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let mut lines = vec![Line::styled(
        "See where the space went → understand an item → add it to a plan → review → confirm",
        Style::default().fg(app.color(MINT)).bold(),
    )];
    if let Some(note) = &w.note {
        section(&mut lines, app, "LAST MESSAGE", note.clone());
    }
    section(
        &mut lines,
        app,
        "SCREENS",
        "1 Overview   where your space went, largest first, with a plain verdict for each item\n2 Explore    browse folders by size\n3 History    past scans and what each cleanup actually freed\nTab switches screens. : lists every command and runs it.".into(),
    );
    section(
        &mut lines,
        app,
        "MOST USED",
        "↑ ↓  choose          Enter  explain the item      Esc  back\nSpace  add to plan   a  add all quick wins          p  review the plan\ni  investigate       /  ask a question              v  what the scan could not see\ne  browse folder     o  show in Finder              r  scan again\nq  quit".into(),
    );
    section(
        &mut lines,
        app,
        "SAFETY",
        "Scanning never changes anything. Nothing runs until you review exact paths and type a confirmation word. Only rebuildable caches are ever cleared routinely; app data needs its own DELETE confirmation.".into(),
    );
    section(
        &mut lines,
        app,
        "LOCAL AI",
        format!(
            "{}\nWhen available, the on-device model checks items with read-only tools and explains what it found. It can suggest, never act. M reduces motion ({}), R toggles online reference research ({}).",
            w.ai_framework.description(),
            if w.motion { "motion on" } else { "motion off" },
            if w.online_research { "on" } else { "off" }
        ),
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(app, " HELP · Esc back · ↑↓ scroll "))
            .wrap(Wrap { trim: false })
            .scroll((w.detail_scroll, 0)),
        area,
    );
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let mut commands: Vec<(&str, &str, KeyCode)> = Vec::new();
    if w.help {
        commands.extend([
            ("Esc", "back", KeyCode::Esc),
            ("↑↓", "scroll", KeyCode::Down),
        ]);
    } else if w.work.is_some() {
        commands.push(("Esc", "request stop", KeyCode::Esc));
    } else if w.palette.is_some() {
        commands.extend([
            ("Enter", "run", KeyCode::Enter),
            ("↑↓", "choose", KeyCode::Down),
            ("Esc", "close", KeyCode::Esc),
        ]);
    } else if w.asking.is_some() {
        commands.extend([
            ("Enter", "ask", KeyCode::Enter),
            ("Esc", "cancel", KeyCode::Esc),
        ]);
    } else if w.reviewing || w.clearing_history {
        commands.extend([
            ("Esc", "back", KeyCode::Esc),
            ("↑↓", "scroll", KeyCode::Down),
            ("Enter", "confirm phrase", KeyCode::Enter),
        ]);
    } else if w.awaiting_approval() {
        commands.extend([
            ("a", "approve trace", KeyCode::Char('a')),
            ("Esc", "skip", KeyCode::Esc),
        ]);
    } else if w.coverage || w.answer_open {
        commands.extend([
            ("Esc", "back", KeyCode::Esc),
            ("↑↓", "scroll", KeyCode::Down),
        ]);
    } else if w.legacy {
        commands.extend([
            ("Esc", "back", KeyCode::Esc),
            ("↑↓", "choose", KeyCode::Down),
            ("Space", "plan", KeyCode::Char(' ')),
        ]);
    } else if w.detail || w.coverage {
        commands.extend([
            ("Esc", "back", KeyCode::Esc),
            ("↑↓", "scroll", KeyCode::Down),
        ]);
        if w.screen == Screen::Overview && !w.coverage {
            commands.push(("i", "investigate", KeyCode::Char('i')));
            if !app.analysis_only
                && w.selected()
                    .is_some_and(|f| matches!(f.target, Target::Cache(_) | Target::Process(..)))
            {
                commands.push(("Space", "add to plan", KeyCode::Char(' ')));
            }
        }
    } else {
        match w.screen {
            Screen::Overview => {
                commands.push(("Enter", "explain", KeyCode::Enter));
                if !app.analysis_only
                    && w.selected()
                        .is_some_and(|f| matches!(f.target, Target::Cache(_) | Target::Process(..)))
                {
                    commands.push(("Space", "add", KeyCode::Char(' ')));
                }
                if !app.analysis_only && w.findings.iter().any(|f| f.quick_win) {
                    commands.push(("a", "quick wins", KeyCode::Char('a')));
                }
                commands.push(("p", "review", KeyCode::Char('p')));
            }
            Screen::Explore => {
                commands.extend([
                    ("Enter", "open", KeyCode::Enter),
                    ("←", "up", KeyCode::Left),
                    ("o", "Finder", KeyCode::Char('o')),
                ]);
            }
            Screen::History => commands.extend([
                ("Enter", "results", KeyCode::Enter),
                ("r", "scan again", KeyCode::Char('r')),
            ]),
        }
        commands.push((":", "all commands", KeyCode::Char(':')));
    }
    // Keep help visible at every width. Drop whole optional commands, never
    // clip a key away from its meaning. Each shown command is also clickable.
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(app.color(FAINT)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut x = inner.x;
    let reserve =
        if w.help || w.reviewing || w.work.is_some() || w.clearing_history || w.asking.is_some() {
            0
        } else {
            8
        };
    for (key, label, code) in commands {
        let width = (key.chars().count() + label.chars().count() + 3) as u16;
        if x + width > inner.right().saturating_sub(reserve) {
            continue;
        }
        let rect = Rect::new(x, inner.y, width, inner.height.min(1));
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" {key} "),
                    Style::default().fg(app.color(BLUE)).bold(),
                ),
                Span::styled(format!("{label} "), Style::default().fg(app.color(MUTED))),
            ])),
            rect,
        );
        w.hits.borrow_mut().push((rect, Control::Key(code)));
        x += width;
    }
    if reserve > 0 {
        let rect = Rect::new(
            inner.right().saturating_sub(8),
            inner.y,
            8,
            inner.height.min(1),
        );
        frame.render_widget(
            Paragraph::new(" ? help").style(Style::default().fg(app.color(BLUE))),
            rect,
        );
        w.hits
            .borrow_mut()
            .push((rect, Control::Key(KeyCode::Char('?'))));
    }
}
