use std::{
    cmp::Reverse,
    collections::BTreeSet,
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    time::Duration,
};

use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, TableState, Wrap},
};

use crate::{
    cache::{
        CacheEntry, CacheSpec, CacheStatus, CleanupOutcome, CleanupStats, ScanLocation,
        clean_cache, format_kb, free_kb, scan_cache, scan_locations, scan_specs,
        validate_scan_root,
    },
    cli::{Cli, Mode},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Location,
    Scanning,
    Review,
    Confirm,
    Cleaning,
    Summary,
}

struct App {
    mode: Mode,
    account_home: PathBuf,
    scan_root: PathBuf,
    locations: Vec<ScanLocation>,
    location_cursor: usize,
    specs: Vec<CacheSpec>,
    entries: Vec<CacheEntry>,
    phase: Phase,
    include_reinstallable: bool,
    no_color: bool,
    scan_index: usize,
    cursor: usize,
    selected: BTreeSet<usize>,
    cleanup_queue: Vec<usize>,
    cleanup_index: usize,
    stats: CleanupStats,
    free_before_kb: u64,
    stopped_early: bool,
    quit: bool,
}

impl App {
    fn new(cli: &Cli, home: &Path) -> Result<Self, String> {
        let requested_root = cli.volume.as_deref().map(validate_scan_root).transpose()?;
        let mut locations = scan_locations(home);
        if let Some(path) = &requested_root
            && !locations.iter().any(|location| location.path == *path)
        {
            locations.push(ScanLocation {
                label: path
                    .file_name()
                    .filter(|name| !name.is_empty())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Selected volume".into()),
                path: path.clone(),
            });
        }
        let scan_root = requested_root.clone().unwrap_or_else(|| home.to_path_buf());
        let location_cursor = locations
            .iter()
            .position(|location| location.path == scan_root)
            .unwrap_or(0);
        Ok(Self {
            mode: cli.mode(),
            account_home: home.to_path_buf(),
            scan_root: scan_root.clone(),
            locations,
            location_cursor,
            specs: scan_specs(&scan_root, home),
            entries: Vec::new(),
            phase: if requested_root.is_some() {
                Phase::Scanning
            } else {
                Phase::Location
            },
            include_reinstallable: cli.include_reinstallable,
            no_color: cli.no_color || std::env::var_os("NO_COLOR").is_some(),
            scan_index: 0,
            cursor: 0,
            selected: BTreeSet::new(),
            cleanup_queue: Vec::new(),
            cleanup_index: 0,
            stats: CleanupStats::default(),
            free_before_kb: 0,
            stopped_early: false,
            quit: false,
        })
    }

    fn advance_work(&mut self) {
        match self.phase {
            Phase::Scanning => self.scan_next(),
            Phase::Cleaning => self.clean_next(),
            _ => {}
        }
    }

    fn scan_next(&mut self) {
        if let Some(spec) = self.specs.get(self.scan_index) {
            let entry = scan_cache(spec, self.include_reinstallable);
            if entry.size_kb > 0
                || matches!(entry.status, CacheStatus::Symlink | CacheStatus::Invalid)
            {
                self.entries.push(entry);
            }
            self.scan_index += 1;
        } else {
            self.entries.sort_by_key(|entry| Reverse(entry.size_kb));
            self.phase = Phase::Review;
            self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
        }
    }

    fn restart_scan(&mut self) {
        self.entries.clear();
        self.selected.clear();
        self.scan_index = 0;
        self.cursor = 0;
        self.phase = Phase::Scanning;
    }

    fn choose_location(&mut self) {
        let Some(location) = self.locations.get(self.location_cursor) else {
            return;
        };
        self.scan_root = location.path.clone();
        self.specs = scan_specs(&self.scan_root, &self.account_home);
        self.restart_scan();
    }

    fn clean_next(&mut self) {
        let Some(&entry_index) = self.cleanup_queue.get(self.cleanup_index) else {
            self.finish_cleanup();
            return;
        };

        let allowlist: Vec<PathBuf> = self.specs.iter().map(|spec| spec.path.clone()).collect();
        match clean_cache(
            &mut self.entries[entry_index],
            &allowlist,
            self.include_reinstallable,
        ) {
            CleanupOutcome::Cleared(kb) => {
                self.stats.cleared += 1;
                self.stats.measured_removed_kb += kb;
            }
            CleanupOutcome::SafetySkipped(_) => self.stats.safety_skipped += 1,
            CleanupOutcome::Failed(_) => self.stats.failed += 1,
        }
        self.cleanup_index += 1;
    }

