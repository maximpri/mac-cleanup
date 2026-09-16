use super::*;

pub(super) fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(app.color(MUTED)));
    match app.phase {
        Phase::Location => {}
        Phase::Scanning => {
            let total = app.specs.len().max(1);
            let completed = app.scan_index.min(total);
            let progress = if app.retention_worker.is_some() {
                format!(
                    "{}  preparing safety checks • temporary data • {}",
                    scan_spinner(app.scan_started_at.elapsed()),
                    format_elapsed(app.scan_started_at.elapsed())
                )
            } else if let Some(task) = &app.scan_task {
                format!(
                    "{}  {completed}/{total} • {} • {} items • {} • {}",
                    scan_spinner(task.started_at.elapsed()),
                    task.spec.label,
                    task.inspected_items,
                    format_kb(task.size_kb()),
                    format_elapsed(app.scan_started_at.elapsed()),
                )
            } else if app.inventory_worker.is_some() {
                let elapsed = app
                    .inventory_started_at
                    .map_or(Duration::ZERO, |started_at| started_at.elapsed());
                format!(
                    "{}  {completed}/{total} • full-volume inventory • walking disk • {}",
                    scan_spinner(elapsed),
                    format_elapsed(app.scan_started_at.elapsed())
                )
            } else {
                let activity = if app.scan_work_complete {
                    "finishing scan"
                } else {
                    "preparing next location"
                };
                format!(
                    "{}  {completed}/{total} • {activity} • {}",
                    scan_spinner(app.scan_started_at.elapsed()),
                    format_elapsed(app.scan_started_at.elapsed())
                )
            };
            frame.render_widget(
                Gauge::default()
                    .block(block.title(if app.inventory_worker.is_some() {
                        " SCANNING • building full disk inventory • Esc cancel • q quit "
                    } else {
                        " SCANNING • live progress • Esc cancel • q quit "
                    }))
                    .gauge_style(
                        Style::default()
                            .fg(app.color(BLUE))
                            .add_modifier(Modifier::BOLD),
                    )
                    .ratio(app.scan_progress_ratio)
                    .label(progress),
                area,
            );
        }
        Phase::Review => {
            let commands = if app.analysis_only {
                vec![
                    ("↑↓", "move"),
                    ("↵", "details"),
                    ("o", "finder"),
                    ("r", "rescan"),
                    ("q", "quit"),
                ]
            } else if app.current_is_review_data() {
                vec![
                    ("d", "advanced delete"),
                    ("c", "clean all safe"),
                    ("o", "reveal in Finder"),
                    ("?", "commands"),
                ]
            } else if app.current_is_optional() {
                vec![
                    ("d", "opt in"),
                    ("c", "clean safe"),
                    ("↵", "details"),
                    ("o", "finder"),
                    ("q", "quit"),
                ]
            } else if app.is_selectable(app.cursor) && app.mode == Mode::Clean {
                vec![
                    ("↑↓", "move"),
                    ("space", "select"),
                    ("a", "all safe"),
                    ("↵", "continue"),
                    ("q", "quit"),
                ]
            } else if app.is_selectable(app.cursor) {
                vec![
                    ("space", "select"),
                    ("d", "delete one"),
                    ("c", "clean safe"),
                    ("↵", "details"),
                    ("o", "finder"),
                    ("q", "quit"),
                ]
            } else if app.mode == Mode::Clean {
                vec![
                    ("↑↓", "move"),
                    ("space", "select"),
                    ("a", "all safe"),
                    ("↵", "continue"),
                    ("q", "quit"),
                ]
            } else {
                vec![
                    ("m", "relocate"),
                    ("c", "clean safe"),
                    ("↵", "details"),
                    ("r", "rescan"),
                    ("q", "quit"),
                ]
            };
            render_command_bar(frame, area, app, &commands);
        }
        Phase::Details => {
            render_command_bar(
                frame,
                area,
                app,
                &[
                    ("esc", "back"),
                    ("d", "advanced delete"),
                    ("c", "clean all safe"),
                    ("o", "reveal in Finder"),
                ],
            );
        }
        Phase::RelocationSources => {}
        Phase::RelocationDestination => {}
        Phase::RelocationPlanning | Phase::Relocating => {}
        Phase::RelocationConfirm => {
            frame.render_widget(
                Paragraph::new("y move and link   n/esc cancel")
                    .block(block.title(" RELOCATION CONFIRMATION "))
                    .alignment(Alignment::Center),
                area,
            );
        }
        Phase::RelocationResult => {}
        Phase::Confirm => {
            frame.render_widget(
                Paragraph::new("Confirm or cancel in the dialog")
                    .block(block.title(" CONFIRMATION "))
                    .alignment(Alignment::Center),
                area,
            );
        }
        Phase::ReviewConfirm => {
            frame.render_widget(
                Paragraph::new("Type DELETE to confirm this one review item, or Esc to cancel")
                    .block(block.title(" ADVANCED REVIEW DELETION "))
                    .alignment(Alignment::Center),
                area,
            );
        }
        Phase::Processes => {
            let commands = if app.analysis_only {
                vec![
                    ("↑↓", "move"),
                    ("pgup", "page"),
                    ("r", "rescan"),
                    ("esc", "home"),
                    ("q", "quit"),
                ]
            } else {
                vec![
                    ("↑↓", "move"),
                    ("d", "signal"),
                    ("r", "rescan"),
                    ("esc", "home"),
                    ("q", "quit"),
                ]
            };
            render_command_bar(frame, area, app, &commands);
        }
        Phase::ProcessConfirm => {
            frame.render_widget(
                Paragraph::new("Choose graceful termination, force kill, or cancel in the dialog")
                    .block(block.title(" PROCESS CONFIRMATION "))
                    .alignment(Alignment::Center),
                area,
            );
        }
        Phase::Cleaning => {
            let total = app.cleanup_queue.len().max(1);
            let title = if app.stopped_early {
                " STOP REQUESTED • finishing the current item "
            } else {
                match app.cleanup_kind {
                    CleanupKind::Cache => " CLEANING • Esc/q stops after the current item ",
                    CleanupKind::ReviewData => {
                        " DELETING REVIEW DATA • Esc/q stops after the current item "
                    }
                }
            };
            let current_label = app
                .cleanup_queue
                .get(app.cleanup_index)
                .and_then(|index| app.entries.get(*index))
                .map(display_entry_label)
                .unwrap_or_else(|| "finishing cleanup".into());
            let elapsed = app
                .cleanup_started_at
                .map_or(Duration::ZERO, |started| started.elapsed());
            frame.render_widget(
                Gauge::default()
                    .block(block.title(title))
                    .gauge_style(
                        Style::default()
                            .fg(app.color(MINT))
                            .add_modifier(Modifier::BOLD),
                    )
                    .ratio(app.cleanup_index as f64 / total as f64)
                    .label(format!(
                        "{}  {} / {} • {}",
                        scan_spinner(elapsed),
                        app.cleanup_index,
                        app.cleanup_queue.len(),
                        current_label,
                    )),
                area,
            );
        }
        Phase::Summary => {
            let status = if app.stopped_early {
                "STOPPED"
            } else {
                "COMPLETE"
            };
            let closes_in = app
                .summary_remaining()
                .unwrap_or_default()
                .as_secs_f64()
                .ceil() as u64;
            let summary = format!(
                "{status}  •  closing in {closes_in}s (enter/q/esc now)  •  removed {}  •  disk change {}  •  cleared {}  •  skipped {}  •  failed {}",
                format_kb(app.stats.measured_removed_kb),
                format_kb(app.stats.filesystem_change_kb),
                app.stats.cleared,
                app.stats.safety_skipped,
                app.stats.failed,
            );
            frame.render_widget(
                Paragraph::new(summary)
                    .block(block.title(" SUMMARY "))
                    .alignment(Alignment::Center),
                area,
            );
        }
    }
}

