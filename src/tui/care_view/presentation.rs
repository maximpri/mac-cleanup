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
    let show_ask = !w.help
        && !w.legacy
        && w.work.is_none()
        && !w.clearing_history
        && !w.reviewing
        && !w.awaiting_approval();
    let regions = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(if show_ask { 4 } else { 0 }),
        Constraint::Length(notice_height),
        Constraint::Length(2),
    ])
    .split(area);
    render_header(frame, regions[0], app, w);
    // Header shortcuts must not type letters into a confirmation or palette.
    // Those views expose their own controls in the footer.
    if w.help
        || w.legacy
        || w.work.is_some()
        || w.reviewing
        || w.clearing_history
        || w.palette.is_some()
        || w.awaiting_approval()
    {
        w.hits.borrow_mut().clear();
    }
    render_status(frame, regions[1], app, w);
    let panes = Layout::horizontal([Constraint::Percentage(44), Constraint::Percentage(56)])
        .spacing(1)
        .split(regions[2]);
    if w.legacy && app.phase == Phase::Processes {
        render_process_table(frame, panes[0], app);
    } else {
        render_list(frame, panes[0], app, w);
    }
    let body = panes[1];
    if w.help {
        render_help(frame, body, app, w);
    } else if w.legacy {
        match app.phase {
            Phase::Processes => render_process_details(frame, body, app),
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
        w.detail_max_scroll.set(text_panel(
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
        ));
    } else if w.ai_status_open {
        render_ai_status(frame, body, app, w);
    } else if w.answer_open {
        render_answer(frame, body, app, w);
    } else {
        render_evidence(frame, body, app, w);
    }
    if show_ask {
        render_ask(frame, regions[3], app, w);
    }
    // Broad pane targets come after their controls so rows and buttons win.
    w.hits.borrow_mut().extend([
        (panes[0], Control::Pane(false)),
        (panes[1], Control::Pane(true)),
    ]);
    if notice_height > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" › ", Style::default().fg(app.color(BLUE)).bold()),
                Span::raw(ai::display_text(w.note.as_deref().unwrap_or_default())),
            ]))
            .style(Style::default().fg(app.color(AMBER)))
            .wrap(Wrap { trim: false }),
            regions[4],
        );
    }
    render_footer(frame, regions[5], app, w);
    if w.palette.is_some() {
        w.hits.borrow_mut().retain(|(_, control)| {
            matches!(
                control,
                Control::Key(KeyCode::Enter | KeyCode::Esc | KeyCode::Up | KeyCode::Down)
            )
        });
        app.hit_regions.borrow_mut().clear();
        app.map_paths.borrow_mut().clear();
        render_palette(frame, regions[2], app, w);
    }
}

/// A shared composer below both panes; the question can concern the whole Mac.
fn render_ask(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let active = w.asking.is_some();
    let color = app.color(if active { BLUE } else { FAINT });
    let block = Block::bordered()
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(app.color(SURFACE)))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let draft = w.asking.as_deref().unwrap_or(&w.ask_draft);
    let placeholder = draft.is_empty();
    let input = if placeholder {
        "Ask about this Mac…"
    } else {
        draft
    };
    let prefix = "Ask AI › ";
    let room = inner.width.saturating_sub(prefix.chars().count() as u16) as usize;
    let text = if active {
        format!("{}▏", truncate_middle(input, room.saturating_sub(1)))
    } else {
        truncate_end(input, room)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prefix, Style::default().fg(app.color(BLUE)).bold()),
            Span::styled(
                text,
                Style::default().fg(app.color(if placeholder { MUTED } else { INK })),
            ),
        ])),
        inner,
    );
    let (status, status_color) = match &w.ai_framework {
        ai::FrameworkStatus::Detecting => ("Checking Apple Intelligence…".into(), MUTED),
        ai::FrameworkStatus::Available { .. } => ("Apple Intelligence · on-device".into(), MUTED),
        ai::FrameworkStatus::Missing { .. } => (
            "AI helper missing · install the release bundle".into(),
            AMBER,
        ),
        ai::FrameworkStatus::Unavailable { detail, .. } => (
            format!(
                "English (US) required for now · {}",
                ai::display_text(detail)
            ),
            AMBER,
        ),
    };
    let settings = matches!(w.ai_framework, ai::FrameworkStatus::Unavailable { .. });
    let hint = if settings {
        if inner.width >= 86 && w.ai_framework_work.is_some() {
            "Checking… · F3 AI · F2 Settings"
        } else if inner.width >= 86 && active {
            "Enter retry · F3 AI · F2 Settings"
        } else {
            "F3 AI · F2 Settings"
        }
    } else if w.ai_framework_work.is_some() {
        "F3 AI"
    } else if !w.model_ready() {
        if active {
            "Enter retry · F3 AI"
        } else {
            "/ retry · F3 AI"
        }
    } else if active {
        "Enter ask · F3 AI"
    } else {
        "/ type · F3 AI"
    };
    if inner.height >= 2 {
        // Keep the recovery shortcut visible even when the reason is long.
        let row = Rect::new(inner.x, inner.y + 1, inner.width, 1);
        let parts = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(hint.chars().count() as u16),
        ])
        .spacing(2)
        .split(row);
        frame.render_widget(
            Paragraph::new(truncate_end(&status, parts[0].width as usize))
                .style(Style::default().fg(app.color(status_color))),
            parts[0],
        );
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(app.color(MUTED))),
            parts[1],
        );
        if let Some(offset) = hint.find("F3 AI") {
            let x = parts[1].x + hint[..offset].chars().count() as u16;
            w.hits
                .borrow_mut()
                .push((Rect::new(x, row.y, 5, 1), Control::Key(KeyCode::F(3))));
        }
        if settings {
            let mut target = parts[1];
            target.x = target.right().saturating_sub(11);
            target.width = 11;
            w.hits
                .borrow_mut()
                .push((target, Control::Key(KeyCode::F(2))));
        }
    }
    w.hits.borrow_mut().push((area, Control::Ask));
}

