use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StorageTab {
    Explore,
    Heatmap,
    Decisions,
    Coverage,
}

const MAX_HEATMAP_SEGMENTS: usize = 20;

struct HeatmapSegment {
    item_index: Option<usize>,
    name: String,
    size_kb: u64,
    action: &'static str,
    color: Color,
}

impl App {
    pub(super) fn explorer_items(&self) -> Vec<&StorageItem> {
        let Some(inventory) = &self.inventory else {
            return Vec::new();
        };
        if let Some(path) = &self.explorer_path {
            return inventory
                .children
                .get(path)
                .map(|items| items.iter().collect())
                .unwrap_or_default();
        }
        let mut items: Vec<_> = inventory
            .roots
            .iter()
            .filter_map(|root| inventory.children.get(&root.path))
            .flat_map(|items| items.iter())
            .collect();
        // Reports supplied by embedded callers may not include an exploration index.
        if inventory.children.is_empty() {
            items.extend(inventory.top_level.iter());
        }
        items.sort_by(|a, b| b.size_kb.cmp(&a.size_kb).then_with(|| a.path.cmp(&b.path)));
        items
    }

    pub(super) fn open_consumer(&mut self) {
        self.explorer_details = false;
        let Some(item) = self.explorer_items().get(self.explorer_cursor).copied() else {
            return;
        };
        if item.kind != StorageItemKind::Directory {
            self.explorer_details = true;
            self.dialog_scroll = 0;
            return;
        }
        let path = item.path.clone();
        self.explorer_history
            .push((self.explorer_path.clone(), self.explorer_cursor));
        self.explorer_path = Some(path);
        self.explorer_cursor = 0;
        self.status_message = None;
    }

    pub(super) fn explorer_back(&mut self) {
        if let Some((path, cursor)) = self.explorer_history.pop() {
            self.explorer_path = path;
            self.explorer_cursor = cursor;
        } else {
            self.explorer_path = None;
            self.explorer_cursor = 0;
        }
        self.status_message = None;
    }

    pub(super) fn switch_storage_tab(&mut self, tab: StorageTab) {
        if tab == StorageTab::Explore
            && self.storage_tab == StorageTab::Decisions
            && let Some(entry) = self.entries.get(self.cursor)
        {
            let path = entry.spec.path.clone();
            if self
                .inventory
                .as_ref()
                .is_some_and(|inventory| inventory.children.contains_key(&path))
            {
                self.explorer_history
                    .push((self.explorer_path.clone(), self.explorer_cursor));
                self.explorer_path = Some(path);
                self.explorer_cursor = 0;
            }
        }
        if tab == StorageTab::Decisions
            && matches!(self.storage_tab, StorageTab::Explore | StorageTab::Heatmap)
        {
            let path = self
                .explorer_items()
                .get(self.explorer_cursor)
                .map(|item| item.path.clone());
            if let Some(path) = path
                && let Some(index) = self
                    .entries
                    .iter()
                    .position(|entry| entry.spec.path == path)
                    .or_else(|| {
                        self.entries
                            .iter()
                            .position(|entry| entry.spec.path.starts_with(&path))
                    })
            {
                self.cursor = index;
            }
        }
        self.explorer_details = false;
        self.storage_tab = tab;
        self.status_message = None;
    }

