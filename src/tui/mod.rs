//! Terminal adapter: session state, lifecycle, and focused presentation modules.

use std::{
    cmp::Reverse,
    collections::{BTreeSet, HashSet},
    fs,
    io::{self, IsTerminal},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Offset, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, Gauge, LineGauge, List, ListItem, ListState,
        Padding, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Shadow, Table,
        TableState, Wrap,
    },
};

use crate::{
    cache::{
        CacheEntry, CacheSpec, CacheStatus, CacheTarget, CacheTier, CleanupOutcome, CleanupStats,
        PathIdentity, ScanLocation, ScanLocationKind, age_cutoff, clean_cache, clean_review_data,
        cleanup_plan, format_kb, free_kb, scan_location_kind, scan_locations, scan_specs,
        status_for, validate_scan_root,
    },
    cli::{Cli, Mode},
    processes::{
        ProcessEntry, ProcessHealth, ProcessOutcome, ProcessSignal, review_processes,
        signal_process,
    },
    relocation::{self, RelocationPlan, RelocationReport, RelocationStatus},
    retention::TempRetentionScan,
    storage::{StorageInventory, StorageItem, StorageItemKind},
    whitelist::Whitelist,
};

mod app;
mod dialogs;
mod explorer;
mod footer;
mod helpers;
mod input;
mod inspector;
mod process_view;
mod relocation_view;
mod scan;
mod shell;
mod storage_view;
#[cfg(test)]
mod tests;
mod theme;

use dialogs::*;
use explorer::*;
use footer::*;
use helpers::*;
use inspector::*;
use process_view::*;
use relocation_view::*;
use scan::{ScanExtras, spawn_cleanup_worker, spawn_retention_worker};
use shell::*;
use storage_view::*;
use theme::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Location,
    Scanning,
    Review,
    Details,
    RelocationSources,
    RelocationDestination,
    RelocationPlanning,
    RelocationConfirm,
    Relocating,
    RelocationResult,
    Confirm,
    ReviewConfirm,
    Processes,
    ProcessConfirm,
    Cleaning,
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuId {
    File,
    View,
    Actions,
    Help,
}

impl MenuId {
    const ALL: [Self; 4] = [Self::File, Self::View, Self::Actions, Self::Help];

    fn index(self) -> usize {
        match self {
            Self::File => 0,
            Self::View => 1,
            Self::Actions => 2,
            Self::Help => 3,
        }
    }

    fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }

    fn label(self) -> &'static str {
        match self {
            Self::File => "FILE",
            Self::View => "NAVIGATE",
            Self::Actions => "ACTIONS",
            Self::Help => "HELP",
        }
    }

    fn hotkey(self) -> char {
        match self {
            Self::File => 'F',
            Self::View => 'N',
            Self::Actions => 'A',
            Self::Help => 'H',
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct MenuItem {
    label: &'static str,
    shortcut: &'static str,
    enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanupKind {
    Cache,
    ReviewData,
}

enum CleanupMessage {
    ItemFinished {
        entry_index: usize,
        entry: Box<CacheEntry>,
        outcome: CleanupOutcome,
    },
    Finished {
        free_before_kb: u64,
        free_after_kb: u64,
    },
}

struct CleanupWorker {
    receiver: Receiver<CleanupMessage>,
    stop_requested: Arc<AtomicBool>,
}

struct InventoryWorker {
    receiver: Receiver<StorageInventory>,
    stop_requested: Arc<AtomicBool>,
}

struct RetentionWorker {
    receiver: Receiver<ScanExtras>,
}

struct RelocationPlanWorker {
    receiver: Receiver<Result<RelocationPlan, String>>,
}

struct RelocationWorker {
    receiver: Receiver<RelocationReport>,
    template: RelocationReport,
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
    identity: Option<PathIdentity>,
    started_at: Instant,
}

#[derive(Clone, Copy)]
enum HitTarget {
    Finding(usize),
    Consumer(usize),
    StorageTab(StorageTab),
    Location(usize),
    Process(usize),
    Source(usize),
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
    whitelist: Whitelist,
    phase: Phase,
    include_reinstallable: bool,
    tmp_retention_days: u64,
    temp_retention: TempRetentionScan,
    retention_worker: Option<RetentionWorker>,
    no_color: bool,
    scan_index: usize,
    scan_task: Option<DirectoryScan>,
    inventory: Option<StorageInventory>,
    storage_tab: StorageTab,
    explorer_path: Option<PathBuf>,
    explorer_cursor: usize,
    explorer_history: Vec<(Option<PathBuf>, usize)>,
    coverage_scroll: u16,
    coverage_max_scroll: std::cell::Cell<u16>,
    explorer_details: bool,
    inventory_worker: Option<InventoryWorker>,
    inventory_started_at: Option<Instant>,
    relocation_sources: Vec<StorageItem>,
    relocation_source_cursor: usize,
    relocation_source: Option<StorageItem>,
    relocation_destination: String,
    relocation_destination_error: Option<String>,
    relocation_plan: Option<RelocationPlan>,
    relocation_plan_worker: Option<RelocationPlanWorker>,
    relocation_worker: Option<RelocationWorker>,
    relocation_started_at: Option<Instant>,
    relocation_report: Option<RelocationReport>,
    scan_started_at: Instant,
    scan_progress_ratio: f64,
    scan_progress_updated_at: Instant,
    scan_work_complete: bool,
    cursor: usize,
    selected: BTreeSet<usize>,
    cleanup_queue: Vec<usize>,
    cleanup_index: usize,
    cleanup_kind: CleanupKind,
    cleanup_worker: Option<CleanupWorker>,
    cleanup_started_at: Option<Instant>,
    review_target: Option<usize>,
    review_confirmation: String,
    review_confirmation_error: bool,
    processes: Vec<ProcessEntry>,
    process_cursor: usize,
    process_scan_error: Option<String>,
    process_scan_completed: bool,
    process_scan_duration: Option<Duration>,
    process_target: Option<usize>,
    stats: CleanupStats,
    stopped_early: bool,
    summary_started_at: Option<Instant>,
    status_message: Option<String>,
    menu_open: Option<MenuId>,
    menu_cursor: usize,
    sidebar_cursor: usize,
    sidebar_focus: bool,
    terminal_width: u16,
    terminal_height: u16,
    show_help: bool,
    dialog_scroll: u16,
    dialog_max_scroll: std::cell::Cell<u16>,
    hit_regions: std::cell::RefCell<Vec<(Rect, HitTarget)>>,
    quit: bool,
}

pub fn can_run() -> bool {
    io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var("TERM").is_ok_and(|term| term != "dumb")
}

pub fn run(cli: &Cli, home: &Path) -> Result<i32, String> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|error| format!("could not enable terminal raw mode: {error}"))?;
    if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout, DisableMouseCapture, Show, LeaveAlternateScreen);
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
            let _ = execute!(fallback, DisableMouseCapture, Show, LeaveAlternateScreen);
            return Err(format!("could not initialize the terminal: {error}"));
        }
    };

    let app = match App::new(cli, home) {
        Ok(app) => app,
        Err(error) => {
            let _ = disable_raw_mode();
            let _ = execute!(
                terminal.backend_mut(),
                DisableMouseCapture,
                Show,
                LeaveAlternateScreen
            );
            let _ = terminal.show_cursor();
            return Err(error);
        }
    };
    let result = run_loop(&mut terminal, app);
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        Show,
        LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut app: App,
) -> Result<i32, String> {
    while !app.quit {
        let size = terminal
            .size()
            .map_err(|error| format!("could not read terminal size: {error}"))?;
        app.terminal_width = size.width;
        app.terminal_height = size.height;
        terminal
            .draw(|frame| render(frame, &app))
            .map_err(|error| format!("could not draw the interface: {error}"))?;

        if matches!(
            app.phase,
            Phase::Scanning | Phase::Cleaning | Phase::RelocationPlanning | Phase::Relocating
        ) {
            if event::poll(Duration::from_millis(16)).map_err(|error| error.to_string())? {
                let terminal_event = event::read().map_err(|error| error.to_string())?;
                handle_terminal_event(&mut app, terminal_event);
            }
            if !app.quit {
                app.advance_work();
            }
        } else if event::poll(Duration::from_millis(250)).map_err(|error| error.to_string())? {
            let terminal_event = event::read().map_err(|error| error.to_string())?;
            handle_terminal_event(&mut app, terminal_event);
        }

        if !app.quit && app.summary_has_timed_out() {
            app.quit = true;
        }
    }
    Ok(i32::from(
        app.stats.failed > 0
            || app
                .relocation_report
                .as_ref()
                .is_some_and(|report| report.status == RelocationStatus::Failed),
    ))
}

fn handle_terminal_event(app: &mut App, terminal_event: Event) {
    match terminal_event {
        Event::Key(key) => app.handle_key(key),
        Event::Mouse(mouse) => app.handle_mouse(mouse),
        Event::Resize(width, height) => {
            app.terminal_width = width;
            app.terminal_height = height;
        }
        _ => {}
    }
}