fn render_ai_status(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let mut text = format!(
        "CURRENT AI REQUIREMENT\nFor now, set both Mac and Siri to English (United States), en-US.\nApple Intelligence must be enabled and its model setup complete.\n\nApple on-device model · Automatic\n\n{}\n",
        w.ai_framework.description(),
    );
    if let Some(diagnostics) = w.ai_framework.diagnostics() {
        text.push_str(&format!(
            "\nLANGUAGE SUPPORT\nMac: {} · {}\nSiri: {}\n",
            ai::display_text(&diagnostics.device_language),
            if diagnostics.locale_supported {
                "supported by this model"
            } else {
                "not supported by this model"
            },
            diagnostics
                .siri_language
                .as_deref()
                .map(ai::display_text)
                .unwrap_or_else(|| "not reported".into()),
        ));
        if !w.model_ready() {
            if diagnostics.locale_supported
                && diagnostics.siri_language.as_deref()
                    == Some(diagnostics.device_language.as_str())
            {
                text.push_str("\nYour detected languages match and Apple's framework lists them as supported. Diskray's current setup requirement remains English (United States) for both Mac and Siri. Language support alone does not make the model ready.\n\nIF MODEL SETUP STAYS UNAVAILABLE\n1. F2 opens Settings: use English (United States) for both Mac and Siri, then allow Apple's model setup to finish.\n2. If setup remains stuck, save your work and restart the Mac, then check again.\n3. If it still fails, check macOS updates or contact Apple Support.\n\nThe framework cannot tell Diskray whether a download is progressing or a system service has failed. Diskray cannot install or repair Apple's model assets. Automatic rechecks detect recovery; they do not repair macOS.\n");
            } else {
                text.push_str("\nF2 opens Settings. For now, set both Mac and Siri to English (United States), then allow Apple's model setup to finish.\n");
            }
        }
        if diagnostics.context_size > 0 {
            text.push_str(&format!(
                "\nModel context: {} tokens\n",
                diagnostics.context_size
            ));
        } else {
            text.push_str("\nModel context: not reported while unavailable\n");
        }
    }
    text.push_str("\nMODEL CHOICE\nAutomatic: macOS selects the Apple model for this Mac. No alternative general-purpose on-device model is exposed to Diskray.\n\nAll inference stays on this Mac. Your system languages stay as you set them.\n\nDiskray checks again every 30 seconds while unavailable. r checks now; your Ask draft is preserved.");
    if let Some(diagnostics) = w.ai_framework.diagnostics() {
        text.push_str(&format!(
            "\n\nLanguages reported by Apple's framework (Diskray currently requires English, US):\n{}",
            diagnostics
                .supported_languages
                .iter()
                .map(|tag| ai::display_text(tag))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    w.detail_max_scroll.set(text_panel(
        frame,
        area,
        app,
        " LOCAL AI · auto-detected ",
        text,
        w.detail_scroll,
    ));
}

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let plan = format!(" p Plan ({}) ", w.plan.len());
    let regions = Layout::horizontal([
        Constraint::Length(10),
        Constraint::Min(0),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(plan.chars().count() as u16),
    ])
    .split(area);
    button(
        frame,
        regions[0],
        app,
        w,
        " DISKRAY".into(),
        Control::Key(KeyCode::Char('g')),
        false,
    );
    if let Some(volume) = &w.volume {
        let ratio = volume.disk_used_kb() as f64 / volume.capacity_kb.max(1) as f64;
        frame.render_widget(
            Paragraph::new(format!("{} free", format_kb(volume.disk_free_kb())))
                .style(Style::default().fg(disk_usage_color(app, ratio))),
            regions[1],
        );
    }
    button(
        frame,
        regions[2],
        app,
        w,
        " / Ask ".into(),
        Control::Ask,
        w.asking.is_some() || w.answer_open,
    );
    button(
        frame,
        regions[3],
        app,
        w,
        " h History ".into(),
        Control::Key(KeyCode::Char('h')),
        false,
    );
    button(
        frame,
        regions[4],
        app,
        w,
        plan,
        Control::Key(KeyCode::Char('p')),
        w.reviewing || !w.plan.is_empty(),
    );
}

fn list_panel<'a>(app: &App, w: &Workspace, title: impl Into<Line<'a>>) -> Block<'a> {
    let active = !w.detail
        && !w.ai_status_open
        && !w.help
        && !w.reviewing
        && w.asking.is_none()
        && !w.clearing_history
        && w.work.is_none();
    panel(app, title).border_style(Style::default().fg(app.color(if active {
        BLUE
    } else {
        FAINT
    })))
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
    context.push(if w.ai_framework_work.is_some() {
        "AI checking".to_string()
    } else {
        w.ai_framework.compact().to_string()
    });
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
            "\n\nAI SUGGESTS: {}. Space adds it; p reviews the plan.",
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
                "LOCAL AI · MEASURED FINDINGS"
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
    w.detail_max_scroll.set(text_panel(
        frame,
        area,
        app,
        " ANSWER · Esc back · ↑↓ scroll ",
        text,
        w.detail_scroll,
    ));
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
        (Target::Folder(p), Action::Trash(plan)) => p == &plan.path,
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
                    MUTED,
                )
            }
        }
        Target::Process(..) => {
            let state = f.observation.split(" · ").next().unwrap_or("Running");
            let mut words = state.to_lowercase();
            if let Some(first) = words.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            (format!("{words} app"), MUTED)
        }
        Target::System => ("Needs attention".into(), CORAL),
    }
}