    pub(super) fn handle_storage_navigation(&mut self, code: KeyCode) -> bool {
        if self.inventory.is_none() {
            return false;
        }
        match code {
            KeyCode::Char('e') => self.switch_storage_tab(StorageTab::Explore),
            KeyCode::Char('h') => self.switch_storage_tab(StorageTab::Heatmap),
            KeyCode::Char('f') => self.switch_storage_tab(StorageTab::Decisions),
            KeyCode::Char('v') => self.switch_storage_tab(StorageTab::Coverage),
            KeyCode::Char(']') | KeyCode::Char('[') => {
                let tabs = [
                    StorageTab::Explore,
                    StorageTab::Heatmap,
                    StorageTab::Decisions,
                    StorageTab::Coverage,
                ];
                let index = tabs
                    .iter()
                    .position(|tab| *tab == self.storage_tab)
                    .unwrap_or(0);
                self.switch_storage_tab(
                    tabs[(index
                        + if code == KeyCode::Char('[') {
                            tabs.len() - 1
                        } else {
                            1
                        })
                        % tabs.len()],
                );
            }
            _ if self.storage_tab == StorageTab::Decisions => return false,
            KeyCode::Char('r') => self.restart_scan(),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('m') | KeyCode::Char('M') if !self.analysis_only => {
                self.open_relocation_sources()
            }
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Left if self.explorer_details => {
                self.explorer_details = false
            }
            KeyCode::Esc
                if matches!(self.storage_tab, StorageTab::Explore | StorageTab::Heatmap)
                    && self.explorer_path.is_some() =>
            {
                self.explorer_back()
            }
            KeyCode::Esc => {
                // Storage is the first destination; leaving a storage view
                // returns to its root instead of an empty home screen.
                self.phase = Phase::Review;
                self.explorer_path = None;
                self.explorer_cursor = 0;
                self.explorer_history.clear();
            }
            _ if self.explorer_details
                && matches!(
                    code,
                    KeyCode::Up
                        | KeyCode::Down
                        | KeyCode::PageUp
                        | KeyCode::PageDown
                        | KeyCode::Home
                        | KeyCode::End
                        | KeyCode::Char('j')
                        | KeyCode::Char('k')
                ) =>
            {
                match code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.dialog_scroll = self.dialog_scroll.saturating_sub(1)
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.dialog_scroll = self
                            .dialog_scroll
                            .saturating_add(1)
                            .min(self.dialog_max_scroll.get())
                    }
                    KeyCode::PageUp => self.dialog_scroll = self.dialog_scroll.saturating_sub(8),
                    KeyCode::PageDown => {
                        self.dialog_scroll = self
                            .dialog_scroll
                            .saturating_add(8)
                            .min(self.dialog_max_scroll.get())
                    }
                    KeyCode::Home => self.dialog_scroll = 0,
                    KeyCode::End => self.dialog_scroll = self.dialog_max_scroll.get(),
                    _ => {}
                }
            }
            _ if self.storage_tab == StorageTab::Coverage => match code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.coverage_scroll = self
                        .coverage_scroll
                        .saturating_add(1)
                        .min(self.coverage_max_scroll.get())
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.coverage_scroll = self.coverage_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => {
                    self.coverage_scroll = self
                        .coverage_scroll
                        .saturating_add(8)
                        .min(self.coverage_max_scroll.get())
                }
                KeyCode::PageUp => self.coverage_scroll = self.coverage_scroll.saturating_sub(8),
                KeyCode::Home => self.coverage_scroll = 0,
                KeyCode::Char('p') => {
                    let result = open_access_settings();
                    self.status_message = Some(match result {
                        Ok(status) if status.success() => "Full Disk Access opened. Enable your terminal app, restart it, then scan again.".into(),
                        _ => "Could not open Settings. Full Disk Access is in Privacy & Security.".into(),
                    });
                }
                _ => {}
            },
            KeyCode::Down | KeyCode::Char('j') => {
                self.explorer_cursor =
                    (self.explorer_cursor + 1).min(self.explorer_items().len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.explorer_cursor = self.explorer_cursor.saturating_sub(1)
            }
            KeyCode::PageDown => {
                self.explorer_cursor =
                    (self.explorer_cursor + 10).min(self.explorer_items().len().saturating_sub(1))
            }
            KeyCode::PageUp => self.explorer_cursor = self.explorer_cursor.saturating_sub(10),
            KeyCode::Home => self.explorer_cursor = 0,
            KeyCode::End => self.explorer_cursor = self.explorer_items().len().saturating_sub(1),
            KeyCode::Enter | KeyCode::Right => self.open_consumer(),
            KeyCode::Backspace | KeyCode::Left => self.explorer_back(),
            KeyCode::Char('i') => {
                self.explorer_details = !self.explorer_details;
                self.dialog_scroll = 0;
            }
            KeyCode::Char('o') | KeyCode::Char('O') => self.reveal_current(),
            _ => {}
        }
        true
    }
}