pub(super) fn render_scan_page(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let page = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .split(area.inner(Margin {
        horizontal: 2,
        vertical: 0,
    }));
    frame.render_widget(
        Paragraph::new(vec![
            heading(
                app,
                format!(
                    "{}  Getting the full picture",
                    scan_spinner(app.scan_started_at.elapsed())
                ),
            ),
            label(
                app,
                format!("Read-only audit of {}", app.scan_root.display()),
            ),
        ]),
        page[1],
    );
    let stage = if app.retention_worker.is_some() {
        0
    } else if app.inventory_worker.is_some() {
        2
    } else {
        1
    };
    let spans = [
        "01 Safety checks",
        "02 Cache discovery",
        "03 Disk inventory",
    ]
    .iter()
    .enumerate()
    .map(|(i, name)| {
        Span::styled(
            format!("{name}    "),
            Style::default().fg(app.color(if i == stage {
                BLUE
            } else if i < stage {
                MINT
            } else {
                MUTED
            })),
        )
    })
    .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Line::from(spans)).wrap(Wrap { trim: true }),
        page[2],
    );
    render_details(frame, page[3], app);
    render_footer(frame, page[4], app);
}

pub(super) fn render_summary_page(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let body = area.inner(Margin {
        horizontal: 3,
        vertical: 0,
    });
    let title = if app.stopped_early {
        "Cleanup stopped."
    } else if app.stats.failed > 0 {
        "Cleanup finished with issues."
    } else {
        "A little more room."
    };
    frame.render_widget(
        Paragraph::new(vec![
            heading(app, title),
            Line::from(""),
            heading(
                app,
                format!("{} removed", format_kb(app.stats.measured_removed_kb)),
            ),
            label(
                app,
                format!(
                    "{} change in filesystem free space",
                    format_kb(app.stats.filesystem_change_kb)
                ),
            ),
            Line::from(""),
            Line::from(format!(
                "{} cleared    {} skipped    {} failed",
                app.stats.cleared, app.stats.safety_skipped, app.stats.failed
            )),
            label(
                app,
                "Filesystem accounting can differ from the size of removed files.",
            ),
        ])
        .wrap(Wrap { trim: true }),
        Rect::new(body.x, body.y + 4, body.width, 9),
    );
    if area.height >= 24 {
        render_cleanup_table(
            frame,
            Rect::new(
                body.x,
                body.y + 13,
                body.width,
                area.height.saturating_sub(16),
            ),
            app,
        );
    }
    render_command_bar(
        frame,
        Rect::new(area.x, area.bottom() - 2, area.width, 2),
        app,
        &[("↵ / Esc", "close"), ("q", "quit")],
    );
    let remaining = app
        .summary_remaining()
        .unwrap_or_default()
        .as_secs_f64()
        .ceil() as u64;
    frame.render_widget(
        Paragraph::new(label(app, format!("Closing in {remaining}s"))).alignment(Alignment::Right),
        Rect::new(area.right() - 18, area.bottom() - 3, 17, 1),
    );
}
