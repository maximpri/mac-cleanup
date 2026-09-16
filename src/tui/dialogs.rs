use super::*;

pub(super) fn render_overlays(frame: &mut Frame<'_>, area: Rect, app: &App) {
    render_menu_overlay_if_open(frame, area, app);
    render_help_overlay_if_open(frame, area, app);
}

pub(super) fn render_menu_overlay_if_open(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(menu) = app.menu_open else {
        return;
    };
    let items = app.menu_items(menu);
    let Some(popup) = menu_popup_rect(area, menu, items.len()) else {
        return;
    };
    frame.render_widget(Clear, popup);

    let rows = items
        .iter()
        .map(|item| {
            let label_style = if item.enabled {
                Style::default().fg(app.color(INK))
            } else {
                Style::default().fg(app.color(MUTED))
            };
            let shortcut_style = if item.enabled {
                Style::default().fg(app.color(BLUE))
            } else {
                Style::default().fg(app.color(MUTED))
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("  {:<24}", item.label),
                    label_style.add_modifier(if item.enabled {
                        Modifier::BOLD
                    } else {
                        Modifier::DIM
                    }),
                ),
                Span::styled(
                    if item.enabled { item.shortcut } else { "—" },
                    shortcut_style,
                ),
            ]))
        })
        .collect::<Vec<_>>();
    let block = popup_panel(
        app,
        Span::styled(
            format!(" {} · ←/→ categories ", menu.label()),
            Style::default()
                .fg(app.color(BLUE))
                .add_modifier(Modifier::BOLD),
        ),
        BLUE,
    );
    frame.render_stateful_widget(
        List::new(rows)
            .block(block)
            .highlight_style(selected_row_style(app))
            .highlight_symbol(" ▸ "),
        popup,
        &mut ListState::default().with_selected(Some(app.menu_cursor)),
    );
}

pub(super) fn render_help_overlay_if_open(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if !app.show_help {
        return;
    }
    let rows = [
        ("1 / 2 / 3", "Storage / processes / move data"),
        (
            "e / h / f / v",
            "Explore folders / storage heatmap / cleanup decisions / scan coverage",
        ),
        ("F8", "Switch pane (alternative to Tab)"),
        (
            "Tab / Shift+Tab",
            "Switch focus between the menu and content",
        ),
        ("Enter / ←", "Open folder / return to parent in Explore"),
        ("↑↓ or j / k", "Choose a task or move through a list"),
        ("Enter", "Open details, a task, or selected cleanup review"),
        ("Space / a", "Select one / all eligible findings"),
        ("d / c", "Review deletion of one item / all safe items"),
        ("o / r", "Reveal in Finder / rescan"),
        ("i", "Toggle reinstallable downloads and rescan"),
        ("m", "Move useful data to an external volume"),
        ("PgUp / PgDn", "Page lists or scroll dialog details"),
        ("F9 / Alt+N", "Open commands / Navigate commands"),
        ("Mouse", "Select visible rows; wheel scrolls"),
        ("Esc / q", "Go back / quit"),
    ];
    let mut lines = vec![
        heading(app, "A workspace you can use from the keyboard."),
        Line::from(""),
    ];
    for (key, description) in rows {
        lines.push(Line::from(vec![
            Span::styled(format!("{key:<17}"), Style::default().fg(app.color(BLUE))),
            Span::raw(description),
        ]));
    }
    lines.extend([
        Line::from(""),
        heading(app, "REVIEW BEFORE ACTION"),
        Line::from(if app.analysis_only {
            "This session is read-only. Cleanup, relocation, and signals are disabled."
        } else {
            "Every deletion, move, and process signal requires a separate confirmation."
        }),
        label(
            app,
            "App-managed data requires typing DELETE. Safety checks run again before changes.",
        ),
    ]);
    render_dialog(
        frame,
        area,
        app,
        "KEYBOARD GUIDE",
        BLUE,
        lines,
        vec![label(app, "Esc / Enter close")],
    );
}

/// Keep the decision controls pinned while long paths and consequences scroll.
pub(super) fn render_dialog<'a>(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    title: &str,
    accent: Color,
    text: Vec<Line<'a>>,
    controls: Vec<Line<'a>>,
) {
    let width = area.width.saturating_sub(4).min(112);
    let content = Paragraph::new(text).wrap(Wrap { trim: false });
    let footer = Paragraph::new(controls).wrap(Wrap { trim: true });
    let inner_width = width.saturating_sub(4);
    let footer_height = footer.line_count(inner_width) as u16;
    let line_count = content.line_count(inner_width);
    let height = (line_count as u16)
        .saturating_add(footer_height + 4)
        .min(area.height.saturating_sub(2));
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let block = popup_panel(app, format!(" {title} "), accent);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let rows = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(footer_height),
    ])
    .split(inner);
    let max_scroll = line_count
        .saturating_sub(rows[0].height as usize)
        .min(u16::MAX as usize) as u16;
    app.dialog_max_scroll.set(max_scroll);
    frame.render_widget(
        content.scroll((app.dialog_scroll.min(max_scroll), 0)),
        rows[0],
    );
    if max_scroll > 0 {
        frame.render_widget(
            Paragraph::new(label(app, "↑↓ / PgUp PgDn  scroll to review all details")),
            rows[1],
        );
    }
    frame.render_widget(footer, rows[2]);
}

