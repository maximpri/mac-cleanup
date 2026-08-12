use std::{
    cmp::Reverse,
    collections::{BTreeSet, HashSet},
    fs,
    io::{self, IsTerminal},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
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
        clean_cache, clean_review_data, format_kb, free_kb, scan_location_kind, scan_locations,
        scan_specs, status_for, validate_scan_root,
    },
    cli::{Cli, Mode},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Location,
    Scanning,
    Review,
    Details,
    Confirm,
    ReviewConfirm,
    Cleaning,
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanupKind {
    Cache,
    ReviewData,
}

const SUMMARY_AUTO_CLOSE_AFTER: Duration = Duration::from_secs(5);
const OPEN_COMMAND: &str = "/usr/bin/open";

struct DirectoryScan {
    spec: CacheSpec,
    status: CacheStatus,
    readers: Vec<fs::ReadDir>,
    seen: HashSet<(u64, u64)>,
    allocated_blocks: u64,
    inspected_items: u64,
    errors: u64,
    started_at: Instant,
}

impl DirectoryScan {
    fn new(spec: CacheSpec, status: CacheStatus) -> Self {
        let mut seen = HashSet::new();
        let mut allocated_blocks = 0;
        let mut errors = 0;
        match fs::symlink_metadata(&spec.path) {
            Ok(metadata) => {
                seen.insert((metadata.dev(), metadata.ino()));
                allocated_blocks = metadata.blocks();
            }
            Err(_) => errors += 1,
        }
        let readers = match fs::read_dir(&spec.path) {
            Ok(reader) => vec![reader],
            Err(_) => {
                errors += 1;
                Vec::new()
            }
        };
        Self {
            spec,
            status,
            readers,
            seen,
            allocated_blocks,
            inspected_items: 0,
            errors,
            started_at: Instant::now(),
        }
    }

    /// Process a bounded slice of filesystem work so drawing and keyboard
    /// handling get a chance to run between slices.
    fn advance(&mut self) -> bool {
        const MAX_ITEMS_PER_TICK: usize = 256;
        const MAX_TICK_TIME: Duration = Duration::from_millis(8);
        let tick_started = Instant::now();

        for _ in 0..MAX_ITEMS_PER_TICK {
            if tick_started.elapsed() >= MAX_TICK_TIME {
                break;
            }
            let Some(reader) = self.readers.last_mut() else {
                return true;
            };
            let entry = match reader.next() {
                Some(Ok(entry)) => entry,
                Some(Err(_)) => {
                    self.errors += 1;
                    continue;
                }
                None => {
                    self.readers.pop();
                    continue;
                }
            };

            self.inspected_items += 1;
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(_) => {
                    self.errors += 1;
                    continue;
                }
            };
            let first_visit = self.seen.insert((metadata.dev(), metadata.ino()));
            if first_visit {
                self.allocated_blocks = self.allocated_blocks.saturating_add(metadata.blocks());
            }
            if first_visit && metadata.is_dir() && !metadata.file_type().is_symlink() {
                match fs::read_dir(entry.path()) {
                    Ok(reader) => self.readers.push(reader),
                    Err(_) => self.errors += 1,
                }
            }
        }
        self.readers.is_empty()
    }

    fn size_kb(&self) -> u64 {
        self.allocated_blocks.saturating_add(1) / 2
    }

    fn into_entry(self) -> CacheEntry {
        let size_kb = self.size_kb();
        CacheEntry {
            spec: self.spec,
            status: if self.errors == 0 {
                self.status
            } else {
                CacheStatus::ScanError
            },
            size_kb,
            outcome: None,
        }
    }
}