pub(super) fn render_storage_workspace(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let compact = area.height < 26;
    let regions = Layout::vertical([
        Constraint::Length(if compact { 3 } else { 4 }),
        Constraint::Length(if compact { 2 } else { 3 }),
        Constraint::Length(if compact { 1 } else { 2 }),
        Constraint::Length(if compact { 1 } else { 3 }),
        Constraint::Min(2),
        Constraint::Length(if compact { 2 } else { 3 }),
    ])
    .split(area);
    render_metrics(frame, regions[1], app);
    let tabs = Layout::horizontal([
        Constraint::Percentage(25),
        Constraint::Percentage(25),
        Constraint::Percentage(25),
        Constraint::Percentage(25),
    ])
    .split(regions[2]);
    let compact_labels = ["e Explore", "h Heatmap", "f Cleanup", "v Coverage"];
    for (index, (tab, title)) in [
        (StorageTab::Explore, "e Explore folders"),
        (StorageTab::Heatmap, "h Storage heatmap"),
        (StorageTab::Decisions, "f Cleanup decisions"),
        (StorageTab::Coverage, "v Scan coverage"),
    ]
    .iter()
    .enumerate()
    {
        frame.render_widget(
            Paragraph::new(if area.width < 65 {
                compact_labels[index]
            } else {
                *title
            })
            .style(if app.storage_tab == *tab {
                selected_row_style(app).fg(app.color(BLUE))
            } else {
                Style::default().fg(app.color(MUTED))
            }),
            tabs[index],
        );
        app.hit_regions
            .borrow_mut()
            .push((tabs[index], HitTarget::StorageTab(*tab)));
    }
    let inventory = app
        .inventory
        .as_ref()
        .expect("storage workspace has inventory");
    let intro = if compact && !inventory.complete {
        format!(
            "Partial scan · {} unreadable · v for coverage",
            inventory.scan_errors
        )
    } else {
        match app.storage_tab {
            StorageTab::Explore => format!(
                "Where is the space?  {} measured · largest first",
                format_kb(inventory.scanned_kb)
            ),
            StorageTab::Heatmap => format!(
                "Where is the waste?  {} measured · larger blocks use more space",
                format_kb(inventory.scanned_kb)
            ),
            StorageTab::Decisions => format!(
                "What can I remove?  {} safe to clean · {} selected",
                format_kb(app.ready_kb()),
                format_kb(app.selected_kb())
            ),
            StorageTab::Coverage => {
                "How much did we see?  Measured files and disk accounting can differ.".into()
            }
        }
    };
    let subtitle = match app.storage_tab {
        StorageTab::Explore => {
            "Enter opens a folder. Each row is a direct child; parent totals are never added to children."
        }
        StorageTab::Heatmap => {
            "Each tile is proportional to allocated space. Colors show cleanable, optional, or review-only data."
        }
        StorageTab::Decisions => {
            "Select a finding to see its impact and recovery options. Enter reviews details or your selection."
        }
        StorageTab::Coverage => {
            "Unknown space is not a cleanup estimate. Review access errors and APFS accounting below."
        }
    };
    let coverage = if !inventory.complete {
        format!(
            "PARTIAL SCAN · {} unreadable entries · v explains missing space",
            inventory.scan_errors
        )
    } else if inventory.unaccounted_kb > 0 {
        format!(
            "{} unexplained by this scan · v shows disk accounting",
            format_kb(inventory.unaccounted_kb)
        )
    } else {
        subtitle.into()
    };
    frame.render_widget(
        Paragraph::new(vec![
            heading(app, intro),
            label(app, subtitle),
            Line::from(Span::styled(
                coverage,
                Style::default().fg(app.color(AMBER)),
            )),
        ]),
        regions[3],
    );
    match app.storage_tab {
        StorageTab::Explore => {
            if app.explorer_details {
                render_consumer_inspector(frame, regions[4], app);
            } else if area.width >= 110 {
                let body =
                    Layout::horizontal([Constraint::Percentage(64), Constraint::Percentage(36)])
                        .spacing(1)
                        .split(regions[4]);
                render_explorer(frame, body[0], app);
                render_consumer_inspector(frame, body[1], app);
            } else {
                render_explorer(frame, regions[4], app);
            }
            render_command_bar(
                frame,
                regions[5],
                app,
                &[
                    ("Enter", "open"),
                    ("←", "up"),
                    ("i", "info"),
                    ("o", "Finder"),
                    ("h", "heatmap"),
                    ("f", "decisions"),
                    ("v", "coverage"),
                ],
            );
        }
        StorageTab::Heatmap => {
            if app.explorer_details {
                render_consumer_inspector(frame, regions[4], app);
            } else if area.width >= 100 {
                let body =
                    Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
                        .spacing(1)
                        .split(regions[4]);
                render_heatmap(frame, body[0], app);
                render_consumer_inspector(frame, body[1], app);
            } else {
                render_heatmap(frame, regions[4], app);
            }
            render_command_bar(
                frame,
                regions[5],
                app,
                &[
                    ("↑↓", "select"),
                    ("Enter", "open"),
                    ("←", "up"),
                    ("o", "Finder"),
                    ("e", "explore"),
                    ("f", "decisions"),
                    ("v", "coverage"),
                ],
            );
        }
        StorageTab::Decisions => {
            if area.width >= 100 {
                let body =
                    Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                        .spacing(1)
                        .split(regions[4]);
                render_cleanup_table(frame, body[0], app);
                render_inspector(frame, body[1], app);
            } else {
                render_cleanup_table(frame, regions[4], app);
            }
            if app.analysis_only {
                render_command_bar(
                    frame,
                    regions[5],
                    app,
                    &[
                        ("Enter", "details"),
                        ("o", "Finder"),
                        ("e", "explore"),
                        ("h", "heatmap"),
                        ("[]", "view"),
                    ],
                );
            } else {
                render_command_bar(
                    frame,
                    regions[5],
                    app,
                    &[
                        ("Space", "select"),
                        ("Enter", "review"),
                        ("c", "review safe"),
                        ("d", "delete item"),
                        ("e", "explore"),
                    ],
                );
            }
        }
        StorageTab::Coverage => {
            let block = panel(app, " SCAN COVERAGE · ↑↓ scroll ");
            let inner = block.inner(regions[4]);
            let paragraph = Paragraph::new(coverage_lines(app)).wrap(Wrap { trim: true });
            let max_scroll = paragraph
                .line_count(inner.width)
                .saturating_sub(inner.height as usize)
                .min(u16::MAX as usize) as u16;
            app.coverage_max_scroll.set(max_scroll);
            frame.render_widget(
                paragraph
                    .scroll((app.coverage_scroll.min(max_scroll), 0))
                    .block(block),
                regions[4],
            );
            render_command_bar(
                frame,
                regions[5],
                app,
                &[
                    ("p", "access settings"),
                    ("r", "rescan"),
                    ("e", "explore"),
                    ("h", "heatmap"),
                    ("↑↓", "scroll"),
                ],
            );
        }
    }
}