pub(super) fn render_review_confirmation(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(entry) = app.review_target.and_then(|index| app.entries.get(index)) else {
        return;
    };
    render_dialog(
        frame,
        area,
        app,
        "ADVANCED REVIEW DELETION",
        CORAL,
        vec![
            heading(app, "PERMANENTLY DELETE APP-MANAGED DATA?"),
            Line::from(""),
            heading(
                app,
                format!("{}  ·  {}", entry.spec.label, format_kb(entry.size_kb)),
            ),
            label(app, "EXACT PATH"),
            Line::from(entry.spec.path.display().to_string()),
            Line::from(""),
            label(app, "IMPACT"),
            Line::from(entry.spec.note),
            Line::from(
                "Everything inside this folder will be permanently deleted. Settings, history, containers, apps, or local data may be lost.",
            ),
            label(
                app,
                "Close the related application first. The folder itself will be retained.",
            ),
        ],
        vec![
            Line::from(Span::styled(
                if app.review_confirmation_error {
                    "Phrase does not match. Type DELETE exactly."
                } else {
                    "Type DELETE, then press Enter:"
                },
                Style::default().fg(app.color(CORAL)),
            )),
            heading(app, format!("> {}_", app.review_confirmation)),
            label(app, "Esc cancels without changing files"),
        ],
    );
}

pub(super) fn render_relocation_confirmation(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(plan) = &app.relocation_plan else {
        return;
    };
    render_dialog(
        frame,
        area,
        app,
        "CONFIRM RELOCATION",
        BLUE,
        vec![
            heading(app, "Move this directory to external storage?"),
            label(
                app,
                format!(
                    "{} across {} files",
                    format_kb(plan.size_kb),
                    plan.file_count
                ),
            ),
            Line::from(""),
            label(app, "SOURCE"),
            Line::from(plan.source.display().to_string()),
            label(app, "DESTINATION"),
            Line::from(plan.destination.display().to_string()),
            Line::from(""),
            Line::from("The copy is content-verified before the original path becomes a symlink."),
            Line::from("Keep the external volume mounted to use this data at its original path."),
        ],
        vec![heading(app, "y move and link    n / Esc cancel")],
    );
}

pub(super) fn render_confirmation(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let includes_optional = app
        .selected
        .iter()
        .filter_map(|index| app.entries.get(*index))
        .any(|entry| entry.status == CacheStatus::Optional);
    let mut text = vec![
        heading(
            app,
            if includes_optional {
                "PERMANENTLY DELETE REINSTALLABLE CACHE?"
            } else {
                "PERMANENTLY DELETE SELECTED DATA?"
            },
        ),
        label(
            app,
            format!(
                "{} selected items  ·  {} on disk",
                app.selected.len(),
                format_kb(app.selected_kb())
            ),
        ),
        Line::from(""),
        label(app, "EXACT TARGETS"),
    ];
    for entry in app
        .selected
        .iter()
        .filter_map(|index| app.entries.get(*index))
    {
        text.extend([
            heading(
                app,
                format!("{}  ·  {}", entry.spec.label, format_kb(entry.size_kb)),
            ),
            Line::from(entry.spec.path.display().to_string()),
            label(
                app,
                if entry.spec.target == CacheTarget::ExactPath {
                    "Remove this exact entry, including its directory if present."
                } else {
                    "Clear contents; keep the containing folder."
                },
            ),
            Line::from(""),
        ]);
    }
    if includes_optional {
        text.push(Line::from(
            "This opts in only the highlighted item. Large files may download again.",
        ));
    }
    text.push(Line::from(
        "Deletion is permanent. Each target is checked again before any change.",
    ));
    render_dialog(
        frame,
        area,
        app,
        "CONFIRM CLEANUP",
        CORAL,
        text,
        vec![Line::from(Span::styled(
            "y delete permanently    n / Esc cancel",
            Style::default()
                .fg(app.color(CORAL))
                .add_modifier(Modifier::BOLD),
        ))],
    );
}

pub(super) fn render_process_confirmation(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(process) = app
        .process_target
        .and_then(|index| app.processes.get(index))
    else {
        return;
    };
    render_dialog(
        frame,
        area,
        app,
        "CONFIRM PROCESS ACTION",
        CORAL,
        vec![
            heading(app, "Send a signal to this process?"),
            label(
                app,
                format!(
                    "PID {}  ·  {}  ·  running {}",
                    process.pid,
                    process.health.label(),
                    process.elapsed
                ),
            ),
            Line::from(""),
            label(app, "FULL COMMAND"),
            Line::from(process.command.as_str()),
            Line::from(""),
            Line::from(process.health.explanation()),
            Line::from("Unsaved work may be lost. Identity and ownership are checked again first."),
        ],
        vec![
            heading(app, "t request graceful exit (SIGTERM)"),
            Line::from(Span::styled(
                "k force kill (SIGKILL)",
                Style::default().fg(app.color(CORAL)),
            )),
            label(app, "Esc / n cancels without sending a signal"),
        ],
    );
}
