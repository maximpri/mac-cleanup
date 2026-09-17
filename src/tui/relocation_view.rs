use super::*;

pub(super) fn render_relocation_sources(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let page = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(area);

    let outer = panel(
        app,
        Span::styled(
            " RELOCATE LARGE DATA • SELECT SOURCE ",
            Style::default()
                .fg(app.color(BLUE))
                .add_modifier(Modifier::BOLD),
        ),
    );
    let inner = outer.inner(page[0]);
    frame.render_widget(outer, page[0]);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(inner);
    let scan_note = if app
        .inventory
        .as_ref()
        .is_some_and(|inventory| !inventory.complete)
    {
        "The inventory was partial; relocation will validate the selected source again."
    } else {
        "These directories are review-only storage consumers. Relocation copies data and leaves the original path usable through a symlink."
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(
                "Choose useful, high-storage data that does not need startup-disk performance.",
            ),
            Line::from(Span::styled(
                scan_note,
                Style::default().fg(app.color(AMBER)),
            )),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        sections[0],
    );

    if app.relocation_sources.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "No relocatable user-owned directories were found in the largest-consumer inventory.",
            )
            .block(panel(app, " SOURCES "))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
            sections[1],
        );
    } else {
        let rows = app.relocation_sources.iter().map(|item| {
            Row::new(vec![
                Cell::from(format_kb(item.size_kb)),
                Cell::from(item.category.label()),
                Cell::from(item.path.display().to_string()),
            ])
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(12),
                Constraint::Length(19),
                Constraint::Min(30),
            ],
        )
        .header(
            Row::new(["SIZE", "CATEGORY", "SOURCE PATH"]).style(
                Style::default()
                    .fg(app.color(MUTED))
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" REVIEW-ONLY SOURCES "),
        )
        .row_highlight_style(selected_row_style(app))
        .highlight_symbol("▸ ");
        let mut state = TableState::default().with_selected(Some(app.relocation_source_cursor));
        frame.render_stateful_widget(table, sections[1], &mut state);
        table_hits(
            app,
            sections[1],
            state.offset(),
            app.relocation_sources.len(),
            1,
            1,
            HitTarget::Source,
        );
        render_vertical_scrollbar(
            frame,
            sections[1],
            app.relocation_sources.len(),
            app.relocation_source_cursor,
            app,
        );
    }
    frame.render_widget(
        Paragraph::new(
            "Only directories inside the current account home or /private/tmp can be selected.",
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        sections[2],
    );
    frame.render_widget(
        Paragraph::new("↑↓/jk move   enter choose source   esc back   q quit")
            .block(panel(app, " KEYS "))
            .alignment(Alignment::Center),
        page[1],
    );
}