fn render_explorer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items = app.explorer_items();
    let total: u64 = items.iter().map(|item| item.size_kb).sum();
    let path = app
        .explorer_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "All scanned locations".into());
    let block = panel(
        app,
        format!(
            " {} ",
            truncate_middle(&path, area.width.saturating_sub(6) as usize)
        ),
    );
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                heading(app, "No measured children"),
                label(
                    app,
                    "This folder may be empty, unreadable, or outside scan coverage.",
                ),
                label(app, "← Back    o Reveal in Finder    v Scan coverage"),
            ])
            .wrap(Wrap { trim: true })
            .block(block),
            area,
        );
        return;
    }
    let rows = items.iter().map(|item| {
        let percent = if total == 0 {
            0.0
        } else {
            item.size_kb as f64 * 100.0 / total as f64
        };
        let name = if app.explorer_path.is_none() {
            item.path.display().to_string()
        } else {
            item.path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        let marker = if item.kind == StorageItemKind::Directory {
            "▸"
        } else if item.kind == StorageItemKind::Symlink {
            "@"
        } else {
            "·"
        };
        Row::new(vec![
            Cell::from(format!("{marker} {name}")),
            Cell::from(format_kb(item.size_kb)).style(Style::default().fg(app.color(BLUE))),
            Cell::from(format!("{percent:>5.1}%")),
            Cell::from("━".repeat((percent / 10.0).round() as usize))
                .style(Style::default().fg(app.color(BLUE))),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Length(10),
            Constraint::Length(7),
            Constraint::Length(if area.width >= 80 { 10 } else { 0 }),
        ],
    )
    .header(
        Row::new(["FOLDER / FILE", "ON DISK", "% HERE", "SHARE"])
            .style(Style::default().fg(app.color(MUTED))),
    )
    .block(block)
    .row_highlight_style(selected_row_style(app))
    .highlight_symbol("› ");
    let mut state = TableState::default().with_selected(Some(app.explorer_cursor));
    frame.render_stateful_widget(table, area, &mut state);
    table_hits(
        app,
        area,
        state.offset(),
        items.len(),
        1,
        1,
        HitTarget::Consumer,
    );
    render_vertical_scrollbar(frame, area, items.len(), app.explorer_cursor, app);
}

fn render_heatmap(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let path = app
        .explorer_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "All scanned locations".into());
    let title = format!(
        " STORAGE HEATMAP · {} ",
        truncate_middle(&path, area.width.saturating_sub(24) as usize)
    );
    let block = panel(app, title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 12 || inner.height < 4 {
        frame.render_widget(
            label(app, "Expand the terminal to see the storage heatmap."),
            inner,
        );
        return;
    }

    let items = app.explorer_items();
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                heading(app, "No measured children"),
                label(
                    app,
                    "This folder may be empty, unreadable, or outside scan coverage.",
                ),
                label(app, "Run a rescan or open Scan coverage for missing space."),
            ])
            .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    let top = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(
        label(
            app,
            "Larger tiles use more allocated space · select one to inspect or open it",
        ),
        top,
    );
    let body = Rect::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    );
    let columns = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
        .spacing(1)
        .split(body);
    let chart = columns[0];
    let legend = columns[1];
    let segments = heatmap_segments(app);
    let tile_columns = (chart.width / 2) as usize;
    let tile_rows = chart.height as usize;
    if tile_columns == 0 || tile_rows == 0 {
        return;
    }
    let tile_count = tile_columns.saturating_mul(tile_rows);
    let sizes: Vec<u64> = segments.iter().map(|segment| segment.size_kb).collect();
    let total = sizes.iter().sum();
    let counts = heatmap_tile_counts(&sizes, total, tile_count);
    let mut owners = Vec::with_capacity(tile_count);
    for (index, count) in counts.iter().enumerate() {
        owners.extend(std::iter::repeat_n(index, *count));
    }
    owners.resize(tile_count, segments.len().saturating_sub(1));

    let lines = (0..tile_rows)
        .map(|row| {
            let mut spans = Vec::with_capacity(tile_columns);
            for col in 0..tile_columns {
                let owner = owners[row * tile_columns + col];
                let segment = &segments[owner];
                let mut style = Style::default().fg(app.color(segment.color));
                if segment.item_index == Some(app.explorer_cursor) {
                    style = style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
                }
                spans.push(Span::styled(heatmap_marker(segment.action), style));
            }
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), chart);

    let legend_width = legend.width.saturating_sub(3) as usize;
    let selected_line = app
        .explorer_items()
        .get(app.explorer_cursor)
        .map(|item| {
            format!(
                "Selected: {}",
                truncate_middle(&item.path.display().to_string(), legend_width)
            )
        })
        .unwrap_or_else(|| "Select a tile to see its exact path.".into());
    let mut legend_lines = vec![
        heading(app, "ACTIONS · TOP CONSUMERS"),
        label(app, selected_line),
        Line::from(""),
    ];
    for segment in &segments {
        let percent = if total == 0 {
            0.0
        } else {
            segment.size_kb as f64 * 100.0 / total as f64
        };
        let marker_style = Style::default().fg(app.color(segment.color));
        let selected = segment.item_index == Some(app.explorer_cursor);
        let prefix = if selected { "›" } else { " " };
        let details = truncate_middle(
            &format!("{percent:>5.1}% {} · {}", segment.name, segment.action),
            legend_width,
        );
        legend_lines.push(Line::from(vec![
            Span::styled(
                prefix,
                if selected {
                    selected_row_style(app)
                } else {
                    Style::default()
                },
            ),
            Span::styled(heatmap_marker(segment.action), marker_style),
            Span::raw(format!(" {details}")),
        ]));
    }
    frame.render_widget(Paragraph::new(legend_lines), legend);

    let mut cursor = 0usize;
    for (segment, count) in segments.iter().zip(counts) {
        let Some(item_index) = segment.item_index else {
            cursor += count;
            continue;
        };
        let end = cursor.saturating_add(count);
        while cursor < end {
            let row = cursor / tile_columns;
            let col = cursor % tile_columns;
            let run = (end - cursor).min(tile_columns - col);
            if run > 0 {
                app.hit_regions.borrow_mut().push((
                    Rect::new(
                        chart.x + (col as u16).saturating_mul(2),
                        chart.y + row as u16,
                        (run as u16).saturating_mul(2).min(chart.width),
                        1,
                    ),
                    HitTarget::Consumer(item_index),
                ));
            }
            cursor += run;
        }
    }
}

