use super::*;

pub(super) fn render_process_table(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.processes.is_empty() {
        let message = app.process_scan_error.as_deref().map_or(
            "No current-account processes were available for review.",
            |_| "Process inspection failed; no processes can be acted on.",
        );
        let mut lines = vec![Line::from(""), Line::from(message)];
        if let Some(error) = &app.process_scan_error {
            lines.push(Line::from(Span::styled(
                error.as_str(),
                Style::default().fg(app.color(CORAL)),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "Refresh to scan the current account again.",
                Style::default().fg(app.color(MUTED)),
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::TOP | Borders::BOTTOM)
                        .border_style(Style::default().fg(app.color(MUTED)))
                        .title(" PROCESS REVIEW "),
                )
                .alignment(Alignment::Left),
            area,
        );
        return;
    }

    let header = Row::new(["", "STATE", "PID", "AGE", "CPU", "COMMAND"]).style(
        Style::default()
            .fg(app.color(MUTED))
            .add_modifier(Modifier::BOLD),
    );
    let rows = app.processes.iter().map(|process| {
        let marker = if !process.signalable {
            " · "
        } else if process.health == ProcessHealth::Running {
            "[ ]"
        } else {
            "[!]"
        };
        let (status, color) = match &process.outcome {
            Some(ProcessOutcome::Exited { .. }) => ("EXITED", MINT),
            Some(ProcessOutcome::SignalSent { .. }) => ("SIGNALED", AMBER),
            Some(ProcessOutcome::SafetySkipped(_)) => ("SKIPPED", AMBER),
            Some(ProcessOutcome::Failed(_)) => ("FAILED", CORAL),
            None => (
                process.health.label(),
                match process.health {
                    ProcessHealth::Running => MINT,
                    ProcessHealth::Zombie => ORCHID,
                    ProcessHealth::Stopped => AMBER,
                    ProcessHealth::Uninterruptible => CORAL,
                },
            ),
        };
        Row::new(vec![
            Cell::from(marker),
            Cell::from(status).style(Style::default().fg(app.color(color))),
            Cell::from(process.pid.to_string()),
            Cell::from(process.elapsed.as_str()),
            Cell::from(format!("{}%", process.cpu_percent)),
            Cell::from(process.command.as_str()),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(11),
            Constraint::Length(7),
            Constraint::Fill(1),
        ],
    )
    .header(header)
    .block(panel(
        app,
        Span::styled(
            format!(" LIVE PROCESSES  ·  {} visible ", app.processes.len()),
            Style::default()
                .fg(app.color(BLUE))
                .add_modifier(Modifier::BOLD),
        ),
    ))
    .row_highlight_style(selected_row_style(app))
    .highlight_symbol("▸ ");
    let mut state = TableState::default().with_selected(Some(app.process_cursor));
    frame.render_stateful_widget(table, area, &mut state);
    table_hits(
        app,
        area,
        state.offset(),
        app.processes.len(),
        1,
        1,
        HitTarget::Process,
    );
    render_vertical_scrollbar(frame, area, app.processes.len(), app.process_cursor, app);
}

pub(super) fn render_process_details(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let lines = if let Some(process) = app.processes.get(app.process_cursor) {
        let action = match &process.outcome {
            Some(ProcessOutcome::Exited { signal }) => {
                format!("Exited after {}.", signal.label())
            }
            Some(ProcessOutcome::SignalSent { signal }) => format!(
                "{} was delivered, but the process was still present after 750 ms. Refresh or explicitly force-kill it.",
                signal.label()
            ),
            Some(ProcessOutcome::SafetySkipped(reason)) => format!("Skipped: {reason}."),
            Some(ProcessOutcome::Failed(error)) => format!("Signal failed: {error}."),
            None => process.signal_block_reason.clone().unwrap_or_else(|| {
                "Review the process first; d offers graceful termination or an explicit force kill."
                    .into()
            }),
        };
        vec![
            Line::from(vec![
                Span::styled("State  ", Style::default().fg(app.color(MUTED))),
                Span::raw(format!(
                    "{} ({}) • parent {} • running {}",
                    process.health.label(),
                    process.state,
                    process.parent_pid,
                    process.elapsed
                )),
            ]),
            Line::from(vec![
                Span::styled("Why    ", Style::default().fg(app.color(MUTED))),
                Span::raw(process.health.explanation()),
            ]),
            Line::from(vec![
                Span::styled("Action  ", Style::default().fg(app.color(MUTED))),
                Span::raw(action),
            ]),
        ]
    } else if let Some(error) = &app.process_scan_error {
        vec![Line::from(format!("Process inspection failed: {error}"))]
    } else {
        vec![Line::from(
            "No unhealthy processes need review. Refresh with r at any time.",
        )]
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(app, " DETAILS "))
            .wrap(Wrap { trim: true }),
        area,
    );
}