pub(super) fn render_relocation_destination(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let page = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(area);

    let outer = panel(
        app,
        Span::styled(
            " RELOCATE LARGE DATA • DESTINATION ",
            Style::default()
                .fg(app.color(BLUE))
                .add_modifier(Modifier::BOLD),
        ),
    );
    let inner = outer.inner(page[0]);
    frame.render_widget(outer, page[0]);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Min(4),
            Constraint::Length(4),
        ])
        .split(inner);
    let source = app.relocation_source.as_ref();
    let source_path = source
        .map(|item| item.path.display().to_string())
        .unwrap_or_else(|| "(none selected)".into());
    let source_size = source
        .map(|item| format_kb(item.size_kb))
        .unwrap_or_else(|| "unknown size".into());
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Relocate useful data to an external volume instead of deleting it.",
                Style::default()
                    .fg(app.color(BLUE))
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "Source       ",
                    Style::default().fg(app.color(MUTED)),
                ),
                Span::raw(source_path),
            ]),
            Line::from(vec![
                Span::styled(
                    "Inventory    ",
                    Style::default().fg(app.color(MUTED)),
                ),
                Span::raw(source_size),
            ]),
            Line::from(Span::styled(
                "The destination must already exist under /Volumes on a different local filesystem.",
                Style::default().fg(app.color(AMBER)),
            )),
        ])
        .wrap(Wrap { trim: true }),
        sections[0],
    );
    let mut input_lines = vec![
        Line::from(Span::styled(
            "Destination root (the copied folder is placed beneath this path):",
            Style::default().fg(app.color(MUTED)),
        )),
        Line::from(Span::styled(
            format!("> {}_", app.relocation_destination),
            Style::default()
                .fg(app.color(INK))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("Example: /Volumes/EXT_DISK/MacCleanup"),
        Line::from("~ is expanded to the current account home."),
    ];
    if let Some(error) = &app.relocation_destination_error {
        input_lines.push(Line::from(Span::styled(
            error,
            Style::default()
                .fg(app.color(CORAL))
                .add_modifier(Modifier::BOLD),
        )));
    }
    frame.render_widget(
        Paragraph::new(input_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" EXTERNAL DESTINATION "),
            )
            .wrap(Wrap { trim: true }),
        sections[1],
    );
    frame.render_widget(
        Paragraph::new(
            "Enter validates the source and destination. No files change until the next confirmation.",
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        sections[2],
    );
    frame.render_widget(
        Paragraph::new("type path   enter validate   backspace edit   esc back   q quit")
            .block(panel(app, " KEYS "))
            .alignment(Alignment::Center),
        page[1],
    );
}

pub(super) fn render_relocation_progress(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let page = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(area);
    let source = app
        .relocation_source
        .as_ref()
        .map(|item| item.path.display().to_string())
        .unwrap_or_else(|| "(source)".into());
    let destination = app
        .relocation_worker
        .as_ref()
        .map(|worker| worker.template.destination.display().to_string())
        .or_else(|| {
            app.relocation_plan
                .as_ref()
                .map(|plan| plan.destination.display().to_string())
        })
        .unwrap_or_else(|| app.relocation_destination.clone());
    let elapsed = app
        .relocation_started_at
        .map_or(Duration::ZERO, |started| started.elapsed());
    let (title, status, note) = if app.phase == Phase::RelocationPlanning {
        (
            " RELOCATION VALIDATION ",
            "Validating source ownership, open handles, process state, and external destination…",
            "No files have been changed.",
        )
    } else {
        (
            " RELOCATION IN PROGRESS ",
            "Copying and verifying file contents. The original path will be linked only after verification…",
            "Do not disconnect the external volume.",
        )
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                status,
                Style::default()
                    .fg(app.color(BLUE))
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Source      ", Style::default().fg(app.color(MUTED))),
                Span::raw(source),
            ]),
            Line::from(vec![
                Span::styled("Destination ", Style::default().fg(app.color(MUTED))),
                Span::raw(destination),
            ]),
            Line::from(format!("Elapsed     {}", format_elapsed(elapsed))),
            Line::from(Span::styled(note, Style::default().fg(app.color(AMBER)))),
        ])
        .block(panel(app, title))
        .wrap(Wrap { trim: true }),
        page[0],
    );
    frame.render_widget(
        Paragraph::new(
            "Please wait. Keyboard exit is disabled while filesystem relocation is active.",
        )
        .block(panel(app, " SAFETY "))
        .alignment(Alignment::Center),
        page[1],
    );
}

pub(super) fn render_relocation_result(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let page = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(area);
    let Some(report) = &app.relocation_report else {
        return;
    };
    let (title, heading, color) = match report.status {
        RelocationStatus::Relocated => (
            " RELOCATION COMPLETE ",
            "The directory was copied, verified, and replaced by a symlink.",
            MINT,
        ),
        RelocationStatus::Failed => (
            " RELOCATION FAILED ",
            "The relocation did not complete. Review the details before retrying.",
            CORAL,
        ),
        RelocationStatus::Planned => (" RELOCATION NOT STARTED ", "No files were changed.", AMBER),
    };
    let original = if report.status == RelocationStatus::Relocated {
        if report.backup_removed {
            "Original path: verified absolute symlink; temporary backup removed."
        } else {
            "Original path: verified absolute symlink; temporary backup retained for safety."
        }
    } else {
        "Original path was not intentionally replaced; inspect the details if rollback was attempted."
    };
    let mut lines = vec![
        Line::from(Span::styled(
            heading,
            Style::default()
                .fg(app.color(color))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Source      ", Style::default().fg(app.color(MUTED))),
            Span::raw(report.source.display().to_string()),
        ]),
        Line::from(vec![
            Span::styled("Destination ", Style::default().fg(app.color(MUTED))),
            Span::raw(report.destination.display().to_string()),
        ]),
        Line::from(format!(
            "Data        {} across {} file(s) and {} directory(s)",
            format_kb(report.size_kb),
            report.file_count,
            report.directory_count
        )),
        Line::from(Span::styled(
            original,
            Style::default().fg(app.color(AMBER)),
        )),
    ];
    if let Some(message) = &report.message {
        lines.push(Line::from(Span::styled(
            format!("Details     {message}"),
            Style::default().fg(app.color(MUTED)),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(app.color(color)))
                    .title(title),
            )
            .wrap(Wrap { trim: true }),
        page[0],
    );
    frame.render_widget(
        Paragraph::new("Enter/Esc rescans storage   q quit")
            .block(panel(app, " KEYS "))
            .alignment(Alignment::Center),
        page[1],
    );
}
