use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StorageTab {
    Explore,
    Heatmap,
    Decisions,
    Coverage,
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
        Constraint::Length(1),
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
            Rect::new(tabs[index].x, tabs[index].y, tabs[index].width, 1),
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
                "Storage map · {} measured · larger areas use more space",
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
            "Choose a folder to see inside it. Enter opens it; ← returns to the parent."
        }
        StorageTab::Heatmap => {
            "Area shows size. Folder colors help you navigate; cleanup eligibility is reviewed separately."
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
            } else if area.width >= 90 {
                let body =
                    Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)])
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
            render_consumer_inspector(frame, regions[4], app);
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

fn render_consumer_inspector(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items = app.explorer_items();
    let Some(item) = items.get(app.explorer_cursor) else {
        frame.render_widget(
            Paragraph::new(vec![
                heading(app, "No measured contents"),
                label(app, "This folder may be empty or unreadable."),
                label(app, "← Back    v Scan coverage"),
            ])
            .wrap(Wrap { trim: true })
            .block(panel(app, " STORAGE MAP ")),
            area,
        );
        return;
    };
    if !app.explorer_details {
        super::folder_map::render_folder_map(frame, area, app, item);
        return;
    }
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
    frame.render_widget(block, area);
    render_inspector_text(frame, inner, app, lines);
}

fn render_inspector_text(frame: &mut Frame<'_>, area: Rect, app: &App, lines: Vec<Line<'static>>) {
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true });
    let max_scroll = paragraph
        .line_count(area.width)
        .saturating_sub(area.height as usize)
        .min(u16::MAX as usize) as u16;
    let scroll = if app.explorer_details {
        app.dialog_max_scroll.set(max_scroll);
        app.dialog_scroll.min(max_scroll)
    } else {
        0
    };
    frame.render_widget(paragraph.scroll((scroll, 0)), area);
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
