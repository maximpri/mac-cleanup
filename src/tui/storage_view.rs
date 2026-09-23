// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn render_metric_strip(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    metrics: [(&str, String, Color); 4],
) {
    let count = if area.width < 65 { 2 } else { 4 };
    let cards = Layout::horizontal(vec![Constraint::Ratio(1, count as u32); count])
        .spacing(1)
        .split(area);
    for (index, (name, value, color)) in metrics.into_iter().take(count).enumerate() {
        frame.render_widget(
            Paragraph::new(vec![
                label(app, format!(" {name}")),
                Line::from(Span::styled(
                    format!(" {value}"),
                    Style::default()
                        .fg(app.color(color))
                        .add_modifier(Modifier::BOLD),
                )),
            ])
            .block(
                Block::default()
                    .borders(Borders::LEFT)
                    .border_style(Style::default().fg(app.color(FAINT))),
            ),
            cards[index],
        );
    }
}

pub(super) fn render_metrics(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if matches!(app.phase, Phase::Processes | Phase::ProcessConfirm) {
        render_process_metrics(frame, area, app);
        return;
    }
    let used_label = app
        .inventory
        .as_ref()
        .and_then(|inventory| inventory.volume.as_ref())
        .map_or_else(
            || "USED".to_string(),
            |volume| {
                format!(
                    "USED · {:.0}%",
                    if volume.capacity_kb == 0 {
                        0.0
                    } else {
                        volume.disk_used_kb() as f64 * 100.0 / volume.capacity_kb as f64
                    }
                )
            },
        );
    let metrics = if let Some(inventory) = &app.inventory {
        let used = inventory
            .volume
            .as_ref()
            .map_or_else(|| "n/a".into(), |volume| format_kb(volume.disk_used_kb()));
        let free = inventory
            .volume
            .as_ref()
            .map_or_else(|| "n/a".into(), |volume| format_kb(volume.disk_free_kb()));
        [
            (used_label.as_str(), used, AMBER),
            ("FREE", free, MINT),
            ("SAFE TO CLEAN", format_kb(app.ready_kb()), BLUE),
            if app.analysis_only {
                ("REVIEW", format_kb(app.review_kb()), ORCHID)
            } else {
                ("SELECTED", format_kb(app.selected_kb()), INK)
            },
        ]
    } else if app.mode == Mode::Clean {
        [
            ("CLEANABLE", format_kb(app.ready_kb()), MINT),
            ("SELECTED", format_kb(app.selected_kb()), BLUE),
            ("OPT-IN", format_kb(app.optional_kb()), AMBER),
            ("REVIEW", format_kb(app.review_kb()), ORCHID),
        ]
    } else {
        [
            ("IDENTIFIED", format_kb(app.identified_kb()), INK),
            ("CLEANABLE", format_kb(app.ready_kb()), MINT),
            ("OPT-IN", format_kb(app.optional_kb()), AMBER),
            ("REVIEW", format_kb(app.review_kb()), ORCHID),
        ]
    };
    render_metric_strip(frame, area, app, metrics);
}

pub(super) fn render_process_metrics(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let count = |health| {
        app.processes
            .iter()
            .filter(|process| process.health == health)
            .count()
    };
    let metrics = [
        ("FLAGGED", app.flagged_process_count().to_string(), INK),
        (
            "STUCK WAIT",
            count(ProcessHealth::Uninterruptible).to_string(),
            CORAL,
        ),
        ("STOPPED", count(ProcessHealth::Stopped).to_string(), AMBER),
        ("DEAD", count(ProcessHealth::Zombie).to_string(), ORCHID),
    ];
    render_metric_strip(frame, area, app, metrics);
}

pub(super) fn render_table(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if matches!(app.phase, Phase::Processes | Phase::ProcessConfirm) {
        render_process_table(frame, area, app);
    } else if app.inventory.is_some() && area.height >= 12 {
        let inventory_height = (area.height / 2).clamp(8, 22);
        let rows = Layout::vertical([Constraint::Length(inventory_height), Constraint::Min(4)])
            .split(area);
        render_inventory_summary(frame, rows[0], app);
        render_cleanup_table(frame, rows[1], app);
    } else {
        render_cleanup_table(frame, area, app);
    }
}

pub(super) fn display_entry_label(entry: &CacheEntry) -> String {
    if entry.spec.target == CacheTarget::ExactPath {
        entry.spec.path.display().to_string()
    } else {
        entry.spec.label.to_string()
    }
}