    fn finish_cleanup(&mut self) {
        self.stats.filesystem_change_kb =
            free_kb(&self.scan_root).saturating_sub(self.free_before_kb);
        self.phase = Phase::Summary;
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.phase == Phase::Cleaning {
                self.stopped_early = true;
                self.cleanup_queue.truncate(self.cleanup_index);
                self.finish_cleanup();
            } else {
                self.quit = true;
            }
            return;
        }

        match self.phase {
            Phase::Location => self.handle_location_key(key.code),
            Phase::Scanning => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    self.quit = true;
                }
            }
            Phase::Review => self.handle_review_key(key.code),
            Phase::Confirm => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.begin_cleanup(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.phase = Phase::Review;
                }
                _ => {}
            },
            Phase::Cleaning => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    self.stopped_early = true;
                    self.cleanup_queue.truncate(self.cleanup_index);
                    self.finish_cleanup();
                }
            }
            Phase::Summary => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Enter | KeyCode::Esc) {
                    self.quit = true;
                }
            }
        }
    }

    fn handle_location_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.location_cursor = self.location_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.location_cursor =
                    (self.location_cursor + 1).min(self.locations.len().saturating_sub(1));
            }
            KeyCode::Home => self.location_cursor = 0,
            KeyCode::End => self.location_cursor = self.locations.len().saturating_sub(1),
            KeyCode::Enter => self.choose_location(),
            KeyCode::Char('q') | KeyCode::Esc if self.entries.is_empty() => self.quit = true,
            KeyCode::Char('q') | KeyCode::Esc => self.phase = Phase::Review,
            _ => {}
        }
    }

    fn handle_review_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor = self.cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(self.entries.len().saturating_sub(1));
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.entries.len().saturating_sub(1),
            KeyCode::Char(' ') if self.mode == Mode::Clean => self.toggle_current(),
            KeyCode::Char('a') if self.mode == Mode::Clean => self.toggle_all(),
            KeyCode::Char('i') => {
                self.include_reinstallable = !self.include_reinstallable;
                self.restart_scan();
            }
            KeyCode::Char('r') => self.restart_scan(),
            KeyCode::Char('v') => self.phase = Phase::Location,
            KeyCode::Enter if self.mode == Mode::Clean && !self.selected.is_empty() => {
                self.phase = Phase::Confirm;
            }
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter if self.mode == Mode::Analyze => {
                self.quit = true;
            }
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            _ => {}
        }
    }

    fn toggle_current(&mut self) {
        if !self.is_selectable(self.cursor) {
            return;
        }
        if !self.selected.remove(&self.cursor) {
            self.selected.insert(self.cursor);
        }
    }

    fn toggle_all(&mut self) {
        let selectable: Vec<usize> = (0..self.entries.len())
            .filter(|&index| self.is_selectable(index))
            .collect();
        if !selectable.is_empty() && selectable.iter().all(|index| self.selected.contains(index)) {
            self.selected.clear();
        } else {
            self.selected.extend(selectable);
        }
    }

    fn is_selectable(&self, index: usize) -> bool {
        self.entries.get(index).is_some_and(|entry| {
            entry.status == CacheStatus::Ready && entry.size_kb > 0 && entry.outcome.is_none()
        })
    }

    fn begin_cleanup(&mut self) {
        self.cleanup_queue = self.selected.iter().copied().collect();
        self.cleanup_index = 0;
        self.stats = CleanupStats::default();
        self.free_before_kb = free_kb(&self.scan_root);
        self.phase = Phase::Cleaning;
    }

    fn found_kb(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size_kb).sum()
    }

    fn ready_kb(&self) -> u64 {
        self.entries
            .iter()
            .filter(|entry| entry.status == CacheStatus::Ready)
            .map(|entry| entry.size_kb)
            .sum()
    }

    fn selected_kb(&self) -> u64 {
        self.selected
            .iter()
            .filter_map(|index| self.entries.get(*index))
            .map(|entry| entry.size_kb)
            .sum()
    }

    fn optional_kb(&self) -> u64 {
        self.entries
            .iter()
            .filter(|entry| entry.status == CacheStatus::Optional)
            .map(|entry| entry.size_kb)
            .sum()
    }

    fn color(&self, color: Color) -> Color {
        if self.no_color { Color::Reset } else { color }
    }
}