fn heatmap_segments(app: &App) -> Vec<HeatmapSegment> {
    let mut indexed = app
        .explorer_items()
        .into_iter()
        .enumerate()
        .collect::<Vec<_>>();
    indexed.sort_by(|(_, left), (_, right)| {
        right
            .size_kb
            .cmp(&left.size_kb)
            .then_with(|| left.path.cmp(&right.path))
    });
    let keep = if indexed.len() > MAX_HEATMAP_SEGMENTS {
        MAX_HEATMAP_SEGMENTS.saturating_sub(1)
    } else {
        indexed.len()
    };
    let mut segments = indexed
        .iter()
        .take(keep)
        .map(|(item_index, item)| {
            let (action, color) = heatmap_action(app, item);
            HeatmapSegment {
                item_index: Some(*item_index),
                name: heatmap_item_name(item),
                size_kb: item.size_kb,
                action,
                color,
            }
        })
        .collect::<Vec<_>>();
    if keep < indexed.len() {
        let remainder = indexed
            .iter()
            .skip(keep)
            .map(|(_, item)| item.size_kb)
            .sum();
        segments.push(HeatmapSegment {
            item_index: None,
            name: format!("{} smaller items", indexed.len() - keep),
            size_kb: remainder,
            action: "OTHER MEASURED",
            color: MUTED,
        });
    }
    segments
}

