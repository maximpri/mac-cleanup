// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{ai, storage};

pub(super) struct DestinationWork {
    receiver: Receiver<DestinationMessage>,
    cancelled: Arc<AtomicBool>,
    inference_cancelled: Arc<AtomicBool>,
}

impl DestinationWork {
    pub(super) fn pause_inference(&self) {
        self.inference_cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for DestinationWork {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.pause_inference();
    }
}

enum DestinationMessage {
    Measured(Vec<relocation::Destination>),
    Advice(Result<ai::RelocationAdvice, String>),
}

impl App {
    pub(super) fn start_relocation_advice(&mut self) {
        self.relocation_advice_worker = None;
        self.relocation_destinations.clear();
        let Some(source) = self.relocation_source.clone() else {
            return;
        };
        self.relocation_advice = "Checking mounted external drives…".into();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let inference_cancelled = Arc::new(AtomicBool::new(false));
        let ai_flag = inference_cancelled.clone();
        let allow_ai = self.relocation_allow_ai;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let destinations = relocation::destinations(&source.path, &flag);
            let required = relocation::required_destination_kb(source.size_kb, 0);
            let facts = destinations
                .iter()
                .enumerate()
                .filter(|(_, destination)| destination.free_kb >= required)
                .map(|(index, d)| ai::RelocationDestination {
                    id: format!("v{}", index + 1),
                    free_kb: d.free_kb,
                    capacity_kb: d.capacity_kb,
                })
                .collect::<Vec<_>>();
            if sender
                .send(DestinationMessage::Measured(destinations))
                .is_err()
                || flag.load(Ordering::Relaxed)
            {
                return;
            }
            let advice = if facts.is_empty() {
                Err("No detected external Mac volume fits the estimate. Connect a drive and press F5.".into())
            } else if !allow_ai || ai_flag.load(Ordering::Relaxed) {
                Err("Local AI is paused while memory pressure is critical.".into())
            } else {
                let age =
                    storage::folder_age(&source.path, 2000, Duration::from_secs(2), &flag).ok();
                let recently_modified = age.as_ref().and_then(|a| {
                    let recent = a.newest_age_secs.map(|seconds| seconds < 7 * 86400)?;
                    (recent || (a.complete && a.errors == 0)).then_some(recent)
                });
                ai::relocation_advice(
                    &ai::RelocationContext {
                        category: source.category,
                        allocated_kb: source.size_kb,
                        recently_modified,
                        destinations: facts,
                    },
                    &ai_flag,
                )
            };
            let _ = sender.send(DestinationMessage::Advice(advice));
        });
        self.relocation_advice_worker = Some(DestinationWork {
            receiver,
            cancelled,
            inference_cancelled,
        });
    }

    pub(super) fn poll_relocation_advice(&mut self) {
        if self.phase != Phase::RelocationDestination {
            self.relocation_advice_worker = None;
            return;
        }
        loop {
            let message = self
                .relocation_advice_worker
                .as_ref()
                .map(|work| work.receiver.try_recv());
            match message {
                Some(Ok(DestinationMessage::Measured(destinations))) => {
                    self.relocation_destinations = destinations;
                    // Never overwrite a typed path or change a user's destination
                    // in response to a late model reply.
                    if self.relocation_destination.is_empty() {
                        self.cycle_relocation_destination(false);
                    }
                    self.relocation_advice =
                        "Measured drives ready · consulting on-device AI…".into();
                }
                Some(Ok(DestinationMessage::Advice(advice))) => {
                    self.relocation_advice = match advice {
                        Ok(advice) if advice.choice == "keep_local" =>
                            "Local AI suggests keeping this folder local. Review its performance and offline needs.".into(),
                        Ok(advice) if advice.choice == "inspect_first" =>
                            "Local AI suggests inspecting this folder first. Check its owner and external-drive support.".into(),
                        Ok(advice) => {
                            let index = advice.choice.strip_prefix('v').and_then(|s| s.parse::<usize>().ok()).and_then(|n| n.checked_sub(1));
                            match index.and_then(|i| self.relocation_destinations.get(i)) {
                                Some(d) => format!("Local AI suggests reviewing {}. Tab selects a drive; Enter validates your choice.", ai::display_text(&d.path.display().to_string())),
                                None => "Measured destinations · AI advice expired; press F5 to refresh.".into(),
                            }
                        }
                        Err(error) => format!("Measured destinations · {}", ai::display_text(&error)),
                    };
                    self.relocation_advice_worker = None;
                    break;
                }
                Some(Err(TryRecvError::Disconnected)) => {
                    self.relocation_advice_worker = None;
                    self.relocation_advice =
                        "Measured destinations · local advice stopped. F5 retries.".into();
                    break;
                }
                _ => break,
            }
        }
    }

    pub(super) fn cycle_relocation_destination(&mut self, backwards: bool) {
        let Some(source) = &self.relocation_source else {
            return;
        };
        let required = relocation::required_destination_kb(source.size_kb, 0);
        let choices = self
            .relocation_destinations
            .iter()
            .filter(|d| d.free_kb >= required)
            .collect::<Vec<_>>();
        if choices.is_empty() {
            return;
        }
        let current = choices
            .iter()
            .position(|d| d.path == Path::new(&self.relocation_destination));
        let index = match current {
            Some(i) if backwards => (i + choices.len() - 1) % choices.len(),
            Some(i) => (i + 1) % choices.len(),
            None if backwards => choices.len() - 1,
            None => 0,
        };
        self.relocation_destination = choices[index].path.display().to_string();
        self.relocation_destination_error = None;
    }
}

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
    let page = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(area);
    let outer = panel(app, " REALLOCATE LOCAL STORAGE • EXTERNAL DESTINATION ");
    let inner = outer.inner(page[0]);
    frame.render_widget(outer, page[0]);
    let source = app.relocation_source.as_ref();
    let size = source.map_or(0, |s| s.size_kb);
    let source_path = source
        .map(|s| ai::display_text(&s.path.display().to_string()))
        .unwrap_or_default();
    let required = relocation::required_destination_kb(size, 0);
    let mut lines = vec![
        Line::from(format!("Source: {source_path}")),
        Line::from(format!(
            "{} measured · copy verified before linking",
            format_kb(size)
        )),
        Line::from(Span::styled(
            format!("> {}_", ai::display_text(&app.relocation_destination)),
            Style::default()
                .fg(app.color(BLUE))
                .add_modifier(Modifier::BOLD),
        )),
    ];
    if let Some(error) = &app.relocation_destination_error {
        lines.push(Line::from(Span::styled(
            ai::display_text(error),
            Style::default().fg(app.color(CORAL)),
        )));
    }
    lines.push(Line::from(Span::styled(
        app.relocation_advice.clone(),
        Style::default().fg(app.color(AMBER)),
    )));
    if inner.height >= 10 {
        lines.push(Line::from(""));
        lines.push(Line::from(
            "Detected drives (Tab cycles those that fit the estimate):",
        ));
        for destination in app.relocation_destinations.iter().take(3) {
            let fits = if destination.free_kb >= required {
                "fits estimate"
            } else {
                "too small"
            };
            lines.push(Line::from(format!(
                "{} · {} free / {} · {fits}",
                ai::display_text(&destination.path.display().to_string()),
                format_kb(destination.free_kb),
                format_kb(destination.capacity_kb)
            )));
        }
        lines.push(Line::from(
            "Type an existing folder on an external APFS/Mac OS Extended drive.",
        ));
        lines.push(Line::from(
            "Keep the drive connected. Apps must support a symlink at the original path.",
        ));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    frame.render_widget(
        Paragraph::new("Tab drive · F5 refresh · Enter validate · Esc back")
            .block(panel(app, " No files change until plan confirmation ")),
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn fixture() -> (tempfile::TempDir, App) {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("Archive");
        fs::create_dir(&source).unwrap();
        let mut app = App::new(&Cli::parse_from(["diskray"]), home.path()).unwrap();
        app.retention_worker = None;
        app.phase = Phase::RelocationDestination;
        app.relocation_source = Some(StorageItem {
            path: source.canonicalize().unwrap(),
            size_kb: 1024,
            kind: StorageItemKind::Directory,
            category: storage::StorageCategory::PersonalData,
        });
        (home, app)
    }

    fn drive(name: &str, free_kb: u64) -> relocation::Destination {
        relocation::Destination {
            path: Path::new("/Volumes").join(name),
            free_kb,
            capacity_kb: 1_000_000,
            volume_uuid: name.into(),
        }
    }

    #[test]
    fn late_measurements_and_ai_never_overwrite_typed_destination() {
        let (_home, mut app) = fixture();
        app.relocation_destination = "/Volumes/My drive/Chosen folder".into();
        let (sender, receiver) = mpsc::channel();
        app.relocation_advice_worker = Some(DestinationWork {
            receiver,
            cancelled: Arc::new(AtomicBool::new(false)),
            inference_cancelled: Arc::new(AtomicBool::new(false)),
        });
        sender
            .send(DestinationMessage::Measured(vec![drive(
                "Other drive",
                900_000,
            )]))
            .unwrap();
        sender
            .send(DestinationMessage::Advice(Ok(ai::RelocationAdvice {
                choice: "v1".into(),
            })))
            .unwrap();
        app.poll_relocation_advice();
        assert_eq!(
            app.relocation_destination,
            "/Volumes/My drive/Chosen folder"
        );
        assert!(
            app.relocation_advice
                .contains("Local AI suggests reviewing /Volumes/Other drive")
        );
        assert!(app.relocation_plan.is_none());
        assert_eq!(app.phase, Phase::RelocationDestination);
    }

    #[test]
    fn measured_fallback_cycles_only_fitting_drives_and_keeps_input_on_error() {
        let (_home, mut app) = fixture();
        let (sender, receiver) = mpsc::channel();
        let flag = Arc::new(AtomicBool::new(false));
        app.relocation_advice_worker = Some(DestinationWork {
            receiver,
            cancelled: flag.clone(),
            inference_cancelled: Arc::new(AtomicBool::new(false)),
        });
        sender
            .send(DestinationMessage::Measured(vec![
                drive("A", 900_000),
                drive("Small", 10),
                drive("B", 800_000),
            ]))
            .unwrap();
        sender
            .send(DestinationMessage::Advice(Err("model unavailable".into())))
            .unwrap();
        app.poll_relocation_advice();
        assert_eq!(app.relocation_destination, "/Volumes/A");
        app.handle_relocation_destination_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.relocation_destination, "/Volumes/B");
        app.cycle_relocation_destination(true);
        assert_eq!(app.relocation_destination, "/Volumes/A");
        assert!(app.relocation_advice.contains("Measured destinations"));
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn leaving_destination_cancels_work_and_discards_late_results() {
        let (_home, mut app) = fixture();
        let (sender, receiver) = mpsc::channel();
        let flag = Arc::new(AtomicBool::new(false));
        app.relocation_advice_worker = Some(DestinationWork {
            receiver,
            cancelled: flag.clone(),
            inference_cancelled: Arc::new(AtomicBool::new(false)),
        });
        app.handle_relocation_destination_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(flag.load(Ordering::Relaxed));
        assert!(sender.send(DestinationMessage::Measured(vec![])).is_err());
        assert_eq!(app.phase, Phase::RelocationSources);
    }

    #[test]
    fn memory_pressure_pauses_inference_but_keeps_drive_measurements() {
        let (_home, mut app) = fixture();
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let inference_cancelled = Arc::new(AtomicBool::new(false));
        app.relocation_advice_worker = Some(DestinationWork {
            receiver,
            cancelled: cancelled.clone(),
            inference_cancelled: inference_cancelled.clone(),
        });
        app.relocation_advice_worker
            .as_ref()
            .unwrap()
            .pause_inference();
        assert!(!cancelled.load(Ordering::Relaxed));
        assert!(inference_cancelled.load(Ordering::Relaxed));
        sender
            .send(DestinationMessage::Measured(vec![drive(
                "Archive", 900_000,
            )]))
            .unwrap();
        app.poll_relocation_advice();
        assert_eq!(app.relocation_destination, "/Volumes/Archive");
        assert!(app.relocation_plan.is_none());
    }

    #[test]
    fn selected_folder_does_not_need_to_be_a_top_inventory_consumer() {
        let (_home, mut app) = fixture();
        let item = app.relocation_source.clone().unwrap();
        app.inventory = None;
        app.open_relocation_for_item(item.clone()).unwrap();
        assert_eq!(app.relocation_source.as_ref().unwrap().path, item.path);
        assert_eq!(app.phase, Phase::RelocationDestination);
        app.relocation_advice_worker = None;
        app.analysis_only = true;
        assert!(app.open_relocation_for_item(item).is_err());
    }

    #[test]
    fn compact_destination_screen_keeps_error_and_confirmation_controls_visible() {
        let (_home, mut app) = fixture();
        app.relocation_destination = "/Volumes/Archive".into();
        app.relocation_destination_error = Some("Not enough external space".into());
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(60, 16)).unwrap();
        terminal
            .draw(|frame| render_relocation_destination(frame, frame.area(), &app))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Not enough external space"));
        assert!(text.contains("Enter validate"));
        assert!(text.contains("Esc back"));
        assert!(text.contains("No files change"));
    }
}