pub(super) fn render_inventory_summary(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(inventory) = &app.inventory else {
        return;
    };
    let block = panel(app, " DISK USAGE ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let (used, capacity, free, ratio) = inventory.volume.as_ref().map_or(
        ("n/a".into(), "n/a".into(), "n/a".into(), 0.0),
        |volume| {
            (
                format_kb(volume.disk_used_kb()),
                format_kb(volume.capacity_kb),
                format_kb(volume.disk_free_kb()),
                if volume.capacity_kb == 0 {
                    0.0
                } else {
                    (volume.disk_used_kb() as f64 / volume.capacity_kb as f64).clamp(0.0, 1.0)
                },
            )
        },
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                used,
                Style::default()
                    .fg(disk_usage_color(app, ratio))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" used / {capacity}   ·   {free} free")),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    if inner.height < 2 {
        return;
    }
    frame.render_widget(
        LineGauge::default()
            .ratio(ratio)
            .filled_symbol("━")
            .unfilled_symbol("─")
            .filled_style(Style::default().fg(disk_usage_color(app, ratio)))
            .unfilled_style(Style::default().fg(app.color(FAINT)))
            .label(format!("{:.0}% used", ratio * 100.0)),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );
    if inner.height < 3 {
        return;
    }
    let mut lines = Vec::new();
    if let Some(volume) = &inventory.volume
        && volume.container_free_kb.is_some()
    {
        lines.push(label(
            app,
            format!(
                "{} on this volume · {} other APFS volumes / metadata",
                format_kb(volume.used_kb),
                format_kb(volume.other_volume_kb())
            ),
        ));
    }
    lines.push(label(app, "LARGEST CONSUMERS · press 2 to explore folders"));
    let reserved = 3 + lines.len() + usize::from(!inventory.complete);
    let limit = (inner.height as usize).saturating_sub(reserved).min(12);
    let mut items: Vec<_> = inventory
        .roots
        .iter()
        .filter_map(|root| inventory.children.get(&root.path))
        .flat_map(|children| children.iter())
        .collect();
    if inventory.children.is_empty() {
        items.extend(inventory.top_level.iter());
    }
    items.sort_by(|a, b| b.size_kb.cmp(&a.size_kb).then_with(|| a.path.cmp(&b.path)));
    items.truncate(limit);
    for (index, item) in items.iter().enumerate() {
        let path_width = inner.width.saturating_sub(17) as usize;
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:>2}  ", index + 1),
                Style::default().fg(app.color(MUTED)),
            ),
            Span::styled(
                format!("{:>9}   ", format_kb(item.size_kb)),
                Style::default().fg(app.color(BLUE)),
            ),
            Span::raw(truncate_middle(
                &item.path.display().to_string(),
                path_width,
            )),
        ]));
    }
    if inner.height >= 4 {
        let mut footer = if inventory.unaccounted_kb > 0 {
            format!(
                "{} not visible in the walk · snapshots or protected data may contribute",
                format_kb(inventory.unaccounted_kb)
            )
        } else if inventory.inventory_overage_kb > 0 {
            format!(
                "{} over filesystem usage · accounting may be changing",
                format_kb(inventory.inventory_overage_kb)
            )
        } else {
            format!(
                "{} items inspected · {} inventory",
                inventory.scanned_items,
                if inventory.complete {
                    "complete"
                } else {
                    "partial"
                }
            )
        };
        if !inventory.local_snapshots.is_empty() {
            footer.push_str(&format!(
                " · {} local TM snapshot(s), latest {}",
                inventory.local_snapshots.len(),
                inventory.local_snapshots.last().expect("non-empty list")
            ));
        }
        lines.push(label(app, footer));
    }
    if !inventory.complete && inner.height >= 6 {
        lines.push(Line::from(Span::styled(
            format!(
                "PARTIAL SCAN · {} unreadable entries · check terminal Full Disk Access",
                inventory.scan_errors
            ),
            Style::default().fg(app.color(AMBER)),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2),
    );
}

pub(super) fn render_cleanup_table(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.phase == Phase::Review && app.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from("No allowlisted cleanup findings were found."),
                Line::from(Span::styled(
                    "Press e to explore folders and files.",
                    Style::default().fg(app.color(MUTED)),
                )),
            ])
            .block(
                Block::default()
                    .borders(Borders::TOP | Borders::BOTTOM)
                    .border_style(Style::default().fg(app.color(MUTED)))
                    .title(" STORAGE FOUND "),
            )
            .alignment(Alignment::Left),
            area,
        );
        return;
    }

    let header = Row::new(["", "DECISION", "FINDING", "ON DISK"])
        .style(
            Style::default()
                .fg(app.color(MUTED))
                .add_modifier(Modifier::BOLD),
        )
        .height(1);
    let rows = app.entries.iter().enumerate().map(|(index, entry)| {
        let marker = if !app.analysis_only && app.is_selectable(index) {
            if app.selected.contains(&index) {
                "[×]"
            } else {
                "[ ]"
            }
        } else if app.mode == Mode::Clean
            && entry.status == CacheStatus::Review
            && entry.size_kb > 0
            && entry.outcome.is_none()
        {
            "[!]"
        } else {
            " · "
        };
        let (status, color) = match &entry.outcome {
            Some(CleanupOutcome::Cleared { .. }) => ("CLEARED", MINT),
            Some(CleanupOutcome::SafetySkipped(_)) => ("SKIPPED", AMBER),
            Some(CleanupOutcome::Failed { .. }) => ("FAILED", CORAL),
            None => (
                match entry.status {
                    CacheStatus::Ready => "SAFE TO CLEAN",
                    CacheStatus::Optional => "REINSTALLABLE",
                    CacheStatus::InUse => "CLOSE APP",
                    CacheStatus::Review => "REVIEW IMPACT",
                    _ => entry.status.label(),
                },
                match entry.status {
                    CacheStatus::Ready => MINT,
                    CacheStatus::Optional | CacheStatus::InUse => AMBER,
                    CacheStatus::Review => ORCHID,
                    CacheStatus::Whitelisted => BLUE,
                    CacheStatus::ScanError | CacheStatus::Symlink | CacheStatus::Invalid => CORAL,
                    CacheStatus::Missing => FAINT,
                },
            ),
        };
        Row::new(vec![
            Cell::from(marker),
            Cell::from(status).style(Style::default().fg(app.color(color))),
            Cell::from(display_entry_label(entry)),
            Cell::from(format_kb(entry.size_kb)).style(Style::default().fg(app.color(MUTED))),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(13),
            Constraint::Fill(1),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(panel(
        app,
        Span::styled(
            format!(" FINDINGS  ·  {} items ", app.entries.len()),
            Style::default()
                .fg(app.color(BLUE))
                .add_modifier(Modifier::BOLD),
        ),
    ))
    .row_highlight_style(selected_row_style(app))
    .highlight_symbol("▸ ");
    let mut state = TableState::default().with_selected(Some(app.cursor));
    frame.render_stateful_widget(table, area, &mut state);
    table_hits(
        app,
        area,
        state.offset(),
        app.entries.len(),
        1,
        1,
        HitTarget::Finding,
    );
    render_vertical_scrollbar(frame, area, app.entries.len(), app.cursor, app);
}

pub(super) fn location_list_area(area: Rect) -> Rect {
    let top = area.y + if area.height >= 24 { 10 } else { 8 };
    Rect::new(
        area.x + 2,
        top,
        area.width.saturating_sub(4),
        area.bottom().saturating_sub(top + 3),
    )
}

pub(super) fn render_location_picker(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let tall = area.height >= 24;
    frame.render_widget(
        Paragraph::new(vec![
            heading(app, "Where should we look?"),
            label(
                app,
                "Choose a volume to map its storage and find recoverable space.",
            ),
            Line::from(""),
            label(
                app,
                "Local drives first. External and network volumes are also supported.",
            ),
        ])
        .wrap(Wrap { trim: true }),
        Rect::new(
            area.x + 2,
            area.y + 4,
            area.width.saturating_sub(4),
            if tall { 5 } else { 3 },
        ),
    );
    let table_area = location_list_area(area);
    let rows = app.locations.iter().map(|location| {
        Row::new(vec![
            Cell::from(location.kind.label()).style(Style::default().fg(app.color(
                match location.kind {
                    ScanLocationKind::Local => MINT,
                    ScanLocationKind::Usb => BLUE,
                    ScanLocationKind::Network => ORCHID,
                },
            ))),
            Cell::from(location.label.clone()),
            Cell::from(location.path.display().to_string())
                .style(Style::default().fg(app.color(MUTED))),
        ])
        .height(2)
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Percentage(35),
            Constraint::Fill(1),
        ],
    )
    .header(
        Row::new(["TYPE", "LOCATION", "EXACT PATH"])
            .style(Style::default().fg(app.color(MUTED)))
            .bottom_margin(1),
    )
    .block(panel(app, " AVAILABLE VOLUMES "))
    .row_highlight_style(selected_row_style(app))
    .highlight_symbol("▸ ");
    let mut state = TableState::default().with_selected(Some(app.location_cursor));
    frame.render_stateful_widget(table, table_area, &mut state);
    table_hits(
        app,
        table_area,
        state.offset(),
        app.locations.len(),
        2,
        2,
        HitTarget::Location,
    );
    render_vertical_scrollbar(
        frame,
        table_area,
        app.locations.len(),
        app.location_cursor,
        app,
    );
    render_command_bar(
        frame,
        Rect::new(area.x, area.bottom() - 2, area.width, 2),
        app,
        &[
            ("↑↓", "choose"),
            ("↵", "scan"),
            ("esc", "home"),
            ("q", "quit"),
        ],
    );
}
