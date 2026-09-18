use super::*;

pub(super) const MIN_WIDTH: u16 = 60;
pub(super) const MIN_HEIGHT: u16 = 16;
pub(super) const TOP_BAR_HEIGHT: u16 = 3;

/// A single geometry source for painting and pointer navigation.
pub(super) fn content_area(area: Rect) -> Rect {
    Rect::new(
        area.x,
        area.y.saturating_add(TOP_BAR_HEIGHT),
        area.width,
        area.height.saturating_sub(TOP_BAR_HEIGHT),
    )
}

pub(super) fn active_section(app: &App) -> usize {
    match app.phase {
        Phase::Processes | Phase::ProcessConfirm => 1,
        Phase::RelocationSources
        | Phase::RelocationDestination
        | Phase::RelocationPlanning
        | Phase::RelocationConfirm
        | Phase::Relocating
        | Phase::RelocationResult => 2,
        _ => 0,
    }
}

pub(super) fn top_menu_item_area(area: Rect, index: usize) -> Rect {
    let title_width = if area.width >= 90 { 16 } else { 14 };
    let widths = if area.width >= 90 {
        [20, 20, 16]
    } else {
        [14, 14, 13]
    };
    let x = area.x + title_width + widths[..index.min(widths.len())].iter().sum::<u16>();
    Rect::new(x, area.y, widths[index.min(widths.len() - 1)], 1)
}

pub(super) fn navigation_at(column: u16, row: u16, width: u16) -> Option<usize> {
    let area = Rect::new(0, 0, width, TOP_BAR_HEIGHT);
    (0..3).find(|index| rect_contains(top_menu_item_area(area, *index), column, row))
}

pub(super) fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    app.hit_regions.borrow_mut().clear();
    app.map_paths.borrow_mut().clear();
    frame.render_widget(
        Block::default().style(
            Style::default()
                .fg(app.color(INK))
                .bg(app.color(BACKGROUND)),
        ),
        area,
    );
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new(vec![
                heading(app, "Mac Cleanup"),
                Line::from(""),
                Line::from("Expand the terminal to at least 60 × 16."),
                label(app, "Actions are paused. Esc goes back; q quits."),
            ])
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    if let Some(workspace) = &app.care {
        care_view::render(frame, area, app, workspace);
        return;
    }
    render_navigation(frame, area, app, "");
    let page = content_area(area);
    match app.phase {
        Phase::Location => render_location_picker(frame, page, app),
        Phase::Scanning => render_scan_page(frame, page, app),
        Phase::Summary => render_summary_page(frame, page, app),
        Phase::RelocationSources => render_relocation_sources(frame, page, app),
        Phase::RelocationDestination => render_relocation_destination(frame, page, app),
        Phase::RelocationPlanning | Phase::Relocating => {
            render_relocation_progress(frame, page, app)
        }
        Phase::RelocationResult => render_relocation_result(frame, page, app),
        _ => render_workspace(frame, page, app),
    }
    match app.phase {
        Phase::Confirm => render_confirmation(frame, area, app),
        Phase::ReviewConfirm => render_review_confirmation(frame, area, app),
        Phase::ProcessConfirm => render_process_confirmation(frame, area, app),
        Phase::RelocationConfirm => render_relocation_confirmation(frame, area, app),
        Phase::Details => render_entry_details(frame, area, app),
        _ => {}
    }
    if let Some(message) = &app.status_message
        && !app.is_scrollable_dialog()
    {
        frame.render_widget(
            Paragraph::new(label(app, message)).style(Style::default().bg(app.color(SURFACE))),
            Rect::new(
                page.x + 1,
                area.bottom() - 3,
                page.width.saturating_sub(2),
                1,
            ),
        );
    }
    render_overlays(frame, area, app);
}

// Sidebar navigation was replaced by the top bar. The compatibility geometry
// helpers above remain for serialized render snapshots.

/* Legacy sidebar layout retained in this comment for historical render notes.
    let focused =
        app.sidebar_focus && app.menu_is_available() && app.menu_open.is_none() && !app.show_help;
    frame.render_widget(
        Paragraph::new(Line::styled(
            if focused { " MENU · FOCUSED" } else { " MENU" },
            Style::default().fg(app.color(if focused { BLUE } else { MUTED })),
        )),
        Rect::new(area.x, area.y + 3, width - 1, 1),
    );
    let names = if width >= 23 {
        [
            "Storage audit",
            "Process health",
            if app.analysis_only {
                "Move (read only)"
            } else {
                "Move data"
            },
        ]
    } else {
        [
            "Storage",
            "Processes",
            if app.analysis_only {
                "Move (locked)"
            } else {
                "Move data"
            },
        ]
    };
    let items = names.iter().enumerate().map(|(index, name)| {
        let active = index == active_section(app);
        ListItem::new(Line::from(format!(
            "{name}{}",
            if active && focused && app.sidebar_cursor != index {
                " •"
            } else {
                ""
            }
        )))
        .style(
            Style::default().fg(app.color(if index == 2 && app.analysis_only {
                FAINT
            } else if active {
                BLUE
            } else {
                INK
            })),
        )
    });
    let list = List::new(items)
        .highlight_symbol(if focused { "› " } else { "  " })
        .highlight_style(if focused {
            Style::default()
                .fg(app.color(INK))
                .bg(app.color(SURFACE_RAISED))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.color(BLUE)).bg(app.color(SURFACE))
        });
    let mut state = ListState::default().with_selected(Some(if focused {
        app.sidebar_cursor
    } else {
        active_section(app)
    }));
    frame.render_stateful_widget(list, sidebar_list_area(area), &mut state);
    render_sidebar_summary(frame, area, app);
    frame.render_widget(
        Paragraph::new(vec![
            label(app, " Tab Switch pane"),
            label(
                app,
                if focused {
                    " ↑↓ Choose  ↵ Open"
                } else {
                    " 1–3 Open section"
                },
            ),
            label(app, " F9 Commands"),
        ]),
        Rect::new(area.x, area.bottom().saturating_sub(3), width - 1, 3),
    );
}

fn render_sidebar_summary(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let summary = sidebar_summary_area(area);
    let mut lines = vec![heading(app, " THIS SCAN")];
    if let Some(inventory) = &app.inventory {
        lines.push(label(
            app,
            format!(" {} measured", format_kb(inventory.scanned_kb)),
        ));
        lines.push(label(
            app,
            if inventory.complete {
                " Scan complete"
            } else {
                " Partial scan"
            },
        ));
        if !inventory.complete {
            lines.push(label(app, " v  View coverage"));
        }
    } else {
        lines.push(label(app, " Scanning storage…"));
    }
    frame.render_widget(Paragraph::new(lines), summary);
}
*/