/// What the user can do next, in plain words.
fn next_step(app: &App, w: &Workspace, f: &Finding) -> String {
    if queued(w, f) {
        return if matches!(f.target, Target::Folder(_)) {
            "Queued for Trash. t takes it out; p reviews the plan. No space is freed until Trash is emptied."
        } else {
            "In your plan. Space takes it out; p reviews the plan."
        }.into();
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
            "Enter / → opens child folders and sizes · t reviews moving this item to Trash · o shows it in Finder.".into()
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
fn capacity_bar(app: &App, o: &crate::why::Overview, width: usize) -> Line<'static> {
    let capacity = o.capacity_kb.max(1);
    let system = o.other_volumes_kb.min(o.used_kb);
    let data = o.used_kb - system;
    let (measured, unseen) = if o.folder_walk {
        let unseen = data.saturating_sub(o.measured_kb);
        (data - unseen, unseen)
    } else {
        (0, data)
    };
    // Cumulative boundaries prevent independently rounded segments overrunning
    // the available width or hiding a small nonzero accounting component.
    let cells = |kb: u64| {
        ((u128::from(kb) * width as u128) / u128::from(capacity)).min(width as u128) as usize
    };
    let a = cells(measured);
    let b = cells(measured + unseen).saturating_sub(a);
    let c = cells(measured + unseen + system).saturating_sub(a + b);
    let free_cells = width.saturating_sub(a + b + c);
    Line::from(vec![
        Span::styled("█".repeat(a), Style::default().fg(app.color(BLUE))),
        Span::styled("▓".repeat(b), Style::default().fg(app.color(AMBER))),
        Span::styled("▒".repeat(c), Style::default().fg(app.color(MUTED))),
        Span::styled("░".repeat(free_cells), Style::default().fg(app.color(MINT))),
    ])
}

fn balance_line(app: &App, label: &str, kb: i128, width: usize, color: Color) -> Line<'static> {
    let amount = format!(
        "{}{}",
        if kb < 0 { "−" } else { "" },
        format_kb(kb.unsigned_abs().min(u64::MAX as u128) as u64)
    );
    let label_width = width.saturating_sub(amount.chars().count() + 1);
    Line::styled(
        format!(
            "{:<label_width$} {amount}",
            truncate_end(label, label_width)
        ),
        Style::default().fg(app.color(color)),
    )
}