struct App {
    mode: Mode,
    analysis_only: bool,
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
    scan_task: Option<DirectoryScan>,
    scan_started_at: Instant,
    cursor: usize,
    selected: BTreeSet<usize>,
    cleanup_queue: Vec<usize>,
    cleanup_index: usize,
    cleanup_kind: CleanupKind,
    review_target: Option<usize>,
    review_confirmation: String,
    review_confirmation_error: bool,
    stats: CleanupStats,
    free_before_kb: u64,
    stopped_early: bool,
    summary_started_at: Option<Instant>,
    status_message: Option<String>,
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
                kind: scan_location_kind(path),
            });
        }
        let scan_root = requested_root.clone().unwrap_or_else(|| home.to_path_buf());
        let location_cursor = requested_root
            .as_ref()
            .and_then(|requested| {
                locations
                    .iter()
                    .position(|location| location.path == *requested)
            })
            .or_else(|| {
                locations
                    .iter()
                    .position(|location| location.path == Path::new("/"))
            })
            .unwrap_or(0);
        Ok(Self {
            mode: cli.mode(),
            analysis_only: cli.analyze,
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
            scan_task: None,
            scan_started_at: Instant::now(),
            cursor: 0,
            selected: BTreeSet::new(),
            cleanup_queue: Vec::new(),
            cleanup_index: 0,
            cleanup_kind: CleanupKind::Cache,
            review_target: None,
            review_confirmation: String::new(),
            review_confirmation_error: false,
            stats: CleanupStats::default(),
            free_before_kb: 0,
            stopped_early: false,
            summary_started_at: None,
            status_message: None,
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
        if let Some(task) = self.scan_task.as_mut() {
            if task.advance() {
                let entry = self
                    .scan_task
                    .take()
                    .expect("active scan task")
                    .into_entry();
                self.record_scan_entry(entry);
                self.scan_index += 1;
            }
            return;
        }

        let Some(spec) = self.specs.get(self.scan_index).cloned() else {
            self.entries.sort_by_key(|entry| Reverse(entry.size_kb));
            self.phase = Phase::Review;
            self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
            return;
        };

        let status = status_for(&spec, self.include_reinstallable);
        if matches!(
            status,
            CacheStatus::Missing
                | CacheStatus::ScanError
                | CacheStatus::Symlink
                | CacheStatus::Invalid
        ) {
            self.record_scan_entry(CacheEntry {
                spec,
                status,
                size_kb: 0,
                outcome: None,
            });
            self.scan_index += 1;
        } else {
            self.scan_task = Some(DirectoryScan::new(spec, status));
        }
    }

    fn record_scan_entry(&mut self, entry: CacheEntry) {
        if entry.size_kb > 0
            || matches!(
                entry.status,
                CacheStatus::ScanError | CacheStatus::Symlink | CacheStatus::Invalid
            )
        {
            self.entries.push(entry);
        }
    }

    fn restart_scan(&mut self) {
        self.entries.clear();
        self.selected.clear();
        self.scan_index = 0;
        self.scan_task = None;
        self.scan_started_at = Instant::now();
        self.cursor = 0;
        self.status_message = None;
        self.phase = Phase::Scanning;
    }

    fn cancel_scan(&mut self) {
        self.scan_task = None;
        self.entries.clear();
        self.selected.clear();
        self.scan_index = 0;
        self.cursor = 0;
        self.phase = Phase::Location;
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
        let outcome = match self.cleanup_kind {
            CleanupKind::Cache => clean_cache(
                &mut self.entries[entry_index],
                &allowlist,
                self.include_reinstallable,
            ),
            CleanupKind::ReviewData => {
                clean_review_data(&mut self.entries[entry_index], &allowlist)
            }
        };
        match outcome {
            CleanupOutcome::Cleared(kb) => {
                self.stats.cleared += 1;
                self.stats.measured_removed_kb += kb;
            }
            CleanupOutcome::SafetySkipped(_) => self.stats.safety_skipped += 1,
            CleanupOutcome::Failed { removed_kb, .. } => {
                self.stats.failed += 1;
                self.stats.measured_removed_kb += removed_kb;
            }
        }
        self.cleanup_index += 1;
    }

    fn finish_cleanup(&mut self) {
        self.stats.filesystem_change_kb =
            free_kb(&self.scan_root).saturating_sub(self.free_before_kb);
        self.summary_started_at = Some(Instant::now());
        self.phase = Phase::Summary;
    }

    fn summary_remaining(&self) -> Option<Duration> {
        self.summary_started_at
            .map(|started_at| SUMMARY_AUTO_CLOSE_AFTER.saturating_sub(started_at.elapsed()))
    }

    fn summary_has_timed_out(&self) -> bool {
        self.phase == Phase::Summary
            && self
                .summary_remaining()
                .is_some_and(|remaining| remaining.is_zero())
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
            Phase::Scanning => match key.code {
                KeyCode::Esc => self.cancel_scan(),
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            },
            Phase::Review => self.handle_review_key(key.code),
            Phase::Details => match key.code {
                KeyCode::Char('d') | KeyCode::Char('D')
                    if !self.analysis_only && self.current_is_review_data() =>
                {
                    self.mode = Mode::Clean;
                    self.start_review_confirmation();
                }
                KeyCode::Char('c') | KeyCode::Char('C') if !self.analysis_only => {
                    self.prepare_safe_cleanup();
                }
                KeyCode::Char('o') | KeyCode::Char('O') => self.reveal_current(),
                KeyCode::Enter | KeyCode::Esc => self.phase = Phase::Review,
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            },
            Phase::Confirm => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.begin_cleanup(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.phase = Phase::Review;
                }
                _ => {}
            },
            Phase::ReviewConfirm => self.handle_review_confirmation_key(key),
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
                self.status_message = None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(self.entries.len().saturating_sub(1));
                self.status_message = None;
            }
            KeyCode::Home => {
                self.cursor = 0;
                self.status_message = None;
            }
            KeyCode::End => {
                self.cursor = self.entries.len().saturating_sub(1);
                self.status_message = None;
            }
            KeyCode::Char(' ') if self.mode == Mode::Clean => self.toggle_current(),
            KeyCode::Char('a') if self.mode == Mode::Clean => self.toggle_all(),
            KeyCode::Char('d') | KeyCode::Char('D')
                if !self.analysis_only && self.current_is_review_data() =>
            {
                self.mode = Mode::Clean;
                self.start_review_confirmation();
            }
            KeyCode::Char('c') | KeyCode::Char('C') if !self.analysis_only => {
                self.prepare_safe_cleanup();
            }
            KeyCode::Char('i') => {
                self.include_reinstallable = !self.include_reinstallable;
                self.restart_scan();
            }
            KeyCode::Char('o') | KeyCode::Char('O') => self.reveal_current(),
            KeyCode::Char('r') => self.restart_scan(),
            KeyCode::Char('v') => self.phase = Phase::Location,
            KeyCode::Enter if self.mode == Mode::Clean && !self.selected.is_empty() => {
                self.phase = Phase::Confirm;
            }
            KeyCode::Enter if self.entries.get(self.cursor).is_some() => {
                self.phase = Phase::Details;
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

    fn prepare_safe_cleanup(&mut self) {
        self.mode = Mode::Clean;
        self.selected.clear();
        let selectable: Vec<usize> = (0..self.entries.len())
            .filter(|&index| self.is_selectable(index))
            .collect();
        self.selected.extend(selectable);
        self.phase = if self.selected.is_empty() {
            Phase::Review
        } else {
            Phase::Confirm
        };
    }

    fn is_selectable(&self, index: usize) -> bool {
        self.entries.get(index).is_some_and(|entry| {
            entry.status == CacheStatus::Ready && entry.size_kb > 0 && entry.outcome.is_none()
        })
    }

    fn current_is_review_data(&self) -> bool {
        self.entries.get(self.cursor).is_some_and(|entry| {
            entry.status == CacheStatus::Review && entry.size_kb > 0 && entry.outcome.is_none()
        })
    }

    fn start_review_confirmation(&mut self) {
        if !self.current_is_review_data() {
            return;
        }
        self.review_target = Some(self.cursor);
        self.review_confirmation.clear();
        self.review_confirmation_error = false;
        self.phase = Phase::ReviewConfirm;
    }

    fn reveal_current(&mut self) {
        let Some(entry) = self.entries.get(self.cursor) else {
            return;
        };
        let path = entry.spec.path.clone();
        self.status_message = Some(
            match Command::new(OPEN_COMMAND)
                .arg("-R")
                .arg(&path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                Ok(status) if status.success() => {
                    format!("Revealed: {}", path.display())
                }
                Ok(status) => format!("Finder could not reveal the path (open exited {status})."),
                Err(error) => format!("Finder could not be opened: {error}"),
            },
        );
    }

    fn handle_review_confirmation_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.review_target = None;
                self.review_confirmation.clear();
                self.review_confirmation_error = false;
                self.phase = Phase::Review;
            }
            KeyCode::Backspace => {
                self.review_confirmation.pop();
                self.review_confirmation_error = false;
            }
            KeyCode::Enter if self.review_confirmation == "DELETE" => {
                self.begin_review_cleanup();
            }
            KeyCode::Enter => self.review_confirmation_error = true,
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.review_confirmation.len() < "DELETE".len() =>
            {
                self.review_confirmation
                    .push(character.to_ascii_uppercase());
                self.review_confirmation_error = false;
            }
            _ => {}
        }
    }

    fn begin_cleanup(&mut self) {
        self.cleanup_queue = self.selected.iter().copied().collect();
        self.cleanup_index = 0;
        self.cleanup_kind = CleanupKind::Cache;
        self.stats = CleanupStats::default();
        self.summary_started_at = None;
        self.free_before_kb = free_kb(&self.scan_root);
        self.phase = Phase::Cleaning;
    }

    fn begin_review_cleanup(&mut self) {
        let Some(target) = self.review_target.take() else {
            self.phase = Phase::Review;
            return;
        };
        if !self.entries.get(target).is_some_and(|entry| {
            entry.status == CacheStatus::Review && entry.size_kb > 0 && entry.outcome.is_none()
        }) {
            self.phase = Phase::Review;
            return;
        }
        self.cleanup_queue = vec![target];
        self.cleanup_index = 0;
        self.cleanup_kind = CleanupKind::ReviewData;
        self.review_confirmation.clear();
        self.review_confirmation_error = false;
        self.stats = CleanupStats::default();
        self.summary_started_at = None;
        self.free_before_kb = free_kb(&self.scan_root);
        self.phase = Phase::Cleaning;
    }

    fn identified_kb(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size_kb).sum()
    }

    fn current_location_kind(&self) -> crate::cache::ScanLocationKind {
        self.locations
            .iter()
            .find(|location| location.path == self.scan_root)
            .map_or(crate::cache::ScanLocationKind::Local, |location| {
                location.kind
            })
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

    fn review_kb(&self) -> u64 {
        self.entries
            .iter()
            .filter(|entry| entry.status == CacheStatus::Review)
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

        if !app.quit && app.summary_has_timed_out() {
            app.quit = true;
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
    } else if app.phase == Phase::ReviewConfirm {
        render_review_confirmation(frame, area, app);
    } else if app.phase == Phase::Details {
        render_entry_details(frame, area, app);
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
        Span::raw("  Find reclaimable space and app-managed storage  •  "),
        Span::styled(
            format!("{}  •  ", app.current_location_kind().label()),
            Style::default().fg(app.color(Color::DarkGray)),
        ),
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
    let metrics = if app.mode == Mode::Clean {
        [
            ("CLEANABLE", format_kb(app.ready_kb()), Color::Green),
            ("SELECTED", format_kb(app.selected_kb()), Color::Cyan),
            ("OPT-IN", format_kb(app.optional_kb()), Color::Yellow),
            ("REVIEW", format_kb(app.review_kb()), Color::Magenta),
        ]
    } else {
        [
            ("IDENTIFIED", format_kb(app.identified_kb()), Color::White),
            ("CLEANABLE", format_kb(app.ready_kb()), Color::Green),
            ("OPT-IN", format_kb(app.optional_kb()), Color::Yellow),
            ("REVIEW", format_kb(app.review_kb()), Color::Magenta),
        ]
    };
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
                Line::from("No reclaimable or app-managed storage was found."),
                Line::from(Span::styled(
                    "Try another disk, rescan, or enable reinstallable downloads.",
                    Style::default().fg(app.color(Color::DarkGray)),
                )),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" STORAGE FOUND "),
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
            Some(CleanupOutcome::Cleared(_)) => ("CLEARED", Color::Green),
            Some(CleanupOutcome::SafetySkipped(_)) => ("SKIPPED", Color::Yellow),
            Some(CleanupOutcome::Failed { .. }) => ("FAILED", Color::Red),
            None => (
                entry.status.label(),
                match entry.status {
                    CacheStatus::Ready => Color::Green,
                    CacheStatus::Optional | CacheStatus::InUse => Color::Yellow,
                    CacheStatus::Review => Color::Magenta,
                    CacheStatus::ScanError | CacheStatus::Symlink | CacheStatus::Invalid => {
                        Color::Red
                    }
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
            .title(" STORAGE FOUND "),
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
    let lines = if app.phase == Phase::Scanning {
        if let Some(task) = &app.scan_task {
            let access = if task.errors == 0 {
                "Reading allocated filesystem blocks".to_string()
            } else {
                format!("Skipped {} unreadable item(s)", task.errors)
            };
            vec![
                Line::from(vec![
                    Span::styled("Current  ", Style::default().fg(app.color(Color::DarkGray))),
                    Span::styled(
                        task.spec.label,
                        Style::default()
                            .fg(app.color(Color::Cyan))
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Path     ", Style::default().fg(app.color(Color::DarkGray))),
                    Span::raw(task.spec.path.display().to_string()),
                ]),
                Line::from(vec![
                    Span::styled("Progress ", Style::default().fg(app.color(Color::DarkGray))),
                    Span::raw(format!(
                        "{} items inspected • {} found • {} elapsed",
                        task.inspected_items,
                        format_kb(task.size_kb()),
                        format_elapsed(task.started_at.elapsed()),
                    )),
                ]),
                Line::from(vec![
                    Span::styled("Access   ", Style::default().fg(app.color(Color::DarkGray))),
                    Span::raw(access),
                ]),
            ]
        } else {
            let next = app
                .specs
                .get(app.scan_index)
                .map(|spec| format!("Preparing {}…", spec.label))
                .unwrap_or_else(|| "Finishing scan…".into());
            vec![Line::from(next)]
        }
    } else if let Some(entry) = app.entries.get(app.cursor) {
        let outcome = match &entry.outcome {
            Some(CleanupOutcome::Cleared(kb)) => {
                format!("Cleared; reclaimed about {}.", format_kb(*kb))
            }
            Some(CleanupOutcome::SafetySkipped(reason)) => format!("Skipped: {reason}."),
            Some(CleanupOutcome::Failed { error, removed_kb }) if *removed_kb > 0 => format!(
                "Failed after removing about {}: {error}",
                format_kb(*removed_kb)
            ),
            Some(CleanupOutcome::Failed { error, .. }) => format!("Failed: {error}"),
            None => entry.status.explanation().to_string(),
        };
        let mut lines = vec![
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
        ];
        if let Some(message) = &app.status_message {
            lines.push(Line::from(vec![
                Span::styled("Finder  ", Style::default().fg(app.color(Color::DarkGray))),
                Span::raw(message),
            ]));
        }
        lines
    } else {
        vec![Line::from(
            "No reclaimable or app-managed storage was found. No files were changed.",
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
            let current =
                (app.scan_index + usize::from(app.scan_index < app.specs.len())).min(total);
            let progress = if let Some(task) = &app.scan_task {
                format!(
                    "{}  {current}/{total} • {} • {} items • {} • {}",
                    scan_spinner(task.started_at.elapsed()),
                    task.spec.label,
                    task.inspected_items,
                    format_kb(task.size_kb()),
                    format_elapsed(app.scan_started_at.elapsed()),
                )
            } else {
                format!(
                    "{}  {current}/{total} • preparing next location • {}",
                    scan_spinner(app.scan_started_at.elapsed()),
                    format_elapsed(app.scan_started_at.elapsed())
                )
            };
            let ratio = smooth_scan_ratio(
                app.scan_index,
                total,
                app.scan_task.as_ref().map(|task| task.started_at.elapsed()),
            );
            frame.render_widget(
                Gauge::default()
                    .block(block.title(" SCANNING • live progress • Esc cancel • q quit "))
                    .gauge_style(
                        Style::default()
                            .fg(app.color(Color::Cyan))
                            .add_modifier(Modifier::BOLD),
                    )
                    .ratio(ratio)
                    .label(progress),
                area,
            );
        }
        Phase::Review => {
            let help = if app.analysis_only {
                "↑↓/jk move   enter details   o finder   i reinstallables   v volume   r rescan   q quit"
            } else if app.mode == Mode::Clean {
                "↑↓/jk move   space select   a all safe   c clean all safe   d delete REVIEW   enter details/continue   o finder   i opt-in   v volume   r rescan   q quit"
            } else if app.current_is_review_data() {
                "d delete this REVIEW   c clean all safe   enter details   o finder   i opt-in   v volume   r rescan   q quit"
            } else {
                "c clean all safe   enter details   o finder   i opt-in   v volume   r rescan   q quit"
            };
            frame.render_widget(
                Paragraph::new(help)
                    .block(block.title(" KEYS "))
                    .alignment(Alignment::Center),
                area,
            );
        }
        Phase::Details => {
            frame.render_widget(
                Paragraph::new("Viewing the highlighted finding")
                    .block(block.title(" DETAILS "))
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
        Phase::ReviewConfirm => {
            frame.render_widget(
                Paragraph::new("Type DELETE to confirm this one review item, or Esc to cancel")
                    .block(block.title(" ADVANCED REVIEW DELETION "))
                    .alignment(Alignment::Center),
                area,
            );
        }
        Phase::Cleaning => {
            let total = app.cleanup_queue.len().max(1);
            let title = match app.cleanup_kind {
                CleanupKind::Cache => " CLEANING • Esc/q stops after the current item ",
                CleanupKind::ReviewData => " DELETING REVIEW DATA • permanent removal in progress ",
            };
            frame.render_widget(
                Gauge::default()
                    .block(block.title(title))
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
            Line::from(
                "Choose a local, USB, or network location to search for reclaimable space.",
            ),
            Line::from(Span::styled(
                "The local startup volume is selected by default. App-managed data requires a separate typed confirmation.",
                Style::default().fg(app.color(Color::DarkGray)),
            )),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        sections[0],
    );

    let rows = app.locations.iter().map(|location| {
        Row::new(vec![
            Cell::from(location.kind.label()).style(match location.kind {
                crate::cache::ScanLocationKind::Local => {
                    Style::default().fg(app.color(Color::Green))
                }
                crate::cache::ScanLocationKind::Usb => Style::default().fg(app.color(Color::Cyan)),
                crate::cache::ScanLocationKind::Network => {
                    Style::default().fg(app.color(Color::Magenta))
                }
            }),
            Cell::from(location.label.clone()),
            Cell::from(location.path.display().to_string())
                .style(Style::default().fg(app.color(Color::Gray))),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(9),
            Constraint::Percentage(28),
            Constraint::Percentage(72),
        ],
    )
    .header(
        Row::new(["TYPE", "LOCATION", "PATH"]).style(
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
    let help = if area.width < 72 {
        format!("↑↓/jk choose   enter scan   esc/q {close}")
    } else {
        format!("↑↓/jk choose   enter scan   esc/q {close}   •   use --volume PATH for automation")
    };
    frame.render_widget(
        Paragraph::new(help)
            .block(Block::default().borders(Borders::ALL).title(" KEYS "))
            .alignment(Alignment::Center),
        sections[2],
    );
}

fn render_entry_details(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(entry) = app.entries.get(app.cursor) else {
        return;
    };
    let popup = centered_rect(84, 13, area);
    frame.render_widget(Clear, popup);

    let outcome = match &entry.outcome {
        Some(CleanupOutcome::Cleared(kb)) => {
            format!("Cleared; reclaimed about {}.", format_kb(*kb))
        }
        Some(CleanupOutcome::SafetySkipped(reason)) => format!("Skipped: {reason}."),
        Some(CleanupOutcome::Failed { error, removed_kb }) if *removed_kb > 0 => format!(
            "Failed after removing about {}: {error}",
            format_kb(*removed_kb)
        ),
        Some(CleanupOutcome::Failed { error, .. }) => format!("Failed: {error}"),
        None => entry.status.explanation().to_string(),
    };
    let mut text = vec![
        Line::from(vec![
            Span::styled("Size    ", Style::default().fg(app.color(Color::DarkGray))),
            Span::styled(
                format_kb(entry.size_kb),
                Style::default()
                    .fg(app.color(Color::Cyan))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Path    ", Style::default().fg(app.color(Color::DarkGray))),
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
        Line::from(""),
        Line::from(
            if !app.analysis_only && entry.status == CacheStatus::Review {
                "d advanced delete   •   c clean all safe   •   o reveal in Finder   •   Enter/Esc close"
            } else if !app.analysis_only && app.mode == Mode::Analyze {
                "c clean all safe   •   o reveal in Finder   •   Enter/Esc close details   •   q quit"
            } else {
                "o reveal in Finder   •   Enter/Esc close details   •   q quit"
            },
        ),
    ];
    if let Some(message) = &app.status_message {
        text.push(Line::from(Span::styled(
            message,
            Style::default().fg(app.color(Color::Cyan)),
        )));
    }
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(app.color(Color::Cyan)))
                    .title(format!(" {} ", entry.spec.label)),
            )
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn render_review_confirmation(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(entry) = app.review_target.and_then(|index| app.entries.get(index)) else {
        return;
    };
    let popup = centered_rect(90, 19, area);
    frame.render_widget(Clear, popup);

    let prompt_style = if app.review_confirmation_error {
        Style::default()
            .fg(app.color(Color::Red))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(app.color(Color::Yellow))
            .add_modifier(Modifier::BOLD)
    };
    let feedback = if app.review_confirmation_error {
        "Phrase does not match. Type DELETE exactly."
    } else {
        "Type DELETE, then press Enter:"
    };
    let text = vec![
        Line::from(Span::styled(
            "PERMANENTLY DELETE APP-MANAGED DATA?",
            Style::default()
                .fg(app.color(Color::Red))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!(
            "{}  •  {}",
            entry.spec.label,
            format_kb(entry.size_kb)
        )),
        Line::from(vec![
            Span::styled("Path  ", Style::default().fg(app.color(Color::DarkGray))),
            Span::raw(entry.spec.path.display().to_string()),
        ]),
        Line::from(vec![
            Span::styled("Impact  ", Style::default().fg(app.color(Color::DarkGray))),
            Span::raw(entry.spec.note),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Everything inside this folder will be permanently deleted. Settings, history, containers, apps, or local data may be lost.",
            Style::default().fg(app.color(Color::Red)),
        )),
        Line::from("Close the related application first. The folder itself will be retained."),
        Line::from(""),
        Line::from(Span::styled(feedback, prompt_style)),
        Line::from(Span::styled(
            format!("> {}_", app.review_confirmation),
            Style::default()
                .fg(app.color(Color::White))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("Esc cancels without changing files"),
    ];
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(app.color(Color::Red)))
                    .title(" ADVANCED REVIEW DELETION "),
            )
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        popup,
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

fn format_elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// Smoothly fills the active category's slice while preserving the exact
/// completed-category boundaries. Directory traversal cannot know its total
/// item count without doing an expensive first pass, so the active slice eases
/// toward (but never reaches) its boundary until the category really finishes.
fn smooth_scan_ratio(completed: usize, total: usize, active_elapsed: Option<Duration>) -> f64 {
    let total = total.max(1);
    if completed >= total {
        return 1.0;
    }

    let active_fraction = active_elapsed.map_or(0.0, |elapsed| {
        const INITIAL_ACTIVITY: f64 = 0.04;
        const ACTIVE_CAP: f64 = 0.94;
        const EASING_SECONDS: f64 = 24.0;
        let eased = 1.0 - (-elapsed.as_secs_f64() / EASING_SECONDS).exp();
        INITIAL_ACTIVITY + (ACTIVE_CAP - INITIAL_ACTIVITY) * eased
    });
    ((completed as f64 + active_fraction) / total as f64).clamp(0.0, 1.0)
}

fn scan_spinner(elapsed: Duration) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let frame = (elapsed.as_millis() / 80) as usize % FRAMES.len();
    FRAMES[frame]
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
            json: false,
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
        app.entries[0].status = CacheStatus::Review;
        assert!(!app.is_selectable(0));
    }

    #[test]
    fn location_picker_defaults_to_the_local_startup_volume() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: false,
            clean: false,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            json: false,
            no_color: true,
            no_tui: false,
        };

        let app = App::new(&cli, temp.path()).unwrap();
        let selected = &app.locations[app.location_cursor];

        assert_eq!(selected.path, Path::new("/"));
        assert_eq!(selected.kind, crate::cache::ScanLocationKind::Local);
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
            json: false,
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

    #[test]
    fn completed_summary_closes_after_the_grace_period() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: false,
            clean: true,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            json: false,
            no_color: true,
            no_tui: false,
        };
        let mut app = App::new(&cli, temp.path()).unwrap();
        app.phase = Phase::Summary;
        app.summary_started_at = Some(Instant::now());

        assert!(!app.summary_has_timed_out());

        app.summary_started_at = Some(
            Instant::now()
                .checked_sub(SUMMARY_AUTO_CLOSE_AFTER + Duration::from_millis(1))
                .unwrap(),
        );
        assert!(app.summary_has_timed_out());
    }

    #[test]
    fn completed_summary_can_be_closed_immediately() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: false,
            clean: true,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            json: false,
            no_color: true,
            no_tui: false,
        };
        let mut app = App::new(&cli, temp.path()).unwrap();
        app.phase = Phase::Summary;
        app.summary_started_at = Some(Instant::now());

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app.quit);
    }

    #[test]
    fn directory_scan_advances_in_bounded_visible_slices() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        fs::create_dir(&cache).unwrap();
        for index in 0..300 {
            fs::write(cache.join(format!("item-{index}")), [1_u8; 4_096]).unwrap();
        }
        let spec = CacheSpec {
            home: temp.path().to_path_buf(),
            path: cache,
            label: "Test cache",
            tier: crate::cache::CacheTier::Routine,
            process_pattern: "",
            note: "test data",
        };
        let mut scan = DirectoryScan::new(spec, CacheStatus::Ready);

        let finished = scan.advance();

        assert!(!finished);
        assert!((1..=256).contains(&scan.inspected_items));
        assert!(scan.size_kb() > 0);
        while !scan.advance() {}
        assert_eq!(scan.inspected_items, 300);
    }

    #[test]
    fn incomplete_directory_scan_is_not_marked_ready() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let unreadable = cache.join("unreadable");
        fs::create_dir(&cache).unwrap();
        fs::create_dir(&unreadable).unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        let spec = CacheSpec {
            home: temp.path().to_path_buf(),
            path: cache,
            label: "Test cache",
            tier: crate::cache::CacheTier::Routine,
            process_pattern: "",
            note: "test data",
        };
        let mut scan = DirectoryScan::new(spec, CacheStatus::Ready);

        while !scan.advance() {}
        let entry = scan.into_entry();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(entry.status, CacheStatus::ScanError);
    }

    #[test]
    fn active_scan_progress_moves_smoothly_without_claiming_completion() {
        let completed_boundary = smooth_scan_ratio(1, 7, None);
        let just_started = smooth_scan_ratio(1, 7, Some(Duration::ZERO));
        let after_ten_seconds = smooth_scan_ratio(1, 7, Some(Duration::from_secs(10)));
        let after_two_minutes = smooth_scan_ratio(1, 7, Some(Duration::from_secs(120)));
        let next_boundary = 2.0 / 7.0;

        assert_eq!(completed_boundary, 1.0 / 7.0);
        assert!(just_started > completed_boundary);
        assert!(after_ten_seconds > just_started);
        assert!(after_two_minutes > after_ten_seconds);
        assert!(after_two_minutes < next_boundary);
        assert_eq!(smooth_scan_ratio(7, 7, None), 1.0);
    }

    #[test]
    fn scan_activity_spinner_advances_independently_of_category_completion() {
        assert_ne!(
            scan_spinner(Duration::ZERO),
            scan_spinner(Duration::from_millis(80))
        );
    }

    #[test]
    fn escape_cancels_scan_and_returns_to_location_picker() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: true,
            clean: false,
            include_reinstallable: false,
            volume: Some(temp.path().to_path_buf()),
            yes: false,
            verbose: false,
            json: false,
            no_color: true,
            no_tui: false,
        };
        let mut app = App::new(&cli, temp.path()).unwrap();
        app.entries.push(CacheEntry {
            spec: app.specs[0].clone(),
            status: CacheStatus::Ready,
            size_kb: 10,
            outcome: None,
        });

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.phase, Phase::Location);
        assert!(!app.quit);
        assert!(app.entries.is_empty());
        assert!(app.scan_task.is_none());
    }

    #[test]
    fn enter_opens_and_closes_details_without_quitting_analyze_mode() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: true,
            clean: false,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            json: false,
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
        app.phase = Phase::Review;

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.phase, Phase::Details);
        assert!(!app.quit);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.phase, Phase::Review);
        assert!(!app.quit);
    }

    #[test]
    fn enter_still_opens_confirmation_when_cleanup_items_are_selected() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: false,
            clean: true,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            json: false,
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
        app.selected.insert(0);
        app.phase = Phase::Review;

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.phase, Phase::Confirm);
        assert!(!app.quit);
    }

    #[test]
    fn default_tui_can_prepare_all_safe_items_without_a_restart() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: false,
            clean: false,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            json: false,
            no_color: true,
            no_tui: false,
        };
        let mut app = App::new(&cli, temp.path()).unwrap();
        app.entries = vec![
            CacheEntry {
                spec: app.specs[0].clone(),
                status: CacheStatus::Ready,
                size_kb: 10,
                outcome: None,
            },
            CacheEntry {
                spec: app.specs[1].clone(),
                status: CacheStatus::InUse,
                size_kb: 20,
                outcome: None,
            },
            CacheEntry {
                spec: app.specs[2].clone(),
                status: CacheStatus::Review,
                size_kb: 30,
                outcome: None,
            },
        ];
        app.phase = Phase::Details;

        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));

        assert_eq!(app.mode, Mode::Clean);
        assert_eq!(app.phase, Phase::Confirm);
        assert_eq!(app.selected, BTreeSet::from([0]));
    }

    #[test]
    fn explicit_analyze_mode_keeps_safe_cleanup_locked() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: true,
            clean: false,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            json: false,
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
        app.phase = Phase::Review;

        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));

        assert_eq!(app.mode, Mode::Analyze);
        assert_eq!(app.phase, Phase::Review);
        assert!(app.selected.is_empty());
    }

    #[test]
    fn default_tui_shows_and_accepts_review_deletion_shortcut() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: false,
            clean: false,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            json: false,
            no_color: true,
            no_tui: false,
        };
        let mut app = App::new(&cli, temp.path()).unwrap();
        let review_spec = app
            .specs
            .iter()
            .find(|spec| spec.label == "Cursor user data")
            .unwrap()
            .clone();
        app.entries = vec![CacheEntry {
            spec: review_spec,
            status: CacheStatus::Review,
            size_kb: 10,
            outcome: None,
        }];
        app.phase = Phase::Details;
        let backend = ratatui::backend::TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered =
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .fold(String::new(), |mut text, cell| {
                    text.push_str(cell.symbol());
                    text
                });
        assert!(rendered.contains("d advanced delete"));
        assert!(rendered.contains("c clean all safe"));
        assert!(rendered.contains("o reveal in Finder"));

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

        assert_eq!(app.mode, Mode::Clean);
        assert_eq!(app.phase, Phase::ReviewConfirm);
        assert_eq!(app.review_target, Some(0));
    }

    #[test]
    fn review_deletion_requires_clean_mode_and_typed_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: false,
            clean: true,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            json: false,
            no_color: true,
            no_tui: false,
        };
        let mut app = App::new(&cli, temp.path()).unwrap();
        let review_spec = app
            .specs
            .iter()
            .find(|spec| spec.label == "Cursor user data")
            .unwrap()
            .clone();
        app.entries = vec![CacheEntry {
            spec: review_spec,
            status: CacheStatus::Review,
            size_kb: 10,
            outcome: None,
        }];
        app.phase = Phase::Review;

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(app.phase, Phase::ReviewConfirm);
        assert_eq!(app.review_target, Some(0));

        for character in "delete".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.phase, Phase::Cleaning);
        assert_eq!(app.cleanup_kind, CleanupKind::ReviewData);
        assert_eq!(app.cleanup_queue, vec![0]);
    }

    #[test]
    fn review_confirmation_prompt_is_visible_in_a_standard_terminal() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: false,
            clean: true,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            json: false,
            no_color: true,
            no_tui: false,
        };
        let mut app = App::new(&cli, temp.path()).unwrap();
        let review_spec = app
            .specs
            .iter()
            .find(|spec| spec.label == "Cursor user data")
            .unwrap()
            .clone();
        app.entries = vec![CacheEntry {
            spec: review_spec,
            status: CacheStatus::Review,
            size_kb: 10,
            outcome: None,
        }];
        app.phase = Phase::Review;
        app.start_review_confirmation();
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();

        let rendered =
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .fold(String::new(), |mut text, cell| {
                    text.push_str(cell.symbol());
                    text
                });
        assert!(rendered.contains("ADVANCED REVIEW DELETION"));
        assert!(rendered.contains("Type DELETE"));
        assert!(rendered.contains("> _"));
        assert!(rendered.contains("Esc cancels"));
    }

    #[test]
    fn review_deletion_is_unavailable_in_analyze_mode() {
        let temp = tempfile::tempdir().unwrap();
        let cli = Cli {
            analyze: true,
            clean: false,
            include_reinstallable: false,
            volume: None,
            yes: false,
            verbose: false,
            json: false,
            no_color: true,
            no_tui: false,
        };
        let mut app = App::new(&cli, temp.path()).unwrap();
        let review_spec = app
            .specs
            .iter()
            .find(|spec| spec.label == "Cursor user data")
            .unwrap()
            .clone();
        app.entries = vec![CacheEntry {
            spec: review_spec,
            status: CacheStatus::Review,
            size_kb: 10,
            outcome: None,
        }];
        app.phase = Phase::Review;

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

        assert_eq!(app.phase, Phase::Review);
        assert_eq!(app.review_target, None);
    }
}