pub fn can_run() -> bool {
    io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var("TERM").is_ok_and(|term| term != "dumb")
}

pub fn run(cli: &Cli, home: &Path) -> Result<i32, String> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|error| format!("could not enable terminal raw mode: {error}"))?;
    if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
        let _ = disable_raw_mode();
        return Err(format!(
            "could not enter the alternate terminal screen: {error}"
        ));
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = disable_raw_mode();
            let mut fallback = io::stdout();
            let _ = execute!(fallback, Show, LeaveAlternateScreen);
            return Err(format!("could not initialize the terminal: {error}"));
        }
    };

    let app = match App::new(cli, home) {
        Ok(app) => app,
        Err(error) => {
            let _ = disable_raw_mode();
            let _ = execute!(terminal.backend_mut(), Show, LeaveAlternateScreen);
            let _ = terminal.show_cursor();
            return Err(error);
        }
    };
    let result = run_loop(&mut terminal, app);
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), Show, LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut app: App,
) -> Result<i32, String> {
    while !app.quit {
        terminal
            .draw(|frame| render(frame, &app))
            .map_err(|error| format!("could not draw the interface: {error}"))?;

        if matches!(app.phase, Phase::Scanning | Phase::Cleaning) {
            if event::poll(Duration::ZERO).map_err(|error| error.to_string())?
                && let Event::Key(key) = event::read().map_err(|error| error.to_string())?
            {
                app.handle_key(key);
            }
            if !app.quit {
                app.advance_work();
            }
        } else if event::poll(Duration::from_millis(250)).map_err(|error| error.to_string())?
            && let Event::Key(key) = event::read().map_err(|error| error.to_string())?
        {
            app.handle_key(key);
        }
    }
    Ok(i32::from(app.stats.failed > 0))
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    if app.phase == Phase::Location {
        render_location_picker(frame, area, app);
        return;
    }
    let compact = area.height < 25;
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if compact {
            vec![
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(7),
                Constraint::Length(3),
            ]
        } else {
            vec![
                Constraint::Length(3),
                Constraint::Length(5),
                Constraint::Min(8),
                Constraint::Length(6),
                Constraint::Length(3),
            ]
        })
        .split(area);

    render_header(frame, sections[0], app);
    render_metrics(frame, sections[1], app);
    render_table(frame, sections[2], app);
    if compact {
        render_footer(frame, sections[3], app);
    } else {
        render_details(frame, sections[3], app);
        render_footer(frame, sections[4], app);
    }

    if app.phase == Phase::Confirm {
        render_confirmation(frame, area, app);
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mode = match app.mode {
        Mode::Analyze => "ANALYZE",
        Mode::Clean => "CLEAN",
    };
    let title_style = Style::default()
        .fg(app.color(Color::Cyan))
        .add_modifier(Modifier::BOLD);
    let content = Line::from(vec![
        Span::styled(format!(" {mode} "), title_style),
        Span::raw("  Find removable caches, trash, and temporary data  •  "),
        Span::styled(
            app.scan_root.display().to_string(),
            Style::default().fg(app.color(Color::DarkGray)),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(content)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Span::styled(" MAC CLEANUP ", title_style)),
            )
            .alignment(Alignment::Left),
        area,
    );
}

fn render_metrics(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let metrics = [
        ("WASTE FOUND", format_kb(app.found_kb()), Color::White),
        ("REMOVABLE", format_kb(app.ready_kb()), Color::Green),
        ("SELECTED", format_kb(app.selected_kb()), Color::Cyan),
        ("OPT-IN", format_kb(app.optional_kb()), Color::Yellow),
    ];
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 4); 4])
        .split(area);
    for ((label, value, color), column) in metrics.into_iter().zip(columns.iter()) {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" {label} "),
                    Style::default().fg(app.color(Color::DarkGray)),
                ),
                Span::styled(
                    value,
                    Style::default()
                        .fg(app.color(color))
                        .add_modifier(Modifier::BOLD),
                ),
            ]))
            .block(Block::default().borders(Borders::ALL)),
            *column,
        );
    }
}