fn balance_footer(
    app: &App,
    w: &Workspace,
    o: &crate::why::Overview,
    width: usize,
    expanded: bool,
) -> Vec<Line<'static>> {
    let (count, listed) = w.listed_storage();
    let mut lines = if expanded {
        o.used_balance(listed)
            .into_iter()
            .map(|(label, kb)| {
                let label = if label == "Listed areas" {
                    format!("Listed areas ({count})")
                } else {
                    label.to_string()
                };
                let color = if label == "Unaccounted usage" || kb < 0 {
                    AMBER
                } else {
                    MUTED
                };
                balance_line(app, &label, kb, width, color)
            })
            .collect::<Vec<_>>()
    } else {
        vec![
            balance_line(
                app,
                &format!("Listed ({count})"),
                i128::from(listed),
                width,
                MUTED,
            ),
            balance_line(
                app,
                "Rest of used",
                i128::from(o.used_kb) - i128::from(listed),
                width,
                MUTED,
            ),
        ]
    };
    if expanded {
        lines.insert(
            0,
            Line::styled(
                "SPACE BALANCE · v exact totals",
                Style::default().fg(app.color(BLUE)).bold(),
            ),
        );
    }
    lines.push(balance_line(
        app,
        "Total used",
        i128::from(o.used_kb),
        width,
        INK,
    ));
    if expanded {
        lines.push(balance_line(
            app,
            "Free",
            i128::from(o.free_kb),
            width,
            MINT,
        ));
        let gap = i128::from(o.capacity_kb) - i128::from(o.used_kb) - i128::from(o.free_kb);
        if gap != 0 {
            lines.push(balance_line(app, "Capacity difference", gap, width, AMBER));
        }
        lines.push(balance_line(
            app,
            "Disk capacity",
            i128::from(o.capacity_kb),
            width,
            INK,
        ));
    }
    lines
}