fn heatmap_item_name(item: &StorageItem) -> String {
    item.path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| item.path.display().to_string())
}

pub(super) fn heatmap_action(app: &App, item: &StorageItem) -> (&'static str, Color) {
    let exact_status = app
        .entries
        .iter()
        .find(|entry| entry.spec.path == item.path)
        .map(|entry| entry.status);
    if let Some(status) = exact_status {
        return match status {
            CacheStatus::Ready => ("CLEANABLE", MINT),
            CacheStatus::Optional => ("OPTIONAL", AMBER),
            CacheStatus::Review => ("REVIEW / KEEP", CORAL),
            CacheStatus::InUse => ("IN USE", AMBER),
            CacheStatus::Whitelisted => ("PROTECTED", BLUE),
            _ => heatmap_category_action(item.category),
        };
    }
    let descendants = app
        .entries
        .iter()
        .filter(|entry| entry.spec.path != item.path && entry.spec.path.starts_with(&item.path));
    let mut has_ready = false;
    let mut has_optional = false;
    let mut has_review = false;
    for entry in descendants {
        has_ready |= entry.status == CacheStatus::Ready;
        has_optional |= entry.status == CacheStatus::Optional;
        has_review |= entry.status == CacheStatus::Review;
    }
    if has_ready {
        ("CLEANABLE BELOW", MINT)
    } else if has_optional {
        ("OPTIONAL BELOW", AMBER)
    } else if has_review {
        ("REVIEW BELOW", CORAL)
    } else {
        heatmap_category_action(item.category)
    }
}