fn render_table(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.phase == Phase::Review && app.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from("No removable data was found in known waste locations."),
                Line::from(Span::styled(
                    "Try another disk, rescan, or enable reinstallable downloads.",
                    Style::default().fg(app.color(Color::DarkGray)),
                )),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" WASTE FOUND "),
            )
            .alignment(Alignment::Center),
            area,
        );
        return;
    }

    let header = Row::new(["", "STATUS", "ITEM", "SIZE"])
        .style(
            Style::default()
                .fg(app.color(Color::DarkGray))
                .add_modifier(Modifier::BOLD),
        )
        .height(1);
    let rows = app.entries.iter().enumerate().map(|(index, entry)| {
        let marker = if app.mode == Mode::Clean && app.is_selectable(index) {
            if app.selected.contains(&index) {
                "[×]"
            } else {
                "[ ]"
            }
        } else {
            " · "
        };
        let (status, color) = match &entry.outcome {
            Some(CleanupOutcome::Cleared(_)) => ("CLEARED", Color::Green),
            Some(CleanupOutcome::SafetySkipped(_)) => ("SKIPPED", Color::Yellow),
            Some(CleanupOutcome::Failed(_)) => ("FAILED", Color::Red),
            None => (
                entry.status.label(),
                match entry.status {
                    CacheStatus::Ready => Color::Green,
                    CacheStatus::Optional | CacheStatus::InUse => Color::Yellow,
                    CacheStatus::Symlink | CacheStatus::Invalid => Color::Red,
                    CacheStatus::Missing => Color::DarkGray,
                },
            ),
        };
        Row::new(vec![
            Cell::from(marker),
            Cell::from(status).style(Style::default().fg(app.color(color))),
            Cell::from(entry.spec.label),
            Cell::from(format_kb(entry.size_kb)).style(Style::default().fg(app.color(Color::Gray))),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(10),
            Constraint::Fill(1),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" WASTE FOUND "),
    )
    .row_highlight_style(
        Style::default()
            .bg(app.color(Color::DarkGray))
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("›");
    let mut state = TableState::default().with_selected(Some(app.cursor));
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_details(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let lines = if let Some(entry) = app.entries.get(app.cursor) {
        let outcome = match &entry.outcome {
            Some(CleanupOutcome::Cleared(kb)) => {
                format!("Cleared; reclaimed about {}.", format_kb(*kb))
            }
            Some(CleanupOutcome::SafetySkipped(reason)) => format!("Skipped: {reason}."),
            Some(CleanupOutcome::Failed(error)) => format!("Failed: {error}"),
            None => entry.status.explanation().to_string(),
        };
        vec![
            Line::from(vec![
                Span::styled("Path  ", Style::default().fg(app.color(Color::DarkGray))),
                Span::raw(entry.spec.path.display().to_string()),
            ]),
            Line::from(vec![
                Span::styled("Effect  ", Style::default().fg(app.color(Color::DarkGray))),
                Span::raw(entry.spec.note),
            ]),
            Line::from(vec![
                Span::styled("Safety  ", Style::default().fg(app.color(Color::DarkGray))),
                Span::raw(outcome),
            ]),
        ]
    } else if app.phase == Phase::Scanning {
        vec![Line::from("Scanning known waste locations…")]
    } else {
        vec![Line::from(
            "Nothing removable was found at this location. No files were changed.",
        )]
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" DETAILS "))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL);
    match app.phase {
        Phase::Location => {}
        Phase::Scanning => {
            let total = app.specs.len().max(1);
            frame.render_widget(
                Gauge::default()
                    .block(block.title(" SCANNING • large folders may take a moment "))
                    .gauge_style(
                        Style::default()
                            .fg(app.color(Color::Cyan))
                            .add_modifier(Modifier::BOLD),
                    )
                    .ratio(app.scan_index as f64 / total as f64)
                    .label(format!("{} / {}", app.scan_index, app.specs.len())),
                area,
            );
        }
        Phase::Review => {
            let help = if app.mode == Mode::Clean {
                "↑↓/jk move   space select   a all   i reinstallables   v volume   r rescan   enter continue   q quit"
            } else {
                "↑↓/jk inspect   i reinstallables   v volume   r rescan   enter/q close"
            };
            frame.render_widget(
                Paragraph::new(help)
                    .block(block.title(" KEYS "))
                    .alignment(Alignment::Center),
                area,
            );
        }
        Phase::Confirm => {
            frame.render_widget(
                Paragraph::new("Confirm or cancel in the dialog")
                    .block(block.title(" CONFIRMATION "))
                    .alignment(Alignment::Center),
                area,
            );
        }
        Phase::Cleaning => {
            let total = app.cleanup_queue.len().max(1);
            frame.render_widget(
                Gauge::default()
                    .block(block.title(" CLEANING • q stops after the current item "))
                    .gauge_style(
                        Style::default()
                            .fg(app.color(Color::Green))
                            .add_modifier(Modifier::BOLD),
                    )
                    .ratio(app.cleanup_index as f64 / total as f64)
                    .label(format!(
                        "{} / {}",
                        app.cleanup_index,
                        app.cleanup_queue.len()
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
            let summary = format!(
                "{status}  •  removed {}  •  disk change {}  •  cleared {}  •  skipped {}  •  failed {}  •  enter/q close",
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

fn render_location_picker(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let outer = Block::default().borders(Borders::ALL).title(Span::styled(
        " SELECT SCAN LOCATION ",
        Style::default()
            .fg(app.color(Color::Cyan))
            .add_modifier(Modifier::BOLD),
    ));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("Choose a disk or home directory to search for removable data."),
            Line::from(Span::styled(
                "The scan checks known caches, Trash/recycle bins, and temporary folders; personal files are excluded.",
                Style::default().fg(app.color(Color::DarkGray)),
            )),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        sections[0],
    );

    let rows = app.locations.iter().map(|location| {
        Row::new(vec![
            Cell::from(location.label.clone()),
            Cell::from(location.path.display().to_string())
                .style(Style::default().fg(app.color(Color::Gray))),
        ])
    });
    let table = Table::new(
        rows,
        [Constraint::Percentage(30), Constraint::Percentage(70)],
    )
    .header(
        Row::new(["LOCATION", "PATH"]).style(
            Style::default()
                .fg(app.color(Color::DarkGray))
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(Block::default().borders(Borders::ALL).title(" AVAILABLE "))
    .row_highlight_style(
        Style::default()
            .bg(app.color(Color::DarkGray))
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("›");
    let mut state = TableState::default().with_selected(Some(app.location_cursor));
    frame.render_stateful_widget(table, sections[1], &mut state);

    let close = if app.entries.is_empty() {
        "quit"
    } else {
        "cancel"
    };
    frame.render_widget(
        Paragraph::new(format!(
            "↑↓/jk choose   enter scan   esc/q {close}   •   use --volume PATH for automation"
        ))
        .block(Block::default().borders(Borders::ALL).title(" KEYS "))
        .alignment(Alignment::Center),
        sections[2],
    );
}

fn render_confirmation(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let popup = centered_rect(62, 9, area);
    frame.render_widget(Clear, popup);
    let count = app.selected.len();
    let amount = format_kb(app.selected_kb());
    let text = vec![
        Line::from(Span::styled(
            "PERMANENTLY DELETE CACHE CONTENTS?",
            Style::default()
                .fg(app.color(Color::Red))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!(
            "Clear {count} selected item(s), totaling about {amount}?"
        )),
        Line::from("The containing folders remain, but deleted contents cannot be restored."),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                " y ",
                Style::default()
                    .fg(app.color(Color::Black))
                    .bg(app.color(Color::Green))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" clear selected     "),
            Span::styled(" n / esc ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" cancel"),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(app.color(Color::Red)))
                    .title(" CONFIRM CLEANUP "),
            )
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let width = area.width.saturating_mul(percent_x).saturating_div(100);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height.min(area.height)) / 2,
        width,
        height.min(area.height),
    )
    .inner(Margin {
        horizontal: 0,
        vertical: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_only_includes_ready_nonempty_entries() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: false,
            clean: true,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            no_color: true,
            no_tui: false,
        };
        let mut app = App::new(&cli, temp.path()).unwrap();
        app.entries = vec![CacheEntry {
            spec: app.specs[0].clone(),
            status: CacheStatus::Ready,
            size_kb: 10,
            outcome: None,
        }];
        assert!(app.is_selectable(0));
        app.entries[0].size_kb = 0;
        assert!(!app.is_selectable(0));
        app.entries[0].size_kb = 10;
        app.entries[0].status = CacheStatus::InUse;
        assert!(!app.is_selectable(0));
    }

    #[test]
    fn completed_scan_hides_missing_candidates() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: true,
            clean: false,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            no_color: true,
            no_tui: false,
        };
        let mut app = App::new(&cli, temp.path()).unwrap();
        app.choose_location();

        while app.phase == Phase::Scanning {
            app.scan_next();
        }

        assert_eq!(app.phase, Phase::Review);
        assert!(app.entries.is_empty());
    }
}