pub(super) fn render_navigation(frame: &mut Frame<'_>, area: Rect, app: &App, _active: &str) {
    let focused =
        app.sidebar_focus && app.menu_is_available() && app.menu_open.is_none() && !app.show_help;
    let title_width = if area.width >= 90 { 16 } else { 14 };
    frame.render_widget(
        Block::default().style(Style::default().bg(app.color(SURFACE))),
        Rect::new(area.x, area.y, area.width, TOP_BAR_HEIGHT),
    );
    frame.render_widget(
        Paragraph::new(heading(app, " MAC CLEANUP")),
        Rect::new(area.x, area.y, title_width, 1),
    );
    let names = if area.width >= 90 {
        ["Storage audit", "Process health", "Move data"]
    } else {
        [
            "Storage",
            "Processes",
            if app.analysis_only {
                "Move (locked)"
            } else {
                "Move data"
            },
        ]
    };
    for (index, name) in names.iter().enumerate() {
        let item_area = top_menu_item_area(area, index);
        let active = index == active_section(app);
        let selected = if focused {
            app.sidebar_cursor
        } else {
            active_section(app)
        };
        let style = if index == selected && focused {
            selected_row_style(app)
                .fg(app.color(INK))
                .add_modifier(Modifier::BOLD)
        } else if active {
            Style::default()
                .fg(app.color(BLUE))
                .add_modifier(Modifier::BOLD)
        } else if index == 2 && app.analysis_only {
            Style::default().fg(app.color(FAINT))
        } else {
            Style::default().fg(app.color(MUTED))
        };
        frame.render_widget(
            Paragraph::new(Line::from(format!(
                "{}{}",
                if index == selected && focused {
                    "› "
                } else {
                    "  "
                },
                name
            )))
            .style(style),
            item_area,
        );
    }
    let status = match app.phase {
        Phase::Review if app.inventory.is_some() => {
            let inventory = app.inventory.as_ref().unwrap();
            format!(
                "{} measured · {} safe to clean",
                format_kb(inventory.scanned_kb),
                format_kb(app.ready_kb())
            )
        }
        Phase::Scanning => "SCANNING · read-only audit in progress".into(),
        Phase::Location => "Choose a volume to begin the audit".into(),
        _ => "Review before action".into(),
    };
    frame.render_widget(
        Paragraph::new(label(app, status)),
        Rect::new(
            area.x + title_width,
            area.y + 1,
            area.width.saturating_sub(title_width + 16),
            1,
        ),
    );
    frame.render_widget(
        Paragraph::new(label(
            app,
            if app.analysis_only {
                "READ ONLY"
            } else {
                "STATUS"
            },
        ))
        .alignment(Alignment::Right),
        Rect::new(area.right().saturating_sub(14), area.y + 1, 13, 1),
    );
    frame.render_widget(
        Paragraph::new(
            Line::from("─".repeat(area.width as usize))
                .style(Style::default().fg(app.color(FAINT))),
        ),
        Rect::new(area.x, area.y + 2, area.width, 1),
    );
}
pub(super) fn menu_popup_rect(area: Rect, _menu: MenuId, item_count: usize) -> Option<Rect> {
    let width = area.width.saturating_sub(2).min(44);
    let height = (item_count as u16 + 2).min(area.height.saturating_sub(1));
    if width < 12 || height < 3 {
        return None;
    }
    Some(Rect::new(
        area.x + 1,
        area.y + TOP_BAR_HEIGHT,
        width,
        height,
    ))
}

pub(super) fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.right() && row >= area.y && row < area.bottom()
}

fn render_workspace(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.phase == Phase::Review && app.inventory.is_some() {
        render_storage_workspace(frame, area, app);
        return;
    }
    let compact = area.height < 24;
    let regions = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(3),
    ])
    .split(area);
    render_metrics(frame, regions[0], app);
    let wide = regions[1].width >= 100;
    if wide {
        let body = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
            .spacing(1)
            .split(regions[1]);
        render_table(frame, body[0], app);
        render_inspector(frame, body[1], app);
    } else if !compact {
        let body = Layout::vertical([Constraint::Min(4), Constraint::Length(6)]).split(regions[1]);
        render_table(frame, body[0], app);
        render_details(frame, body[1], app);
    } else {
        render_table(frame, regions[1], app);
    }
    render_footer(frame, regions[2], app);
}