fn heatmap_category_action(category: StorageCategory) -> (&'static str, Color) {
    match category {
        StorageCategory::TemporaryData => ("TEMPORARY", MINT),
        StorageCategory::DeveloperData => ("DEVELOPER", BLUE),
        StorageCategory::ApplicationData => ("APP DATA", ORCHID),
        StorageCategory::Applications => ("APPLICATIONS", BLUE),
        StorageCategory::PersonalData => ("PERSONAL / KEEP", CORAL),
        StorageCategory::SystemData => ("SYSTEM / KEEP", MUTED),
        StorageCategory::Other => ("REVIEW", MUTED),
    }
}

pub(super) fn heatmap_marker(action: &str) -> &'static str {
    if action.contains("CLEANABLE") || action == "TEMPORARY" {
        "██"
    } else if action.contains("OPTIONAL") || action == "IN USE" {
        "▓▓"
    } else if action.contains("REVIEW") || action.contains("KEEP") {
        "░░"
    } else if matches!(action, "DEVELOPER" | "APP DATA" | "APPLICATIONS") {
        "▒▒"
    } else {
        "··"
    }
}

pub(super) fn heatmap_tile_counts(sizes: &[u64], total: u64, tiles: usize) -> Vec<usize> {
    if sizes.is_empty() || total == 0 || tiles == 0 {
        return vec![0; sizes.len()];
    }
    let total_u128 = u128::from(total);
    let tiles_u128 = tiles as u128;
    let mut counts = sizes
        .iter()
        .map(|size| (u128::from(*size) * tiles_u128 / total_u128) as usize)
        .collect::<Vec<_>>();
    let nonzero = sizes.iter().filter(|size| **size > 0).count();
    if nonzero <= tiles {
        for (size, count) in sizes.iter().zip(&mut counts) {
            if *size > 0 && *count == 0 {
                *count = 1;
            }
        }
    }
    while counts.iter().sum::<usize>() > tiles {
        let Some(index) = counts
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 1)
            .min_by_key(|(index, _)| sizes[*index])
            .map(|(index, _)| index)
        else {
            break;
        };
        counts[index] -= 1;
    }
    let missing = tiles.saturating_sub(counts.iter().sum::<usize>());
    if missing > 0 {
        let mut order = (0..sizes.len()).collect::<Vec<_>>();
        order.sort_by(|left, right| {
            let left_remainder = (u128::from(sizes[*left]) * tiles_u128) % total_u128;
            let right_remainder = (u128::from(sizes[*right]) * tiles_u128) % total_u128;
            right_remainder
                .cmp(&left_remainder)
                .then_with(|| sizes[*right].cmp(&sizes[*left]))
        });
        for step in 0..missing {
            counts[order[step % order.len()]] += 1;
        }
    }
    counts
}

fn render_consumer_inspector(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items = app.explorer_items();
    let Some(item) = items.get(app.explorer_cursor) else {
        return;
    };
    let matched = app
        .entries
        .iter()
        .find(|entry| entry.spec.path == item.path);
    let mut lines = vec![
        heading(app, format_kb(item.size_kb)),
        label(app, item.category.label()),
        Line::from(""),
        label(app, "SELECTED PATH"),
        Line::from(item.path.display().to_string()),
        Line::from(""),
        heading(app, "Understand this space"),
    ];
    if let Some(entry) = matched {
        let guidance = decision_guidance(entry);
        lines.extend([
            Line::from(entry.spec.note),
            Line::from(""),
            heading(app, "Recommended next step"),
            Line::from(guidance.recommendation),
            Line::from(""),
            label(app, "IF REMOVED"),
            Line::from(guidance.impact),
            label(app, "RECOVERY"),
            Line::from(guidance.recovery),
            Line::from(""),
            heading(app, "f  Review this cleanup finding"),
        ]);
    } else {
        lines.extend([Line::from(if item.kind == StorageItemKind::Directory { "Folder total includes everything beneath it. Open it to find the largest children." } else { "Allocated size of this file. Size alone does not mean it is disposable." }), Line::from(""), heading(app, "Recommended next step"), Line::from("Inspect the contents and keep data you still need. Use the owning app to manage its data."), Line::from(""), label(app, "No exact cleanup rule for this path."), label(app, "f opens available cleanup decisions.")]);
    }
    lines.extend([
        Line::from(""),
        label(
            app,
            if item.kind == StorageItemKind::Directory {
                "Enter  Explore children"
            } else {
                "o      Inspect this file in Finder"
            },
        ),
        label(app, "o      Reveal in Finder"),
        label(app, "←      Return to parent"),
    ]);
    let block = panel(
        app,
        if app.explorer_details {
            " ITEM DETAILS · ↑↓ scroll · Esc back "
        } else {
            " UNDERSTAND & DECIDE · i full details "
        },
    );
    let inner = block.inner(area);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true });
    let max_scroll = paragraph
        .line_count(inner.width)
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    let scroll = if app.explorer_details {
        app.dialog_max_scroll.set(max_scroll);
        app.dialog_scroll.min(max_scroll)
    } else {
        0
    };
    frame.render_widget(paragraph.scroll((scroll, 0)).block(block), area);
}