fn balance_details(app: &App, w: &Workspace) -> Vec<Line<'static>> {
    let o = w.overview(app);
    let (count, listed) = w.listed_storage();
    let mut lines = vec![
        Line::styled(
            "STORAGE BALANCE · exact KiB",
            Style::default().fg(app.color(BLUE)).bold(),
        ),
        Line::raw(format!(
            "{count} listed areas, including rows off screen. Hidden and smaller areas remain in Other measured files."
        )),
        Line::raw(""),
    ];
    let mut rows = o.used_balance(listed);
    rows.extend([
        ("Total used", i128::from(o.used_kb)),
        ("Free", i128::from(o.free_kb)),
    ]);
    let gap = i128::from(o.capacity_kb) - i128::from(o.used_kb) - i128::from(o.free_kb);
    if gap != 0 {
        rows.push(("Capacity difference", gap));
    }
    rows.push(("Disk capacity", i128::from(o.capacity_kb)));
    for (label, amount) in rows {
        lines.push(Line::raw(format!("{label}: {amount} KiB")));
    }
    lines.extend([
        Line::raw(""),
        Line::raw("Listed areas + other measured files + unaccounted usage + other APFS volumes − measurement excess = total used. Displayed GiB values are rounded; the KiB values above reconcile exactly."),
        Line::raw("Directory measurements come from one walk on the accounted volume. Cleanup estimates are not added again. macOS-managed files are included in measured files; other APFS volumes are separate."),
        Line::raw("Unaccounted usage has no proven cause. Unreadable files, snapshots, filesystem metadata, or changes during scanning may contribute. It is not a cleanup estimate."),
    ]);
    if !o.whole_volume {
        lines.push(Line::raw("This is a folder scan: Outside this scan includes the rest of the disk, not unexplained usage."));
    }
    if o.measurement_excess_kb > 0 {
        lines.push(Line::styled("Measured file blocks exceed reported usage. Shared APFS blocks or changes during scanning may contribute; this is a discrepancy, not extra reclaimable space.", Style::default().fg(app.color(AMBER))));
    }
    lines.push(Line::raw(""));
    lines
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
        w.hits.borrow_mut().push((
            Rect::new(
                inner.x,
                inner.y + 1 + (index - start) as u16,
                inner.width,
                1,
            ),
            Control::PaletteRow(index),
        ));
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
    let title = if area.width < 40 {
        " STORAGE ".to_string()
    } else if w.volume.is_some() {
        format!(" WHERE YOUR {} WENT ", format_kb(o.used_kb))
    } else {
        " WHERE THE SPACE WENT ".to_string()
    };
    let position = if visible.is_empty() { 0 } else { w.cursor + 1 };
    let filter = if w.show_all { "fewer" } else { "all" };
    let footer_title = if area.width < 40 {
        format!(" {position}/{} f {filter} v totals ", visible.len())
    } else {
        format!(" {position} of {} · f {filter} · v totals ", visible.len())
    };
    let block = list_panel(app, w, title).title_bottom(Line::from(footer_title));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let muted = Style::default().fg(app.color(MUTED));

    let mut header: Vec<Line<'static>> = Vec::new();
    if w.volume.is_some() {
        header.push(capacity_bar(app, &o, width.min(72)));
        header.push(Line::styled(
            truncate_end(
                if w.accounted_rows.is_some() && width < 35 {
                    "v explains disk usage"
                } else if w.accounted_rows.is_some() && o.whole_volume {
                    "█ files  ▓ unaccounted  ▒ APFS  ░ free"
                } else if w.accounted_rows.is_some() {
                    "█ this scan  ▓ rest of disk  ░ free"
                } else {
                    "Measuring · cleanup estimates below"
                },
                width,
            ),
            muted,
        ));
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
                    "✓ Quick wins in plan".to_string()
                } else if width < 40 {
                    format!("Quick wins: {}", format_kb(total))
                } else {
                    format!(
                        "Quick wins: {} · {} item(s){}",
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

    if w.accounted_rows.is_some() {
        let expanded = inner.height >= 24 && width >= 35;
        let balance = balance_footer(app, w, &o, width, expanded);
        // On a short terminal, reserve the accounting rows before the optional
        // bar legend and spacing. The selected item must remain reachable too.
        let minimum_row = if inner.height < 6 { 1 } else { 2 };
        header.truncate((inner.height as usize).saturating_sub(balance.len() + minimum_row));
        // Preserve space for at least two list rows; accounting takes priority
        // over secondary status notes already available in details and help.
        let room = (inner.height as usize).saturating_sub(header.len() + balance.len() + 4);
        footer.truncate(room.min(2));
        footer.extend(balance);
    }
    let rows_height = (inner.height as usize).saturating_sub(header.len() + footer.len());
    let row_height = if rows_height < 2 { 1 } else { 2 };
    let mut lines = header.clone();
    let rows_top = inner.y + header.len() as u16;
    if visible.is_empty() {
        lines.push(Line::styled(
            if w.complete {
                if w.kept.is_empty() {
                    "Nothing large enough."
                } else {
                    "All items hidden."
                }
            } else {
                "Measuring folders…"
            },
            muted,
        ));
        if rows_height >= 2 {
            lines.push(Line::styled("f shows all items.", muted));
        }
    }
    let capacity = (rows_height / row_height).max(1);
    let start = w.cursor.saturating_sub(capacity - 1);
    for (row, (index, f)) in visible
        .iter()
        .enumerate()
        .skip(start)
        .take(capacity)
        .enumerate()
    {
        let (verdict_text, color) = verdict(app, w, f);
        let size = if f.size_kb > 0 {
            format_kb(f.size_kb)
        } else {
            String::new()
        };
        let name_width = width.saturating_sub(2 + 9);
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
            Span::styled(format!("{size:>9}"), base),
        ]));
        if row_height == 2 {
            lines.push(Line::styled(
                format!("  {}", truncate_end(&verdict_text, width.saturating_sub(2))),
                base.fg(app.color(color)),
            ));
        }
        w.hits.borrow_mut().push((
            Rect::new(
                inner.x,
                rows_top + (row * row_height) as u16,
                inner.width,
                row_height as u16,
            ),
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
    let block = list_panel(app, w, " FOLDERS ").title_bottom(Line::from(" ← parent · g storage "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(truncate_middle(&path, inner.width as usize))
            .style(Style::default().fg(app.color(BLUE))),
        Rect::new(inner.x, inner.y, inner.width, inner.height.min(1)),
    );
    let items = app.explorer_items();
    let total: u64 = items.iter().map(|i| i.size_kb).sum();
    let complete = app
        .explorer_path
        .as_ref()
        .and_then(|path| w.measured_folders.get(path))
        .copied()
        .unwrap_or_else(|| app.inventory.as_ref().is_some_and(|i| i.complete));
    let measuring = w.measure.as_ref().filter(|work| {
        app.explorer_path
            .as_ref()
            .is_some_and(|path| path.starts_with(&work.path))
    });
    let status = if let Some(work) = measuring {
        format!(
            "Measuring · {} items · {} so far",
            work.progress.items,
            format_kb(work.progress.size_kb)
        )
    } else {
        format!(
            "{} items · {}{}",
            items.len(),
            format_kb(total),
            if complete { "" } else { " observed · partial" }
        )
    };
    frame.render_widget(
        Paragraph::new(truncate_middle(&status, inner.width as usize))
            .style(Style::default().fg(app.color(MUTED))),
        Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(1).min(1),
        ),
    );
    let rows = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(2),
    );
    if items.is_empty() {
        let text = if !w.complete || measuring.is_some() {
            "Measuring folder contents…\nSizes update when this walk finishes. You can keep browsing."
        } else if complete {
            "This folder is empty.\n← returns to the parent. i measures it again."
        } else {
            "No readable children measured here.\n← returns to the parent. i retries this folder; v shows coverage."
        };
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(app.color(MUTED))),
            rows,
        );
        return;
    }
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
        let queued = w.plan.iter().any(|a| a.path() == Some(item.path.as_path()));
        let action = if queued {
            "IN PLAN"
        } else if let Some(entry) = app.entries.iter().find(|e| e.spec.path == item.path) {
            match entry.status {
                CacheStatus::Ready | CacheStatus::Optional => "Space: cleanup",
                CacheStatus::Review => "Space: review data",
                _ => entry.status.label(),
            }
        } else {
            item.category.verdict()
        };
        let percent = if total == 0 {
            0
        } else {
            (item.size_kb as f64 * 100.0 / total as f64).round() as u64
        };
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
                    "  {} {:>3}% · {}",
                    meter(
                        item.size_kb as f64 / total.max(1) as f64,
                        if rect.width < 38 { 3 } else { 8 }
                    ),
                    percent,
                    action
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
    let block =
        list_panel(app, w, " SAVED SCANS ").title_bottom(Line::from(" Esc storage · r recheck "));
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
        let mut lines = if app.inventory.is_none() {
            vec![
                Line::raw("Coverage unavailable."),
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
        if w.accounted_rows.is_some() {
            let mut balance = balance_details(app, w);
            balance.append(&mut lines);
            lines = balance;
        }
        scrollable_lines(
            frame,
            area,
            app,
            w,
            " TOTALS & COVERAGE · Esc back · PgUp/PgDn ",
            lines,
        );
        return;
    }
    if w.screen == Screen::History {
        render_results(frame, area, app, w);
        return;
    }
    let macos_group = w.screen == Screen::Overview
        && w.selected().is_some_and(|f| f.id == care::MACOS_FINDING_ID);
    let selected_item = if macos_group {
        w.selected().map(|finding| StorageItem {
            path: PathBuf::from("/"),
            size_kb: finding.size_kb,
            kind: StorageItemKind::Directory,
            category: crate::storage::StorageCategory::SystemData,
        })
    } else if w.screen == Screen::Explore {
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
                        "Enter / → opens it · ← goes up · o shows it in Finder.\n{}",
                        item.category.advice()
                    )
                } else {
                    format!("o shows it in Finder.\n{}", item.category.advice())
                },
            );
            let cleanup = if app.analysis_only {
                "This session is read-only.".into()
            } else if w.plan.iter().any(|a| a.path() == Some(item.path.as_path())) {
                "In your plan. p reviews the exact action. Use Space to remove a cleanup action or t to remove a Trash move. Nothing has run yet.".into()
            } else if let Some(entry) = app.entries.iter().find(|e| e.spec.path == item.path) {
                format!(
                    "Space adds this exact cleanup rule: {} · {}.\n{}\np reviews your plan before anything runs.",
                    entry.spec.label,
                    entry.status.label(),
                    entry.spec.note
                )
            } else {
                "No automatic cleanup rule. If you no longer need this personal item, t queues the entire item for Trash; p reviews it. Trash does not free space until emptied. Protected app/system locations stay unavailable.".into()
            };
            section(&mut lines, app, "CLEANUP", cleanup);
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
                "LOCAL AI · MEASURED FINDINGS",
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
    // Keep the selected area's contents visible in both lists. Even a compact
    // terminal gets a small map; explanations remain independently scrollable.
    let filesystem_selection = selected_item.is_some()
        || w.selected().is_some_and(|f| {
            w.screen == Screen::Overview && matches!(f.target, Target::Cache(_) | Target::Folder(_))
        });
    let map_children = if macos_group {
        Some(w.macos_items.as_slice())
    } else {
        selected_item.as_ref().and_then(|item| {
            app.inventory
                .as_ref()?
                .children
                .get(&item.path)
                .map(Vec::as_slice)
        })
    };
    let visual_height = if filesystem_selection && area.height >= 7 {
        if map_children.is_some_and(|children| children.iter().any(|child| child.size_kb > 0)) {
            (area.height * 2 / 5).clamp(3, 16)
        } else {
            area.height.saturating_sub(4).min(5)
        }
    } else {
        0
    };
    let parts =
        Layout::vertical([Constraint::Min(3), Constraint::Length(visual_height)]).split(area);
    let text = Paragraph::new(lines).wrap(Wrap { trim: false });
    let block = panel(
        app,
        if w.detail {
            " DETAILS · Tab to list "
        } else {
            " DETAILS · Tab to focus "
        },
    );
    let block = block.border_style(Style::default().fg(app.color(
        if w.detail && w.asking.is_none() {
            BLUE
        } else {
            FAINT
        },
    )));
    let inner = block.inner(parts[0]);
    let line_count = text.line_count(inner.width) as u16;
    let max_scroll = line_count.saturating_sub(inner.height);
    w.detail_max_scroll.set(max_scroll);
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
        folder_map::render_care_map(frame, parts[1], app, &item, map_children);
    } else if visual_height > 0 {
        text_panel(
            frame,
            parts[1],
            app,
            " HEATMAP ",
            "Contents have not been measured yet. The map fills in as the scan progresses.".into(),
            0,
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
    w.detail_max_scroll.set(text_panel(
        frame,
        area,
        app,
        " RESULTS · ↑↓ scroll ",
        text,
        w.detail_scroll,
    ));
}

fn render_review(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    if w.plan.is_empty() {
        text_panel(frame, area, app, " YOUR PLAN IS EMPTY ", "Nothing is queued.\n\n1. Return to Findings with Esc.\n2. Read a finding and its tradeoff.\n3. Space adds a supported action to the plan.\n4. p opens this review before anything runs.\n\nAdding to a plan never changes files.".into(), 0);
        return;
    }
    let word = if w.plan.iter().any(|a| matches!(a, Action::Trash(_))) {
        "TRASH"
    } else if w.plan.iter().any(|a| matches!(a, Action::ReviewClean(_))) {
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
        "{} action(s){}",
        w.plan.len(),
        if reclaim > 0 {
            format!(" · {} max", format_kb(reclaim))
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
    let groups: [Group; 5] = [
        ("MOVE SELECTED ITEMS TO TRASH", |a| {
            matches!(a, Action::Trash(_))
        }),
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
        if w.plan.len() > 1 {
            text.push_str(&format!("\n\n{heading}"));
        }
        for action in actions {
            number += 1;
            let why = if suggested_ids.contains(&action.id()) {
                "\nAI suggested this from cited evidence; you added it."
            } else {
                ""
            };
            if w.plan.len() == 1 {
                text.push_str(&format!("\n\n{}{why}", action.description()));
            } else {
                text.push_str(&format!("\n{number}. {}{why}", action.description()));
            }
        }
    }
    w.review_max_scroll.set(text_panel(
        frame,
        parts[0],
        app,
        " REVIEW EXACT ACTIONS · ↑↓ scroll ",
        text,
        w.review_scroll,
    ));
    let mut acknowledgements = Vec::new();
    if w.plan.iter().any(|a| matches!(a, Action::Signal(..))) {
        acknowledgements.push(if w.signals_ack {
            "s [✓] Signals"
        } else {
            "s [ ] Signals"
        });
    }
    if w.plan.iter().any(|a| matches!(a, Action::Move(..))) {
        acknowledgements.push(if w.moves_ack {
            "m [✓] Move"
        } else {
            "m [ ] Move"
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
                " Esc back · Delete clears plan",
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
        "TWO PANELS",
        "The left panel lists storage items, folders, or saved scans. The right panel explains your selection and holds investigations and plan review. Ask AI spans the width below both panels: / or a click focuses input; Esc returns to browsing and keeps your draft. F3 shows the detected model and any AI blocker.\nTab switches focus. ↑↓ selects on the left and scrolls on the right. Enter / → opens a folder, including from details. ← / Backspace goes to the containing folder. Esc closes details, then goes back to the previous list. Home / End and PgUp / PgDn move within the focused panel.\ng storage findings · b browse folders · h saved scans\n: lists every command.".into(),
    );
    section(
        &mut lines,
        app,
        "MOST USED",
        "↑ ↓  choose          Enter  open folder      Esc  back\nSpace  add to plan   a  add all quick wins          p  review the plan\ni  investigate       /  ask a question              v  what the scan could not see\nt  queue for Trash   o  show in Finder              r  scan again\nq  quit".into(),
    );
    section(
        &mut lines,
        app,
        "SAFETY",
        "Scanning never changes anything. Nothing runs until you review exact paths and type a confirmation word. Only rebuildable caches are cleared routinely; app data needs its own DELETE confirmation. User-selected files and folders can be moved to Trash in a separate TRASH plan. This frees no space until Trash is emptied.".into(),
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
    scrollable_lines(frame, area, app, w, " HELP · Esc back · ↑↓ scroll ", lines);
}

fn scrollable_lines(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    w: &Workspace,
    title: &str,
    lines: Vec<Line<'_>>,
) {
    let block = panel(app, title);
    let inner = block.inner(area);
    let text = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max_scroll =
        (text.line_count(inner.width).min(u16::MAX as usize) as u16).saturating_sub(inner.height);
    w.detail_max_scroll.set(max_scroll);
    frame.render_widget(
        text.block(block)
            .scroll((w.detail_scroll.min(max_scroll), 0)),
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
    } else if w.ai_status_open {
        commands.extend([
            ("Esc", "back", KeyCode::Esc),
            ("r", "check AI", KeyCode::Char('r')),
            ("F2", "Settings", KeyCode::F(2)),
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
            (
                "Enter",
                if w.model_ready() { "ask" } else { "retry AI" },
                KeyCode::Enter,
            ),
            ("Esc", "browse", KeyCode::Esc),
        ]);
        if !w.model_ready() {
            commands.push(("F2", "Settings", KeyCode::F(2)));
        }
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
            ("Tab", "list", KeyCode::Tab),
            ("↑↓", "scroll", KeyCode::Down),
        ]);
        if w.screen == Screen::Overview && !w.coverage {
            commands.push(("Enter", "browse", KeyCode::Enter));
            commands.push(("i", "investigate", KeyCode::Char('i')));
            if !app.analysis_only
                && w.selected()
                    .is_some_and(|f| matches!(f.target, Target::Cache(_) | Target::Process(..)))
            {
                commands.push(("Space", "add to plan", KeyCode::Char(' ')));
            }
        }
        if w.screen == Screen::Explore {
            commands.push(("Enter", "open", KeyCode::Enter));
            if !app.analysis_only {
                commands.push(("Space", "cleanup", KeyCode::Char(' ')));
                commands.push(("t", "Trash", KeyCode::Char('t')));
            }
        }
    } else {
        match w.screen {
            Screen::Overview => {
                commands.push(("Enter", "browse", KeyCode::Enter));
                commands.push(("Tab", "details", KeyCode::Tab));
                if !app.analysis_only
                    && w.selected()
                        .is_some_and(|f| matches!(f.target, Target::Cache(_) | Target::Process(..)))
                {
                    commands.push(("Space", "add", KeyCode::Char(' ')));
                }
                if !app.analysis_only && w.findings.iter().any(|f| f.quick_win) {
                    commands.push(("a", "quick wins", KeyCode::Char('a')));
                }
                commands.push(("e", "browse", KeyCode::Char('e')));
                commands.push(("i", "investigate", KeyCode::Char('i')));
            }
            Screen::Explore => {
                commands.extend([
                    ("Enter", "open", KeyCode::Enter),
                    ("←", "up", KeyCode::Left),
                ]);
                if !app.analysis_only {
                    commands.push(("Space", "cleanup", KeyCode::Char(' ')));
                    commands.push(("t", "Trash", KeyCode::Char('t')));
                }
                commands.push(("i", "measure", KeyCode::Char('i')));
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