fn coverage_lines(app: &App) -> Vec<Line<'static>> {
    let Some(inventory) = &app.inventory else {
        return vec![];
    };
    let mut lines = vec![
        heading(
            app,
            if inventory.complete {
                "Scan finished without read errors"
            } else {
                "Partial scan — some storage could not be inspected"
            },
        ),
        Line::from(""),
        heading(app, "Disk accounting"),
    ];
    if let Some(volume) = &inventory.volume {
        lines.extend([
            Line::from(format!(
                "{} used of {} · {} free",
                format_kb(volume.disk_used_kb()),
                format_kb(volume.capacity_kb),
                format_kb(volume.disk_free_kb())
            )),
            Line::from(format!(
                "{} measured on the accounted volume",
                format_kb(inventory.scanned_on_volume_kb)
            )),
            Line::from(format!(
                "{} not explained by this walk",
                format_kb(inventory.unaccounted_kb)
            )),
            Line::from(format!(
                "{} in other APFS volumes / metadata",
                format_kb(volume.other_volume_kb())
            )),
        ]);
    }
    if let Some(error) = &inventory.volume_error {
        lines.push(Line::from(error.clone()));
    }
    if inventory.inventory_overage_kb > 0 {
        lines.push(Line::from(format!("Measured sizes exceed volume usage by {}. Shared blocks or changes during scanning may contribute.", format_kb(inventory.inventory_overage_kb))));
    }
    lines.extend([Line::from(""), Line::from("Unexplained space may include protected files, snapshots, and data outside the scan scope. These amounts are not automatically reclaimable."), Line::from(""), heading(app, format!("{} local Time Machine snapshots", inventory.local_snapshots.len()))]);
    lines.extend(
        inventory
            .local_snapshots
            .iter()
            .map(|date| Line::from(date.clone())),
    );
    lines.extend([Line::from(""), heading(app, format!("{} unreadable entries", inventory.scan_errors)), Line::from("Press p to open Full Disk Access settings. Enable the terminal hosting this app, restart it, and scan again. Access may not resolve every error."), Line::from(""), label(app, "Sample unreadable paths:")]);
    lines.extend(
        inventory
            .scan_error_paths
            .iter()
            .map(|path| Line::from(path.display().to_string())),
    );
    if inventory.scan_error_paths.is_empty() {
        lines.push(Line::from("No unreadable paths recorded."));
    }
    lines.extend([Line::from(""), heading(app, "Scanned locations")]);
    lines.extend(inventory.roots.iter().map(|root| {
        Line::from(format!(
            "{}  {} · {} errors",
            format_kb(root.size_kb),
            root.path.display(),
            root.scan_errors
        ))
    }));
    lines
}

// Keep a settings shortcut from blocking the terminal indefinitely.
fn open_access_settings() -> io::Result<std::process::ExitStatus> {
    let mut child = Command::new(OPEN_COMMAND)
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Settings launch timed out",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}
