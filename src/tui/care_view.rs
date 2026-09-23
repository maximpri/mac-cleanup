// SPDX-License-Identifier: GPL-3.0-or-later
//! Unified visual care workspace. The existing domain engines retain action authority.
use super::*;
use crate::cache;
use crate::{
    agent, agent_tools, ai,
    care::{self, Finding, Metrics, RecordedAction, Session, Target},
    investigation,
};
use std::collections::{HashMap, VecDeque};

mod presentation;
pub(super) use presentation::render;

/// A process report is retired once live readings are this much newer.
const REPORT_LIFETIME_SECS: u64 = 120;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Overview,
    Explore,
    History,
}
fn supports_subject(finding: &Finding, subject: &ai::Subject) -> bool {
    finding.id == subject.id
        && finding.consequence == subject.consequence
        && (matches!(finding.target, Target::Process(..))
            || finding.observation == subject.observation)
}
fn subject_for_finding(f: &Finding) -> ai::Subject {
    let action_ids = match &f.target {
        Target::Cache(path) if f.quick_win => vec![format!("clean:{}", path.display())],
        _ => vec![],
    };
    let (priority, disruption) = if f.quick_win {
        (1, "routine")
    } else {
        match &f.target {
            Target::System => (0, "attention"),
            Target::Process(..) => (2, "review"),
            Target::Folder(_) => (2, "review"),
            Target::Cache(_) => (3, "review"),
        }
    };
    ai::Subject {
        id: f.id.clone(),
        title: ai::display_text(&f.title),
        observation: ai::display_text(&f.observation),
        consequence: ai::display_text(&f.consequence),
        action_ids,
        quick_win: f.quick_win,
        priority,
        disruption: disruption.into(),
    }
}
#[derive(Clone)]
enum Action {
    Clean(CacheEntry),
    ReviewClean(CacheEntry),
    Signal(ProcessEntry, ProcessSignal),
    Move(RelocationPlan),
}
impl Action {
    fn id(&self) -> String {
        match self {
            Self::Clean(e) => format!("clean:{}", e.spec.path.display()),
            Self::ReviewClean(e) => format!("review:{}", e.spec.path.display()),
            Self::Signal(p, s) => format!("signal:{}:{}:{}", p.pid, p.start_time, s.label()),
            Self::Move(p) => format!("move:{}", p.source.display()),
        }
    }
    fn path(&self) -> Option<&Path> {
        match self {
            Self::Clean(e) | Self::ReviewClean(e) => Some(&e.spec.path),
            Self::Move(p) => Some(&p.source),
            Self::Signal(..) => None,
        }
    }
    fn description(&self) -> String {
        match self {
            Self::ReviewClean(e) => format!(
                "PERMANENTLY DELETE {} · {}\n{}\nApp-managed or personal data will be lost. Prefer the owning app. {}",
                e.spec.label,
                format_kb(e.size_kb),
                e.spec.path.display().to_string().escape_debug(),
                e.spec.note
            ),
            Self::Clean(e) => format!(
                "Clear {} · {}\n{}\nGood to know: {}",
                e.spec.label,
                format_kb(e.size_kb),
                e.spec.path.display().to_string().escape_debug(),
                e.spec.note
            ),
            Self::Signal(p, s) => format!(
                "{} · PID {} · {}\nStarted {} · parent {}\nUnsaved work may be lost. No automatic escalation.",
                s.label(),
                p.pid,
                ai::display_text(&p.command),
                p.start_time,
                p.parent_pid
            ),
            Self::Move(p) => format!(
                "Move {}\nto {}\nOriginal becomes a symlink. Keep the destination mounted.",
                p.source.display().to_string().escape_debug(),
                p.destination.display().to_string().escape_debug()
            ),
        }
    }
}
/// The one place a finding becomes a plan action. `Space` and AI suggestions
/// share it, so a suggestion can never describe a different action than the
/// one the user would add. `Ok(None)` means the target is no longer measured;
/// `Err` carries the reason no action is available.
fn action_for_finding(
    app: &App,
    target: &Target,
    force: bool,
) -> Result<Option<Action>, Option<String>> {
    match target {
        Target::Cache(path) => {
            let Some(entry) = app.entries.iter().find(|e| &e.spec.path == path).cloned() else {
                return Ok(None);
            };
            if matches!(
                entry.status,
                CacheStatus::Ready | CacheStatus::Optional | CacheStatus::InUse
            ) {
                Ok(Some(Action::Clean(entry)))
            } else if entry.status == CacheStatus::Review {
                Ok(Some(Action::ReviewClean(entry)))
            } else {
                Err(Some(entry.status.explanation().into()))
            }
        }
        Target::Process(pid, identity) => {
            let Some(process) = app
                .processes
                .iter()
                .find(|p| p.pid == *pid && &p.start_time == identity)
            else {
                return Ok(None);
            };
            if process.signalable {
                Ok(Some(Action::Signal(
                    process.clone(),
                    if force {
                        ProcessSignal::Kill
                    } else {
                        ProcessSignal::Terminate
                    },
                )))
            } else {
                Err(process.signal_block_reason.clone())
            }
        }
        _ => Err(Some(
            "Inspect this area or choose a supported action on one of its findings.".into(),
        )),
    }
}
/// The action ID the model may suggest for a target: only routine cleanup
/// that is ready now, or a graceful stop of a signalable account process.
/// Review data, in-use caches, force-stops, and moves are never suggested.
fn suggestion_id(app: &App, target: &Target) -> Option<String> {
    care::suggestion_id(&app.entries, &app.processes, target)
}
/// A validated local-AI report shown with its finding (or Ask question).
struct ReportView {
    case_id: String,
    /// The finding it explains; `None` for Ask answers.
    subject: Option<ai::Subject>,
    sampled_at: Option<u64>,
    summary: String,
    cited: Vec<String>,
    suggestions: Vec<String>,
    by_model: bool,
}
struct MeasureWork {
    receiver: Receiver<StorageInventory>,
    stop: Arc<AtomicBool>,
}
impl Drop for MeasureWork {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
enum WorkMessage {
    Progress(Session),
    Finished(Session),
}
struct Work {
    receiver: Receiver<WorkMessage>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Work {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
struct InsightWork {
    receiver: Receiver<Result<ai::Insight, String>>,
    worker: Option<thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    request: ai::Request,
}
struct TriageWork {
    receiver: Receiver<Result<ai::Triage, String>>,
    worker: Option<thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    request: ai::Request,
    id_map: HashMap<String, String>,
}
impl Drop for TriageWork {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for InsightWork {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn resolve_triage_ids(
    mut triage: ai::Triage,
    id_map: &HashMap<String, String>,
) -> Option<ai::Triage> {
    for id in triage
        .key_area_ids
        .iter_mut()
        .chain(triage.quick_win_ids.iter_mut())
    {
        *id = id_map.get(id)?.clone();
    }
    // Model prose is not evidence and may not correspond to the returned order.
    triage.reasons.clear();
    Some(triage)
}

pub(super) struct Workspace {
    assessment: Option<care::Assessment>,
    findings: Vec<Finding>,
    screen: Screen,
    cursor: usize,
    nav: usize,
    focus: usize,
    detail: bool,
    help: bool,
    coverage: bool,
    detail_scroll: u16,
    show_all: bool,
    kept: HashSet<String>,
    /// The finding kept visible after re-ranking because it is selected.
    pinned: Option<String>,
    /// Significant growth since the previous complete assessment.
    growth: Vec<crate::growth::Delta>,
    growth_since: Option<u64>,
    /// An outside agent's proposal, applied once the assessment finishes.
    proposal: Option<(PathBuf, crate::pending::PendingPlan)>,
    stage: String,
    progress: care::AssessmentProgress,
    assessment_elapsed: Option<Duration>,
    complete: bool,
    volume: Option<crate::storage::VolumeStats>,
    metrics: Metrics,
    trend: VecDeque<u64>,
    revision: u64,
    started: Instant,
    triage_work: Option<TriageWork>,
    triage: Option<ai::Triage>,
    triage_revision: Option<u64>,
    triage_is_ai: bool,
    result_work: Option<InsightWork>,
    result_insight: Option<ai::Insight>,
    result_summary_session: Option<u64>,
    ai_error: Option<String>,
    ai_framework: ai::FrameworkStatus,
    ai_framework_work: Option<Receiver<ai::FrameworkStatus>>,
    auto_requested: bool,
    /// The one automatic investigation per assessment has started.
    auto_agent_started: bool,
    /// An automatic investigation paused for memory pressure resumes once.
    auto_agent_resume: bool,
    agent: Option<agent::AgentRun>,
    investigation_case: Option<investigation::InvestigationCase>,
    /// The finding the current investigation explains, as measured when it started.
    agent_subject: Option<ai::Subject>,
    report: Option<ReportView>,
    /// Text typed into the Ask box while it is open.
    asking: Option<String>,
    /// The Ask answer page is open.
    answer_open: bool,
    measure: Option<MeasureWork>,
    online_research: bool,
    note: Option<String>,
    plan: Vec<Action>,
    reviewing: bool,
    signals_ack: bool,
    moves_ack: bool,
    clearing_history: bool,
    acknowledgement: String,
    review_scroll: u16,
    work: Option<Work>,
    session: Session,
    history: Vec<Session>,
    history_cursor: usize,
    /// The user moved the selection during this assessment.
    user_moved: bool,
    /// Smallest item listed by default: 100 MB, or 1% of what was measured
    /// on small volumes.
    min_row_kb: u64,
    /// What blocks each in-use cache, as `name (PID n)`.
    blockers: HashMap<PathBuf, Vec<String>>,
    /// Findings left out of the default list: unreadable targets and
    /// ordinary running processes. `f` shows them.
    hidden_by_default: HashSet<String>,
    /// The `:` command palette: filter text and selected row.
    palette: Option<(String, usize)>,
    legacy: bool,
    motion: bool,
    focused: bool,
    flash: Option<Instant>,
    hits: std::cell::RefCell<Vec<(Rect, Control)>>,
}
/// How many rows the Overview lists before `f` shows everything.
const OVERVIEW_ROWS: usize = 15;
/// Terminals at least this wide show details beside the list.
const SPLIT_WIDTH: u16 = 110;

/// Every command in the `:` palette: key, what it does, and the key it sends.
const PALETTE: [(&str, &str, KeyCode); 22] = [
    (
        "1",
        "Go to Overview: where the space went",
        KeyCode::Char('1'),
    ),
    (
        "2",
        "Go to Explore: browse folders by size",
        KeyCode::Char('2'),
    ),
    ("3", "Go to History", KeyCode::Char('3')),
    ("Enter", "Explain the selected item", KeyCode::Enter),
    (
        "Space",
        "Add or remove the selected item from your plan",
        KeyCode::Char(' '),
    ),
    ("a", "Add all quick wins to your plan", KeyCode::Char('a')),
    ("p", "Review your plan", KeyCode::Char('p')),
    ("i", "Investigate the selected item", KeyCode::Char('i')),
    ("/", "Ask a question about this Mac", KeyCode::Char('/')),
    ("e", "Browse the selected item's folder", KeyCode::Char('e')),
    ("o", "Reveal in Finder", KeyCode::Char('o')),
    ("f", "Show all items, or fewer", KeyCode::Char('f')),
    (
        "K",
        "Hide this item until the next scan",
        KeyCode::Char('K'),
    ),
    ("v", "What the scan could not see", KeyCode::Char('v')),
    ("r", "Scan again", KeyCode::Char('r')),
    ("P", "Running processes", KeyCode::Char('P')),
    (
        "x",
        "Force-stop the selected process (adds to plan)",
        KeyCode::Char('x'),
    ),
    (
        "m",
        "Move a large folder to another disk",
        KeyCode::Char('m'),
    ),
    (
        "A",
        "Open macOS Settings (Full Disk Access, Apple Intelligence)",
        KeyCode::Char('A'),
    ),
    ("M", "Reduce motion on or off", KeyCode::Char('M')),
    (
        "R",
        "Online reference research on or off",
        KeyCode::Char('R'),
    ),
    ("q", "Quit", KeyCode::Char('q')),
];

/// Palette commands whose key or description contains every typed word.
fn palette_matches(query: &str) -> Vec<(&'static str, &'static str, KeyCode)> {
    let query = query.to_lowercase();
    PALETTE
        .iter()
        .filter(|(key, label, _)| {
            let text = format!("{key} {}", label.to_lowercase());
            query.split_whitespace().all(|word| text.contains(word))
        })
        .copied()
        .collect()
}

/// Up to three follow-up questions after an answer, never the same question.
fn follow_ups(question: &str) -> Vec<&'static str> {
    let asked = question.trim().to_lowercase();
    [
        "Why is my disk almost full?",
        "What grew since last week?",
        "Which caches can I clear safely?",
        "Where is the hidden space on this disk?",
        "What is using memory right now?",
    ]
    .into_iter()
    .filter(|candidate| candidate.to_lowercase() != asked)
    .take(3)
    .collect()
}

#[derive(Clone, Copy)]
enum Control {
    Nav(usize),
    Row(usize),
    Key(KeyCode),
}

impl Workspace {
    pub(super) fn new(app: &App) -> Self {
        let mut result = Self::empty();
        result.history = care::sessions(&app.account_home);
        result.online_research = care::online_research_enabled(&app.account_home);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(ai::framework_status());
        });
        result.ai_framework_work = Some(receiver);
        result.assessment = Some(care::assess(
            app.scan_root.clone(),
            app.account_home.clone(),
            app.include_reinstallable,
            app.tmp_retention_days,
        ));
        result
    }
    fn empty() -> Self {
        Self {
            assessment: None,
            findings: vec![],
            screen: Screen::Overview,
            cursor: 0,
            nav: 0,
            focus: 1,
            detail: false,
            help: false,
            coverage: false,
            detail_scroll: 0,
            show_all: false,
            kept: HashSet::new(),
            pinned: None,
            growth: Vec::new(),
            growth_since: None,
            proposal: None,
            stage: "Starting assessment".into(),
            progress: Default::default(),
            assessment_elapsed: None,
            complete: false,
            volume: None,
            metrics: Metrics::default(),
            trend: VecDeque::new(),
            revision: 0,
            started: Instant::now(),
            triage_work: None,
            triage: None,
            triage_revision: None,
            triage_is_ai: false,
            result_work: None,
            result_insight: None,
            result_summary_session: None,
            ai_error: None,
            ai_framework: ai::FrameworkStatus::Detecting,
            ai_framework_work: None,
            auto_requested: false,
            auto_agent_started: false,
            auto_agent_resume: false,
            agent: None,
            investigation_case: None,
            agent_subject: None,
            report: None,
            asking: None,
            answer_open: false,
            measure: None,
            online_research: false,
            note: None,
            plan: vec![],
            reviewing: false,
            signals_ack: false,
            moves_ack: false,
            clearing_history: false,
            acknowledgement: String::new(),
            review_scroll: 0,
            work: None,
            session: Session::default(),
            history: vec![],
            history_cursor: 0,
            user_moved: false,
            min_row_kb: care::BREAKDOWN_MIN_KB,
            blockers: HashMap::new(),
            hidden_by_default: HashSet::new(),
            palette: None,
            legacy: false,
            motion: std::env::var_os("REDUCE_MOTION").is_none(),
            focused: true,
            flash: None,
            hits: Default::default(),
        }
    }
    /// Rows in the Overview: disk items ranked by size, with live alerts
    /// first. By default unreadable targets, ordinary running processes,
    /// small items, and the low-space alert (the header shows free space)
    /// are left out, and at most `OVERVIEW_ROWS` are listed; `f` lists all.
    fn visible(&self) -> Vec<&Finding> {
        let rank = |f: &Finding| {
            let group = match &f.target {
                Target::System => 0,
                Target::Cache(_) | Target::Folder(_) if !f.id.starts_with("growth:") => 1,
                Target::Process(..) => 2,
                _ => 3,
            };
            (group, std::cmp::Reverse(f.size_kb))
        };
        let mut rows: Vec<&Finding> = self
            .findings
            .iter()
            .filter(|f| !self.kept.contains(&f.id))
            .filter(|f| {
                // The selected finding always stays listed, so re-ranking can
                // never silently move the cursor onto a different finding.
                self.show_all
                    || self.pinned.as_deref() == Some(f.id.as_str())
                    || self.listed_by_default(f)
            })
            .collect();
        rows.sort_by_key(|f| rank(f));
        if !self.show_all && rows.len() > OVERVIEW_ROWS {
            let pinned = self.pinned.as_deref();
            let keep = rows
                .iter()
                .skip(OVERVIEW_ROWS)
                .find(|f| Some(f.id.as_str()) == pinned)
                .copied();
            rows.truncate(OVERVIEW_ROWS);
            rows.extend(keep);
        }
        rows
    }
    fn listed_by_default(&self, f: &Finding) -> bool {
        if self.hidden_by_default.contains(&f.id) || f.id.starts_with("growth:") {
            return false;
        }
        match &f.target {
            Target::System => f.id != "system:disk",
            Target::Cache(_) => f.quick_win || (f.size_kb > 0 && f.size_kb >= self.min_row_kb),
            Target::Folder(_) => f.size_kb >= self.min_row_kb,
            Target::Process(..) => true,
        }
    }
    fn selected(&self) -> Option<&Finding> {
        self.visible().get(self.cursor).copied()
    }
    /// "What grew" findings for the three largest increases. Growth alone
    /// never makes data removable; these lead to an investigation.
    fn growth_findings(&self, app: &App) -> Vec<Finding> {
        let since = self
            .growth_since
            .map(|updated| {
                crate::history::format_timestamp(
                    std::time::UNIX_EPOCH + Duration::from_secs(updated),
                )
            })
            .unwrap_or_else(|| "the previous check".into());
        self.growth
            .iter()
            .filter(|delta| delta.change_kb() > 0)
            .take(3)
            .map(|delta| {
                let path = PathBuf::from(&delta.path);
                Finding {
                    id: format!("growth:{}", delta.path),
                    title: format!(
                        "Grew {}: {}",
                        format_kb(delta.change_kb().unsigned_abs()),
                        agent_tools::short_path(&path, &app.account_home)
                    ),
                    observation: format!(
                        "{} → {} since {since}.",
                        format_kb(delta.before_kb),
                        format_kb(delta.after_kb)
                    ),
                    consequence: "Growth alone does not make data removable. Press i to see what is writing here.".into(),
                    size_kb: delta.after_kb,
                    quick_win: false,
                    target: Target::Folder(path),
                    related_pids: vec![],
                }
            })
            .collect()
    }
    fn apply_triage(&mut self, triage: ai::Triage, ai_ranked: bool) {
        let selected = self.selected().map(|finding| finding.id.clone());
        let mut rank = HashMap::new();
        for (index, id) in triage.quick_win_ids.iter().enumerate() {
            rank.insert(id.clone(), (0_u8, index));
        }
        for (index, id) in triage.key_area_ids.iter().enumerate() {
            rank.insert(id.clone(), (1_u8, index));
        }
        let old_order: HashMap<_, _> = self
            .findings
            .iter()
            .enumerate()
            .map(|(index, finding)| (finding.id.clone(), index))
            .collect();
        self.findings.sort_by_key(|finding| {
            rank.get(&finding.id)
                .copied()
                .unwrap_or((2, old_order.get(&finding.id).copied().unwrap_or(usize::MAX)))
        });
        self.triage = Some(triage);
        self.triage_revision = Some(self.revision);
        self.triage_is_ai = ai_ranked;
        self.pinned = selected.clone();
        self.cursor = selected
            .and_then(|id| self.visible().iter().position(|finding| finding.id == id))
            .unwrap_or_else(|| self.cursor.min(self.visible().len().saturating_sub(1)));
    }
    fn rebuild(&mut self, app: &App) {
        // Until the user picks something, the selection stays on the top row
        // as new measurements re-rank the list.
        if !self.user_moved {
            self.pinned = None;
            self.cursor = 0;
        }
        let selected = self
            .selected()
            .filter(|_| self.user_moved)
            .map(|f| f.id.clone());
        let mut incoming = care::findings(
            &app.entries,
            &app.processes,
            &self.metrics,
            app.inventory.as_ref(),
        );
        if let Some(finding) = self.volume.as_ref().and_then(care::disk_finding) {
            incoming.insert(0, finding);
        }
        let system = incoming
            .iter()
            .take_while(|finding| matches!(finding.target, Target::System))
            .count();
        for (offset, finding) in self.growth_findings(app).into_iter().enumerate() {
            incoming.insert(system + offset, finding);
        }
        let old: HashMap<_, _> = self
            .findings
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id.clone(), i))
            .collect();
        incoming.sort_by_key(|f| old.get(&f.id).copied().unwrap_or(usize::MAX));
        self.findings = incoming;
        let measured = app.inventory.as_ref().map_or_else(
            || app.entries.iter().map(|e| e.size_kb).sum(),
            |i| i.scanned_kb,
        );
        self.min_row_kb = care::BREAKDOWN_MIN_KB.min((measured / 100).max(1));
        let pressure = self.metrics.pressure.is_some_and(|p| p == 2 || p == 4);
        self.hidden_by_default = self
            .findings
            .iter()
            .filter(|f| match &f.target {
                Target::Cache(path) => app
                    .entries
                    .iter()
                    .find(|e| &e.spec.path == path)
                    .is_some_and(|e| e.status == CacheStatus::ScanError),
                Target::Process(pid, _) => {
                    !pressure
                        && app
                            .processes
                            .iter()
                            .find(|p| p.pid == *pid)
                            .is_none_or(|p| p.health == crate::processes::ProcessHealth::Running)
                }
                _ => false,
            })
            .map(|f| f.id.clone())
            .collect();
        if let Some(id) = selected {
            self.pinned = Some(id.clone());
            self.cursor = self
                .visible()
                .iter()
                .position(|f| f.id == id)
                .unwrap_or(self.cursor);
        }
        self.cursor = self.cursor.min(self.visible().len().saturating_sub(1));
        // A report explains the evidence it was built from. Changed cleanup
        // measurements retire it at once; live process readings drift every
        // sample, so a process report is retired once it is two minutes stale.
        let expired = self.report.as_ref().and_then(|report| {
            let subject = report.subject.as_ref()?;
            if !self.findings.iter().any(|f| supports_subject(f, subject)) {
                return Some("Evidence changed since this local AI report. Press i to investigate again.");
            }
            let live = subject.id.starts_with("process:") || subject.id == "system:memory";
            (live
                && report
                    .sampled_at
                    .is_some_and(|at| self.metrics.sampled_at > at + REPORT_LIFETIME_SECS))
            .then_some("This local AI report is older than the live process readings. Press i to refresh it.")
        });
        if let Some(reason) = expired {
            self.report = None;
            self.ai_error = Some(reason.into());
        }
        if self.triage.is_some()
            && (self.triage_revision != Some(self.revision)
                || self.triage.as_ref().is_some_and(|triage| {
                    triage
                        .key_area_ids
                        .iter()
                        .chain(triage.quick_win_ids.iter())
                        .any(|id| !self.findings.iter().any(|finding| &finding.id == id))
                }))
        {
            self.triage = None;
            self.triage_revision = None;
            self.triage_is_ai = false;
        }
    }
    pub(super) fn tick(&mut self, app: &mut App) {
        if let Some(status) = self
            .ai_framework_work
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        {
            self.ai_framework = status;
            self.ai_framework_work = None;
        }
        let mut events = Vec::new();
        if let Some(assessment) = &self.assessment {
            while let Ok(event) = assessment.receiver.try_recv() {
                events.push(event);
            }
        }
        let mut changed = false;
        for event in events {
            match event {
                care::Event::Capacity(result) => match result {
                    Ok(stats) => {
                        self.volume = Some(stats);
                        changed = true;
                    }
                    Err(error) => self.note = Some(format!("Disk accounting unavailable: {error}")),
                },
                care::Event::Processes(result, metrics) => {
                    self.metrics = metrics;
                    self.respond_to_pressure(app);
                    if !self.metrics.cpu.is_empty() {
                        self.trend
                            .push_back(self.metrics.cpu.values().sum::<f64>().round() as u64);
                    }
                    if self.trend.len() > 40 {
                        self.trend.pop_front();
                    }
                    match result {
                        Ok(processes) => {
                            app.processes = processes;
                            app.process_scan_error = None;
                        }
                        Err(error) => {
                            app.processes.clear();
                            app.process_scan_error = Some(error.clone());
                            self.note = Some(format!("Process readings unavailable: {error}"));
                        }
                    }
                    changed = true;
                }
                care::Event::Entry(entry) => {
                    if entry.status == CacheStatus::InUse {
                        self.blockers.insert(
                            entry.spec.path.clone(),
                            crate::cache::blocking_processes(entry.spec.process_pattern),
                        );
                    } else {
                        self.blockers.remove(&entry.spec.path);
                    }
                    if let Some(existing) = app
                        .entries
                        .iter_mut()
                        .find(|e| e.spec.path == entry.spec.path)
                    {
                        *existing = entry;
                    } else {
                        if !app.specs.iter().any(|s| s.path == entry.spec.path) {
                            app.specs.push(entry.spec.clone());
                        }
                        app.entries.push(entry);
                    }
                    self.revision += 1;
                    changed = true;
                }
                care::Event::Inventory(inventory) => {
                    app.inventory = Some(inventory);
                    self.revision += 1;
                    changed = true;
                }
                care::Event::Stage(stage) => self.stage = stage,
                care::Event::Progress(progress) => self.progress = progress,
                care::Event::Finished => {
                    // Start at the largest item unless the user already chose one.
                    if !self.user_moved {
                        self.pinned = None;
                        self.cursor = 0;
                    }
                    self.complete = true;
                    self.assessment_elapsed = Some(self.started.elapsed());
                    self.auto_requested = false;
                    self.stage = if app.inventory.as_ref().is_some_and(|i| i.complete) {
                        "Assessment complete"
                    } else {
                        "Assessment partial · inspect coverage"
                    }
                    .into();
                    self.session.measurements =
                        care::measurements(&app.entries, app.inventory.as_ref());
                    self.session.root = Some(app.scan_root.display().to_string());
                    self.session.volume_id = care::volume_id(&app.scan_root);
                    self.session.complete = app.inventory.as_ref().is_some_and(|i| i.complete);
                    self.session.source = "tui".into();
                    self.session.state = self.stage.clone();
                    self.session.after = Some(self.metrics.clone());
                    if let Some(volume) = &self.volume {
                        self.session.free_after_kb = Some(volume.disk_free_kb());
                    }
                    self.persist(app);
                    self.history = care::sessions(&app.account_home);
                    self.apply_proposal(app);
                    if let Some(base) = crate::growth::previous(&self.history, &self.session) {
                        self.growth = crate::growth::diff(base, &self.session);
                        self.growth_since = Some(base.updated);
                    }
                }
            }
        }
        if changed {
            self.rebuild(app);
        }
        self.maybe_start_automatic(app);
        if let Some(result) =
            self.triage_work
                .as_ref()
                .and_then(|work| match work.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Disconnected) => Some(Err("AI triage worker stopped".into())),
                    Err(TryRecvError::Empty) => None,
                })
        {
            let work = self.triage_work.take().expect("AI triage work");
            match result {
                Ok(triage) if work.request.revision == self.revision => {
                    if let Some(triage) = resolve_triage_ids(triage, &work.id_map) {
                        self.apply_triage(triage, true);
                        self.flash = Some(Instant::now());
                        self.ai_error = None;
                    } else {
                        self.apply_triage(
                            resolve_triage_ids(
                                ai::deterministic_triage(&work.request),
                                &work.id_map,
                            )
                            .expect("policy triage uses known IDs"),
                            false,
                        );
                        self.ai_error = Some(
                            "AI ranking used unknown findings. Showing measured priority order."
                                .into(),
                        );
                    }
                }
                Ok(_) => {
                    self.ai_error = Some(
                        "Evidence changed while triage was running. The measured order is still used.".into(),
                    );
                }
                Err(error) if work.request.revision == self.revision => {
                    self.apply_triage(
                        resolve_triage_ids(ai::deterministic_triage(&work.request), &work.id_map)
                            .expect("policy triage uses known IDs"),
                        false,
                    );
                    self.ai_error = Some(format!(
                        "AI ranking skipped: {} Showing measured priority order.",
                        ai::display_text(&error)
                    ));
                }
                Err(error) => self.ai_error = Some(error),
            }
        }
        if let Some(result) =
            self.result_work
                .as_ref()
                .and_then(|work| match work.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Disconnected) => {
                        Some(Err("AI result-summary worker stopped".into()))
                    }
                    Err(TryRecvError::Empty) => None,
                })
        {
            let work = self.result_work.take().expect("AI result summary work");
            match result {
                Ok(summary)
                    if summary.evidence_ids.iter().all(|id| {
                        work.request
                            .subjects
                            .iter()
                            .any(|subject| &subject.id == id)
                    }) =>
                {
                    self.result_insight = Some(summary);
                    self.flash = Some(Instant::now());
                }
                Ok(_) => {
                    self.ai_error = Some("AI result summary cited unsupported evidence.".into())
                }
                Err(error) => self.ai_error = Some(error),
            }
        }
        if let Some(inventory) = self
            .measure
            .as_ref()
            .and_then(|work| work.receiver.try_recv().ok())
        {
            self.measure = None;
            let complete = inventory.complete;
            if let Some(existing) = &mut app.inventory {
                existing.children.extend(inventory.children);
            } else {
                app.inventory = Some(inventory);
            }
            self.note = Some(if complete {
                "Selected folder measured. Explore shows its children."
            } else {
                "Selected folder scan is partial. Unreadable or cancelled entries are not empty."
            }
            .into());
            self.revision += 1;
            self.flash = Some(Instant::now());
        }
        self.pump_agent(app);
        let work_message = self
            .work
            .as_ref()
            .and_then(|work| match work.receiver.try_recv() {
                Ok(message) => Some(message),
                Err(TryRecvError::Disconnected) => Some(WorkMessage::Finished(Session {
                    state: "Worker stopped · verification needed".into(),
                    ..self.session.clone()
                })),
                Err(TryRecvError::Empty) => None,
            });
        if let Some(message) = work_message {
            match message {
                WorkMessage::Progress(session) => {
                    self.session = session;
                }
                WorkMessage::Finished(session) => {
                    self.session = session;
                    self.work = None;
                    self.plan.clear();
                    self.navigate(3);
                    self.detail = true;
                    self.history = care::sessions(&app.account_home);
                    self.history.insert(0, self.session.clone());
                    self.history.dedup_by_key(|s| s.id);
                    self.history_cursor = 0;
                    self.stage = "Results ready · Recheck to assess again".into();
                    self.flash = Some(Instant::now());
                    self.report = None;
                    self.result_insight = None;
                    let completed = self.session.clone();
                    self.result_summary_session = Some(completed.id);
                    self.start_result_summary(&completed);
                }
            }
        }
        if self.legacy
            && app.phase == Phase::RelocationConfirm
            && let Some(plan) = app.relocation_plan.take()
        {
            self.add_action(Action::Move(plan));
            app.phase = Phase::Review;
            self.legacy = false;
            self.reviewing = true;
        }
        if self.legacy && matches!(app.phase, Phase::Review | Phase::Summary) {
            self.legacy = false;
            if app.phase == Phase::Summary {
                self.note = Some(format!(
                    "Cleanup finished · {} removed",
                    format_kb(app.stats.measured_removed_kb)
                ));
            }
            app.phase = Phase::Review;
            self.rebuild(app);
        }
        if !self.auto_requested
            // The first telemetry sample is enough to make ranking useful;
            // waiting ten seconds made local AI feel absent even when the
            // model was ready. Deeper investigations remain explicit.
            && self.started.elapsed() >= Duration::from_secs(3)
            && self.metrics.sampled_at > 0
            && (self.findings.len() >= 3 || self.complete)
            && !self.findings.is_empty()
            && self.work.is_none()
            && self.triage_work.is_none()
            && self.agent.is_none()
            && !self.reviewing
            && !self.legacy
        {
            self.start_triage();
        }
    }
    fn persist(&mut self, app: &App) {
        // A session can contain several bounded investigations (for example,
        // a process case followed by a storage case). Updating one case must
        // not erase the earlier evidence from the durable history record.
        if let Some(case) = self.investigation_case.clone() {
            if let Some(existing) = self
                .session
                .investigations
                .iter_mut()
                .find(|existing| existing.id == case.id)
            {
                *existing = case;
            } else {
                self.session.investigations.push(case);
            }
        }
        self.session.updated = care::timestamp();
        if let Err(error) = care::save_session(&app.account_home, &self.session) {
            self.note = Some(format!("History could not be saved: {error}"));
        }
    }
    /// Automatic model work yields to memory pressure. Critical pressure
    /// stops any model session; measured checks keep running.
    fn respond_to_pressure(&mut self, app: &App) {
        let pressure = self.metrics.pressure;
        if pressure == Some(4) && self.triage_work.is_some() {
            self.triage_work = None;
            self.ai_error = Some("Local AI paused while memory pressure is critical.".into());
        }
        let Some(run) = &self.agent else {
            return;
        };
        if !run.by_model() {
            return;
        }
        if run.automatic && pressure.is_some_and(|level| level >= 2) {
            self.cancel_agent(
                app,
                "Automatic local AI paused while memory pressure is elevated. It resumes once pressure is normal.",
            );
            self.auto_agent_resume = true;
        } else if pressure == Some(4) {
            self.cancel_agent(
                app,
                "Local AI stopped while memory pressure is critical. Evidence collected so far is kept.",
            );
        }
    }
    /// Investigate the top key area once per assessment, after the scan and
    /// triage have settled, only when the model is ready and pressure is normal.
    fn maybe_start_automatic(&mut self, app: &App) {
        let ready = matches!(self.ai_framework, ai::FrameworkStatus::Available { .. })
            && self.complete
            && self.metrics.pressure == Some(1)
            && self.triage_work.is_none()
            && self.triage_revision == Some(self.revision)
            && self.agent.is_none()
            && self.work.is_none()
            && self.asking.is_none()
            && !self.reviewing
            && !self.legacy;
        if !ready || (self.auto_agent_started && !self.auto_agent_resume) {
            return;
        }
        let top = self
            .triage
            .as_ref()
            .and_then(|triage| triage.key_area_ids.first())
            .and_then(|id| self.findings.iter().find(|finding| &finding.id == id))
            .or_else(|| {
                self.visible()
                    .into_iter()
                    .find(|finding| !finding.quick_win)
            })
            .cloned();
        let Some(finding) = top else {
            return;
        };
        self.auto_agent_started = true;
        self.auto_agent_resume = false;
        self.start_investigation(app, &finding, true);
        if self.agent.is_some() {
            self.note = Some(format!(
                "Local AI is looking into {} with a small read-only tool budget. Press i for a deeper look, or keep browsing.",
                ai::display_text(&finding.title)
            ));
        }
    }
    /// Stop the running investigation and keep its evidence in History.
    fn cancel_agent(&mut self, app: &App, reason: &str) {
        if self.agent.take().is_none() {
            return;
        }
        if let Some(case) = &mut self.investigation_case
            && !case.phase.finished()
        {
            case.phase = investigation::CasePhase::Inconclusive;
            case.conclusion = Some(format!("Stopped before a conclusion. {reason}"));
        }
        self.ai_error = Some(reason.into());
        self.persist(app);
    }
    /// Build a case for a finding. Families decide the hypotheses and toolset.
    fn case_for_finding(
        &self,
        app: &App,
        finding: &Finding,
        automatic: bool,
    ) -> investigation::InvestigationCase {
        let is_fseventsd = match &finding.target {
            Target::Process(pid, identity) => app
                .processes
                .iter()
                .find(|process| process.pid == *pid && &process.start_time == identity)
                .is_some_and(|process| {
                    Path::new(&process.command)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.eq_ignore_ascii_case("fseventsd"))
                }),
            _ => false,
        };
        let mut case = if is_fseventsd {
            investigation::InvestigationCase::new_fseventsd("fseventsd", self.revision, automatic)
        } else {
            match &finding.target {
                Target::Cache(path) | Target::Folder(path) => {
                    let label =
                        format!("{} {}", finding.title, path.display()).to_ascii_lowercase();
                    let target = ai::display_text(&path.display().to_string());
                    if [
                        "xcode",
                        "developer",
                        "opencode",
                        "codex",
                        "playwright",
                        "node",
                        "npm",
                        "python",
                        "homebrew",
                        "gradle",
                        "swiftpm",
                    ]
                    .iter()
                    .any(|marker| label.contains(marker))
                    {
                        investigation::InvestigationCase::new_developer(
                            target,
                            self.revision,
                            automatic,
                        )
                    } else {
                        investigation::InvestigationCase::new_storage(
                            target,
                            self.revision,
                            automatic,
                        )
                    }
                }
                Target::Process(..) => investigation::InvestigationCase::new_process(
                    finding.title.clone(),
                    self.revision,
                    automatic,
                    finding.observation.contains("RAM"),
                ),
                Target::System if finding.id == "system:disk" => {
                    investigation::InvestigationCase::new_capacity(self.revision, automatic)
                }
                Target::System => investigation::InvestigationCase::new_process(
                    finding.title.clone(),
                    self.revision,
                    automatic,
                    true,
                ),
            }
        };
        // Seed the case with the exact observation that prompted it, so the
        // model starts from a timestamped measurement, not a title.
        let baseline_kind = match &finding.target {
            Target::Process(..) => investigation::EvidenceKind::ProcessSample,
            Target::System if finding.id == "system:memory" => {
                investigation::EvidenceKind::ProcessSample
            }
            _ => investigation::EvidenceKind::VolumeContext,
        };
        case.add_evidence(investigation::evidence(
            "E1",
            baseline_kind,
            "initial measurement",
            format!(
                "Measured at {}: {}",
                care::timestamp(),
                ai::display_text(&finding.observation)
            ),
            &[],
            &[],
            investigation::EvidenceStatus::Complete,
        ));
        if finding.quick_win {
            case.add_evidence(investigation::evidence(
                "E2",
                investigation::EvidenceKind::Policy,
                "cleanup policy",
                format!(
                    "Rust cleanup policy classifies this measured target as a current low-disruption quick win: {}",
                    ai::display_text(&finding.consequence)
                ),
                &["rebuildable_data"],
                &[],
                investigation::EvidenceStatus::Complete,
            ));
        }
        case
    }
    /// What the run starts from: the subject handle, a measured description,
    /// and any action Rust would allow the model to suggest.
    fn agent_subject_for(&self, app: &App, finding: &Finding) -> agent::Subject {
        let folder = match &finding.target {
            Target::Cache(path) | Target::Folder(path) => Some(path.clone()),
            Target::System if finding.id == "system:disk" => Some(app.scan_root.clone()),
            _ => None,
        };
        let process = match &finding.target {
            Target::Process(pid, start) => Some((*pid, start.clone())),
            Target::System if finding.id == "system:memory" => app
                .processes
                .iter()
                .filter_map(|process| {
                    self.metrics
                        .rss_kb
                        .get(&process.pid)
                        .or(process.rss_kb.as_ref())
                        .map(|rss| (*rss, process))
                })
                .max_by_key(|(rss, _)| *rss)
                .map(|(_, process)| (process.pid, process.start_time.clone())),
            _ => None,
        };
        let place = folder
            .as_deref()
            .map(|path| agent_tools::short_path(path, &app.account_home))
            .unwrap_or_else(|| ai::display_text(&finding.title));
        let status = match &finding.target {
            Target::Cache(path) => app
                .entries
                .iter()
                .find(|entry| &entry.spec.path == path)
                .map(|entry| format!(" · cleanup rule {}", entry.status.label()))
                .unwrap_or_default(),
            _ => String::new(),
        };
        let actions = suggestion_id(app, &finding.target)
            .and_then(|id| {
                action_for_finding(app, &finding.target, false)
                    .ok()
                    .flatten()
                    .map(|action| {
                        (
                            id,
                            action
                                .description()
                                .lines()
                                .next()
                                .unwrap_or_default()
                                .to_string(),
                        )
                    })
            })
            .into_iter()
            .collect();
        agent::Subject {
            folder,
            process,
            description: format!(
                "{place}{}{status}",
                if finding.size_kb > 0 {
                    format!(" · {}", format_kb(finding.size_kb))
                } else {
                    String::new()
                }
            ),
            actions,
        }
    }
    fn awaiting_approval(&self) -> bool {
        self.agent
            .as_ref()
            .is_some_and(|run| run.approval_label().is_some())
    }
    fn model_ready(&self) -> bool {
        matches!(self.ai_framework, ai::FrameworkStatus::Available { .. })
    }
    fn start_investigation(&mut self, app: &App, finding: &Finding, automatic: bool) {
        if automatic && self.metrics.pressure.is_some_and(|level| level >= 2) {
            self.ai_error =
                Some("Automatic local AI paused while memory pressure is elevated.".into());
            return;
        }
        self.cancel_agent(app, "A new investigation replaced the previous one.");
        let mut case = self.case_for_finding(app, finding, automatic);
        let subject = self.agent_subject_for(app, finding);
        let use_model = self.model_ready() && self.metrics.pressure != Some(4);
        let run = agent::start(&mut case, subject, automatic, use_model);
        self.agent = Some(run);
        self.agent_subject = Some(subject_for_finding(finding));
        self.investigation_case = Some(case);
        self.report = None;
        self.answer_open = false;
        self.ai_error = None;
        self.persist(app);
    }
    fn start_ask(&mut self, app: &App, question: &str) {
        let question: String = ai::display_text(question.trim())
            .chars()
            .take(agent::QUESTION_LIMIT)
            .collect();
        if question.is_empty() {
            return;
        }
        self.cancel_agent(app, "A question replaced the previous investigation.");
        let mut case = investigation::InvestigationCase::new_question(&question, self.revision);
        let run = agent::start(&mut case, agent::Subject::default(), false, true);
        self.agent = Some(run);
        self.agent_subject = None;
        self.investigation_case = Some(case);
        self.report = None;
        self.answer_open = true;
        self.detail = false;
        self.detail_scroll = 0;
        self.ai_error = None;
        self.persist(app);
    }
    /// Advance the running investigation and apply what it reports.
    fn pump_agent(&mut self, app: &mut App) {
        let Some(mut run) = self.agent.take() else {
            return;
        };
        let Some(mut case) = self.investigation_case.take() else {
            return;
        };
        let calls_before = case.tool_calls.len();
        let events = {
            let suggest = |target: &Target| suggestion_id(app, target);
            let still = |id: &str| {
                self.findings
                    .iter()
                    .any(|finding| suggestion_id(app, &finding.target).as_deref() == Some(id))
                    || app.entries.iter().any(|entry| {
                        suggestion_id(app, &Target::Cache(entry.spec.path.clone())).as_deref()
                            == Some(id)
                    })
            };
            let world = agent_tools::ToolWorld {
                home: &app.account_home,
                inventory: app.inventory.as_ref(),
                entries: &app.entries,
                processes: &app.processes,
                metrics: &self.metrics,
                findings: &self.findings,
                history: &self.history,
                volume: self.volume.as_ref(),
                online_research: self.online_research,
                subject_pid: self
                    .agent_subject
                    .as_ref()
                    .and_then(|subject| self.findings.iter().find(|f| f.id == subject.id))
                    .and_then(|f| match f.target {
                        Target::Process(pid, _) => Some(pid),
                        _ => None,
                    }),
                suggest: &suggest,
            };
            run.step(&mut case, &world, &still)
        };
        let mut finished = None;
        let mut changed = !events.is_empty();
        for event in events {
            match event {
                agent::Event::Merge(inventory) => {
                    if let Some(existing) = &mut app.inventory {
                        existing.children.extend(inventory.children);
                    } else {
                        app.inventory = Some(*inventory);
                    }
                }
                agent::Event::ApprovalNeeded => {
                    self.detail_scroll = 0;
                    self.note = Some(
                        "Local AI asked for a fixed 8-second read-only filesystem trace. Press a to approve, or Esc to skip it."
                            .into(),
                    );
                }
                agent::Event::Finished(finish) => finished = Some(finish),
            }
        }
        changed |= case.tool_calls.len() != calls_before;
        self.investigation_case = Some(case);
        match finished {
            Some(finish) => self.apply_finish(&run, finish),
            None => self.agent = Some(run),
        }
        if changed {
            self.persist(app);
        }
    }
    fn apply_finish(&mut self, run: &agent::AgentRun, finish: agent::Finish) {
        let subject = self.agent_subject.clone();
        let sampled_at = subject.as_ref().and_then(|subject| {
            (subject.id.starts_with("process:") || subject.id == "system:memory")
                .then_some(self.metrics.sampled_at)
        });
        if subject
            .as_ref()
            .is_some_and(|subject| !self.findings.iter().any(|f| supports_subject(f, subject)))
        {
            self.ai_error = Some(
                "Evidence changed while local AI was working. The investigation is kept in History; press i to look again."
                    .into(),
            );
            return;
        }
        self.ai_error = (!finish.notes.is_empty()).then(|| finish.notes.join(" "));
        self.report = Some(ReportView {
            case_id: run.case_id.clone(),
            subject,
            sampled_at,
            summary: finish.summary,
            cited: finish.cited,
            suggestions: finish.suggestions,
            by_model: finish.by_model,
        });
        self.flash = Some(Instant::now());
        if !run.automatic {
            self.note = Some(if self.report.as_ref().is_some_and(|r| !r.suggestions.is_empty()) {
                "Investigation finished. AI-suggested actions are marked; Space adds one to your plan."
            } else {
                "Investigation finished. Details show the evidence and conclusion."
            }
            .into());
        }
    }
    fn start_measure(&mut self, app: &App, path: PathBuf) {
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let home = app.account_home.clone();
        thread::spawn(move || {
            let inventory = StorageInventory::scan_with_cancel(&path, &home, &flag);
            if !flag.load(Ordering::Relaxed) {
                let _ = sender.send(inventory);
            }
        });
        self.measure = Some(MeasureWork { receiver, stop });
    }
    fn start_triage(&mut self) {
        if self.metrics.pressure == Some(4) {
            self.auto_requested = true;
            self.ai_error = Some("Local AI paused while memory pressure is critical.".into());
            return;
        }
        let findings: Vec<_> = self
            .findings
            .iter()
            .filter(|finding| !self.kept.contains(&finding.id))
            .take(8)
            .collect();
        if findings.is_empty() {
            return;
        }
        // Short opaque IDs are easier for the local model to reproduce exactly.
        // The app resolves them back to measured finding IDs after validation.
        let mut id_map = HashMap::new();
        let subjects = findings
            .into_iter()
            .enumerate()
            .map(|(index, finding)| {
                let mut subject = subject_for_finding(finding);
                let short_id = format!("item{}", index + 1);
                id_map.insert(short_id.clone(), subject.id);
                subject.id = short_id;
                subject
            })
            .collect();
        let request = ai::Request {
            revision: self.revision,
            subjects,
        };
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let input = request.clone();
        let worker = thread::spawn(move || {
            let _ = sender.send(ai::triage(&input, &flag));
        });
        self.triage_work = Some(TriageWork {
            receiver,
            worker: Some(worker),
            cancel,
            request,
            id_map,
        });
        self.ai_error = None;
        self.auto_requested = true;
    }
    fn start_result_summary(&mut self, session: &Session) {
        if session.actions.is_empty() || self.metrics.pressure == Some(4) {
            return;
        }
        let request = ai::Request {
            revision: session.id,
            subjects: session
                .actions
                .iter()
                .enumerate()
                .map(|(index, action)| ai::Subject {
                    id: format!("result:{index}"),
                    title: "Completed maintenance action".into(),
                    observation: ai::display_text(&action.result),
                    consequence: "Compare the recorded before and after observations.".into(),
                    action_ids: vec![],
                    quick_win: false,
                    priority: 2,
                    disruption: "completed".into(),
                })
                .collect(),
        };
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let input = request.clone();
        let worker = thread::spawn(move || {
            let _ = sender.send(ai::summarize(&input, &flag));
        });
        self.result_work = Some(InsightWork {
            receiver,
            worker: Some(worker),
            cancel,
            request,
        });
    }
    fn add_action(&mut self, action: Action) {
        if self.plan.iter().any(|a| a.id() == action.id()) {
            self.plan.retain(|a| a.id() != action.id());
            self.signals_ack = false;
            self.moves_ack = false;
            return;
        }
        if (!self.plan.is_empty() && matches!(action, Action::ReviewClean(_)))
            || self
                .plan
                .iter()
                .any(|a| matches!(a, Action::ReviewClean(_)))
        {
            self.note=Some("Protected review data requires a separate, single-item DELETE confirmation. Finish or clear the current plan first.".into());
            return;
        }
        if self.plan.iter().any(|a| match (a.path(), action.path()) {
            (Some(a), Some(b)) => a.starts_with(b) || b.starts_with(a),
            _ => false,
        }) {
            self.note = Some(
                "This target overlaps an existing plan action. Remove that action first.".into(),
            );
            return;
        }
        if matches!(&action,Action::Signal(p,_) if self.plan.iter().any(|a|matches!(a,Action::Signal(other,_) if p.pid==other.pid)))
        {
            self.note = Some("A signal for this process is already in the plan.".into());
            return;
        }
        self.plan.push(action);
        self.acknowledgement.clear();
        self.signals_ack = false;
        self.moves_ack = false;
    }
    fn add_selected(&mut self, app: &mut App, force: bool) {
        if app.analysis_only {
            self.note = Some("This session is locked read-only.".into());
            return;
        }
        let Some(finding) = self.selected().cloned() else {
            return;
        };
        match action_for_finding(app, &finding.target, force) {
            Ok(Some(action)) => self.add_action(action),
            Ok(None) => {}
            Err(reason) => self.note = reason,
        }
    }
    fn execute(&mut self, app: &mut App) {
        if app.analysis_only || self.plan.is_empty() {
            return;
        }
        let needed = if self
            .plan
            .iter()
            .any(|a| matches!(a, Action::ReviewClean(_)))
        {
            "DELETE"
        } else if self
            .plan
            .iter()
            .any(|a| matches!(a, Action::Signal(..) | Action::Move(_)))
        {
            "APPLY"
        } else {
            "CLEAN"
        };
        if self.acknowledgement != needed {
            self.note = Some(format!("Type {needed} to confirm these exact actions."));
            return;
        }
        if self.plan.iter().any(|a| matches!(a, Action::Signal(..))) && !self.signals_ack {
            self.note =
                Some("Press s in review to acknowledge the process-signal consequences.".into());
            return;
        }
        if self.plan.iter().any(|a| matches!(a, Action::Move(..))) && !self.moves_ack {
            self.note = Some(
                "Press m in review to acknowledge the relocation and symlink consequences.".into(),
            );
            return;
        }
        self.cancel_agent(app, "Investigation stopped so the reviewed plan could run.");
        self.assessment = None;
        self.triage_work = None;
        self.result_work = None;
        self.measure = None;
        let actions = self.plan.clone();
        let home = app.account_home.clone();
        let root = app.scan_root.clone();
        let allowlist = app.specs.iter().map(|s| s.path.clone()).collect::<Vec<_>>();
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let session = Session::default();
        self.session = session.clone();
        thread::spawn(move || {
            execute_actions(actions, home, root, allowlist, flag, sender, session);
        });
        self.work = Some(Work { receiver, cancel });
        self.reviewing = false;
        self.note = None;
        self.stage = "Executing reviewed plan".into();
    }
}

impl Workspace {
    pub(super) fn load_proposal(&mut self, proposal: (PathBuf, crate::pending::PendingPlan)) {
        self.note = Some(format!(
            "Loading a cleanup proposal from {}. It is checked again when the assessment finishes; nothing runs without your confirmation.",
            ai::display_text(&proposal.1.client)
        ));
        self.proposal = Some(proposal);
    }
    /// Turn a proposal into plan actions, re-checking every target against
    /// the fresh measurements. Only actions eligible right now are added.
    fn apply_proposal(&mut self, app: &App) {
        let Some((path, plan)) = self.proposal.take() else {
            return;
        };
        if app.analysis_only {
            self.note = Some("This session is read-only; restart `diskray review` without --analyze to act on the proposal.".into());
            return;
        }
        let mut added = 0;
        let mut skipped = Vec::new();
        for proposed in &plan.actions {
            let target = Target::Cache(PathBuf::from(&proposed.path));
            let action = (suggestion_id(app, &target).as_deref() == Some(proposed.id.as_str()))
                .then(|| action_for_finding(app, &target, false).ok().flatten())
                .flatten();
            match action {
                Some(action) => {
                    let before = self.plan.len();
                    self.add_action(action);
                    added += usize::from(self.plan.len() > before);
                }
                None => skipped.push(ai::display_text(&proposed.label)),
            }
        }
        crate::pending::consume(&path);
        self.reviewing = added > 0;
        self.review_scroll = 0;
        self.note = Some(format!(
            "Proposal from {}{}: {added} action(s) added to your plan{}. Nothing has run; read each target and type the confirmation to proceed.",
            ai::display_text(&plan.client),
            if plan.reason.is_empty() {
                String::new()
            } else {
                format!(" (\u{201c}{}\u{201d})", ai::display_text(&plan.reason))
            },
            if skipped.is_empty() {
                String::new()
            } else {
                format!("; no longer eligible and left out: {}", skipped.join(", "))
            }
        ));
    }
    pub(super) fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }
    pub(super) fn failed(&self) -> bool {
        self.session
            .actions
            .iter()
            .any(|a| a.result.starts_with("Failed"))
            || self.session.state.starts_with("Worker stopped")
    }
    /// The Why screen's shared overview of this assessment.
    fn overview(&self, app: &App) -> crate::why::Overview {
        let findings: Vec<Finding> = self
            .findings
            .iter()
            .filter(|f| !self.kept.contains(&f.id))
            .cloned()
            .collect();
        crate::why::overview(
            &app.scan_root,
            &app.account_home,
            self.volume.as_ref(),
            app.inventory.as_ref(),
            &findings,
            5,
        )
    }
    fn palette_key(&mut self, app: &mut App, key: KeyEvent) {
        let Some((query, cursor)) = &mut self.palette else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.palette = None,
            KeyCode::Backspace => {
                query.pop();
                *cursor = 0;
            }
            KeyCode::Down => {
                *cursor = (*cursor + 1).min(palette_matches(query).len().saturating_sub(1))
            }
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Enter => {
                let chosen = palette_matches(query).get(*cursor).map(|command| command.2);
                self.palette = None;
                if let Some(code) = chosen {
                    self.key(app, KeyEvent::new(code, KeyModifiers::NONE));
                }
            }
            KeyCode::Char(c) if !c.is_control() && query.chars().count() < 40 => {
                query.push(c);
                *cursor = 0;
            }
            _ => {}
        }
    }
    fn navigate(&mut self, index: usize) {
        self.nav = index.min(2);
        self.screen = [Screen::Overview, Screen::Explore, Screen::History][self.nav];
        self.detail_scroll = 0;
        self.detail = false;
        self.coverage = false;
        self.help = false;
    }
    /// Switch tabs from a key or click. Explore opens at the home folder
    /// when the whole startup volume was scanned.
    fn go_to(&mut self, app: &mut App, index: usize) {
        self.navigate(index);
        self.focus = 1;
        if self.screen == Screen::Explore
            && app.explorer_path.is_none()
            && app.scan_root == Path::new("/")
            && app
                .inventory
                .as_ref()
                .is_some_and(|i| i.children.contains_key(&app.account_home))
        {
            app.explorer_path = Some(app.account_home.clone());
            app.explorer_cursor = 0;
        }
    }
    /// Add every ready quick win to the plan.
    fn add_quick_wins(&mut self, app: &mut App) {
        if app.analysis_only {
            self.note = Some(
                "This session is read-only; restart without --analyze to make changes.".into(),
            );
            return;
        }
        let wins: Vec<Target> = self
            .findings
            .iter()
            .filter(|f| f.quick_win && !self.kept.contains(&f.id))
            .map(|f| f.target.clone())
            .collect();
        let mut added = 0;
        for target in wins {
            if let Ok(Some(action)) = action_for_finding(app, &target, false)
                && !self.plan.iter().any(|queued| queued.id() == action.id())
            {
                self.add_action(action);
                added += 1;
            }
        }
        self.note = Some(if added == 0 {
            "All quick wins are already in your plan.".into()
        } else {
            format!("Added {added} quick win(s) to your plan. p reviews it; nothing has run.")
        });
    }
    fn inspect(&mut self, app: &mut App) {
        if let Some(finding) = self.selected().cloned() {
            match finding.target {
                Target::Folder(path) | Target::Cache(path) => {
                    app.explorer_path = Some(path.clone());
                    app.explorer_cursor = 0;
                    self.navigate(2);
                    if !app
                        .inventory
                        .as_ref()
                        .is_some_and(|i| i.children.contains_key(&path))
                    {
                        self.start_measure(app, path);
                    }
                }
                Target::Process(pid, _) => {
                    app.process_cursor = app
                        .processes
                        .iter()
                        .position(|p| p.pid == pid)
                        .unwrap_or_default();
                    app.phase = Phase::Processes;
                    self.legacy = true;
                }
                Target::System => {
                    self.detail = true;
                }
            }
        }
    }
    fn recheck(&mut self, app: &mut App) {
        self.cancel_agent(app, "Investigation stopped for a fresh assessment.");
        self.agent_subject = None;
        self.report = None;
        self.answer_open = false;
        self.auto_agent_started = false;
        self.auto_agent_resume = false;
        self.assessment = None;
        self.triage_work = None;
        self.result_work = None;
        self.measure = None;
        self.findings.clear();
        self.growth.clear();
        self.growth_since = None;
        app.entries.clear();
        app.inventory = None;
        self.volume = None;
        self.metrics = Metrics::default();
        self.trend.clear();
        self.kept.clear();
        self.result_insight = None;
        self.result_summary_session = None;
        self.triage = None;
        self.triage_revision = None;
        self.triage_is_ai = false;
        self.plan.clear();
        self.session = Session::default();
        self.started = Instant::now();
        self.progress = Default::default();
        self.assessment_elapsed = None;
        self.complete = false;
        self.auto_requested = false;
        self.stage = "Rechecking storage and activity".into();
        self.navigate(0);
        self.cursor = 0;
        self.user_moved = false;
        self.assessment = Some(care::assess(
            app.scan_root.clone(),
            app.account_home.clone(),
            app.include_reinstallable,
            app.tmp_retention_days,
        ));
    }
    pub(super) fn key(&mut self, app: &mut App, key: KeyEvent) -> bool {
        if (app.terminal_width < MIN_WIDTH || app.terminal_height < MIN_HEIGHT)
            && !matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
            && !(key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return true;
        }
        if key.kind != KeyEventKind::Press {
            return true;
        }
        // Any choice about the list keeps the selection from moving on its own.
        if matches!(
            key.code,
            KeyCode::Down
                | KeyCode::Up
                | KeyCode::Enter
                | KeyCode::Char('j' | 'k' | ' ' | 'i' | 'x' | 'e' | 'K' | 'd')
        ) {
            self.user_moved = true;
        }
        // A message stays until the next key; that key may set a new one.
        if self.work.is_none() && !matches!(key.code, KeyCode::Char('?')) {
            self.note = None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(work) = &self.work {
                work.cancel.store(true, Ordering::Relaxed);
                self.note = Some("Stopping after the current action finishes.".into());
            } else {
                app.quit = true;
            }
            return true;
        }
        if self.legacy {
            // Specialist forms own their input; the retained legacy menus are not drawn here.
            if matches!(
                key.code,
                KeyCode::F(_) | KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('?')
            ) || key.modifiers.contains(KeyModifiers::ALT)
            {
                return true;
            }
            if app.phase == Phase::Processes {
                match key.code {
                    KeyCode::Esc => {
                        self.legacy = false;
                        app.phase = Phase::Review;
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        app.process_cursor =
                            (app.process_cursor + 1).min(app.processes.len().saturating_sub(1))
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        app.process_cursor = app.process_cursor.saturating_sub(1)
                    }
                    KeyCode::Char(' ') | KeyCode::Char('d') | KeyCode::Char('x')
                        if !app.analysis_only =>
                    {
                        if let Some(process) = app
                            .processes
                            .get(app.process_cursor)
                            .filter(|p| p.signalable)
                        {
                            self.add_action(Action::Signal(
                                process.clone(),
                                if key.code == KeyCode::Char('x') {
                                    ProcessSignal::Kill
                                } else {
                                    ProcessSignal::Terminate
                                },
                            ));
                            self.legacy = false;
                            app.phase = Phase::Review;
                            self.reviewing = true;
                        }
                    }
                    KeyCode::Char('q') => app.quit = true,
                    _ => {}
                }
                return true;
            }
            return false;
        }
        if self.work.is_some() {
            if key.code == KeyCode::Esc
                && let Some(work) = &self.work
            {
                work.cancel.store(true, Ordering::Relaxed);
                self.note =
                    Some("Stop requested. Waiting for the current action to finish safely.".into());
            }
            return true;
        }
        if self.help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Backspace => {
                    self.help = false;
                    self.detail_scroll = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.detail_scroll = self.detail_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(5),
                KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(5),
                _ => {}
            }
            return true;
        }
        if self.palette.is_some() {
            self.palette_key(app, key);
            return true;
        }
        if let Some(text) = &mut self.asking {
            match key.code {
                KeyCode::Esc => self.asking = None,
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Enter => {
                    let question = text.trim().to_string();
                    if !question.is_empty() {
                        self.asking = None;
                        self.start_ask(app, &question);
                    }
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && text.chars().count() < agent::QUESTION_LIMIT =>
                {
                    text.push(c)
                }
                _ => {}
            }
            return true;
        }
        if self.clearing_history {
            if app.analysis_only {
                self.clearing_history = false;
                return true;
            }
            match key.code {
                KeyCode::Esc => {
                    self.clearing_history = false;
                    self.acknowledgement.clear();
                }
                KeyCode::Backspace => {
                    self.acknowledgement.pop();
                }
                KeyCode::Char(c) if c.is_ascii_alphabetic() && self.acknowledgement.len() < 5 => {
                    self.acknowledgement.push(c)
                }
                KeyCode::Enter if self.acknowledgement == "CLEAR" => {
                    match care::clear_sessions(&app.account_home) {
                        Ok(count) => {
                            self.history.clear();
                            self.report = None;
                            self.note = Some(format!(
                                "Cleared {count} session records. The deletion audit log is retained."
                            ));
                        }
                        Err(e) => self.note = Some(format!("History could not be cleared: {e}")),
                    };
                    self.clearing_history = false;
                    self.acknowledgement.clear();
                }
                _ => {}
            }
            return true;
        }
        if self.reviewing {
            match key.code {
                KeyCode::Esc => {
                    self.reviewing = false;
                    self.acknowledgement.clear();
                }
                KeyCode::Char('s')
                    if self
                        .plan
                        .iter()
                        .any(|action| matches!(action, Action::Signal(..))) =>
                {
                    self.signals_ack = !self.signals_ack;
                    self.note = Some(if self.signals_ack {
                        "Process-signal consequences acknowledged for this exact plan.".into()
                    } else {
                        "Process-signal acknowledgment removed.".into()
                    });
                }
                KeyCode::Char('m')
                    if self
                        .plan
                        .iter()
                        .any(|action| matches!(action, Action::Move(..))) =>
                {
                    self.moves_ack = !self.moves_ack;
                    self.note = Some(if self.moves_ack {
                        "Relocation consequences acknowledged for this exact plan.".into()
                    } else {
                        "Relocation acknowledgment removed.".into()
                    });
                }
                KeyCode::Enter => self.execute(app),
                KeyCode::Backspace => {
                    self.acknowledgement.pop();
                }
                KeyCode::Char(c) if c.is_ascii_alphabetic() && self.acknowledgement.len() < 8 => {
                    self.acknowledgement.push(c)
                }
                KeyCode::Delete => {
                    self.plan.clear();
                    self.reviewing = false;
                    self.acknowledgement.clear();
                    self.signals_ack = false;
                    self.moves_ack = false;
                }
                KeyCode::Down | KeyCode::PageDown => {
                    self.review_scroll = self.review_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::PageUp => {
                    self.review_scroll = self.review_scroll.saturating_sub(1)
                }
                _ => {}
            }
            return true;
        }
        if self.focus == 0 {
            match key.code {
                KeyCode::Left | KeyCode::Up => self.navigate(self.nav.saturating_sub(1)),
                KeyCode::Right | KeyCode::Down => self.navigate((self.nav + 1).min(3)),
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Enter | KeyCode::Esc => self.focus = 1,
                KeyCode::Char('q') => app.quit = true,
                _ => {}
            }
            return true;
        }
        if self.screen == Screen::History && key.code == KeyCode::Delete && !app.analysis_only {
            self.clearing_history = true;
            self.acknowledgement.clear();
            return true;
        }
        if self.screen == Screen::History && key.code == KeyCode::Char('i') {
            return true;
        }
        if self.awaiting_approval() {
            match key.code {
                KeyCode::Down | KeyCode::PageDown => {
                    self.detail_scroll = self.detail_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::PageUp => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(1)
                }
                KeyCode::Char('?') => {
                    self.help = true;
                    self.detail_scroll = 0;
                }
                KeyCode::Char('a') => {
                    if let (Some(run), Some(case)) = (&mut self.agent, &mut self.investigation_case)
                    {
                        run.approve(case);
                    }
                    self.note = Some(
                        "Approved the fixed 8-second read-only trace. macOS authorization may be required."
                            .into(),
                    );
                    self.persist(app);
                }
                KeyCode::Esc => {
                    if let (Some(run), Some(case)) = (&mut self.agent, &mut self.investigation_case)
                    {
                        run.decline(
                            case,
                            "The operator declined administrator-assisted tracing.",
                        );
                    }
                    self.note = Some(
                        "Administrator-assisted tracing skipped. The case will continue with other evidence."
                            .into(),
                    );
                    self.persist(app);
                }
                _ => {}
            }
            return true;
        }
        if self.answer_open
            && let KeyCode::Char(digit @ '1'..='3') = key.code
        {
            let question = self
                .investigation_case
                .as_ref()
                .and_then(|case| case.question.clone())
                .unwrap_or_default();
            if let Some(follow_up) = follow_ups(&question).get(digit as usize - '1' as usize) {
                self.answer_open = false;
                self.asking = Some((*follow_up).to_string());
            }
            return true;
        }
        if self.answer_open && matches!(key.code, KeyCode::Esc | KeyCode::Backspace | KeyCode::Left)
        {
            self.answer_open = false;
            self.detail_scroll = 0;
            return true;
        }
        if self.answer_open
            && matches!(
                key.code,
                KeyCode::Down | KeyCode::Up | KeyCode::Char('j') | KeyCode::Char('k')
            )
        {
            self.detail_scroll = if matches!(key.code, KeyCode::Down | KeyCode::Char('j')) {
                self.detail_scroll.saturating_add(1)
            } else {
                self.detail_scroll.saturating_sub(1)
            };
            return true;
        }
        if (self.detail || self.coverage)
            && matches!(key.code, KeyCode::Esc | KeyCode::Backspace | KeyCode::Left)
        {
            if self.coverage {
                self.coverage = false;
            } else {
                self.detail = false;
            }
            self.detail_scroll = 0;
            return true;
        }
        if (self.detail || self.coverage)
            && matches!(
                key.code,
                KeyCode::Down | KeyCode::Up | KeyCode::Char('j') | KeyCode::Char('k')
            )
        {
            self.detail_scroll = if matches!(key.code, KeyCode::Down | KeyCode::Char('j')) {
                self.detail_scroll.saturating_add(1)
            } else {
                self.detail_scroll.saturating_sub(1)
            };
            return true;
        }
        if self.coverage
            && !matches!(
                key.code,
                KeyCode::Char('v' | '?' | 'A')
                    | KeyCode::PageDown
                    | KeyCode::PageUp
                    | KeyCode::Tab
                    | KeyCode::BackTab
            )
        {
            return true;
        }
        if key.code == KeyCode::Char('v') {
            self.coverage = !self.coverage;
            self.detail_scroll = 0;
            return true;
        }
        if key.code == KeyCode::Char('f') && self.screen == Screen::Explore {
            let path = app
                .explorer_items()
                .get(app.explorer_cursor)
                .map(|i| i.path.clone());
            if let Some(id) = self
                .findings
                .iter()
                .find(|f| matches!((&f.target,&path),(Target::Cache(a),Some(b)) if a==b))
                .map(|f| f.id.clone())
            {
                self.kept.remove(&id);
                self.show_all = true;
                self.navigate(1);
                self.cursor = self
                    .visible()
                    .iter()
                    .position(|f| f.id == id)
                    .unwrap_or_default();
            } else {
                self.note=Some("No direct cleanup rule for this path. Inspect its children or move useful data with m.".into());
            }
            return true;
        }
        if self.coverage && key.code == KeyCode::Esc {
            self.coverage = false;
            return true;
        }
        if key.code == KeyCode::Char('i') && self.screen == Screen::Explore {
            if let Some(path) = app
                .explorer_items()
                .get(app.explorer_cursor)
                .map(|item| item.path.clone())
            {
                self.start_measure(app, path);
            }
            return true;
        }
        match key.code {
            KeyCode::Char('q') => app.quit = true,
            KeyCode::Char(digit @ '1'..='3') => self.go_to(app, digit as usize - '1' as usize),
            KeyCode::Char(':') => self.palette = Some((String::new(), 0)),
            KeyCode::Tab => self.go_to(app, (self.nav + 1) % 3),
            KeyCode::BackTab => self.go_to(app, (self.nav + 2) % 3),
            KeyCode::Char('a') if self.screen == Screen::Overview => self.add_quick_wins(app),
            KeyCode::Char('p') => {
                if self.agent.as_ref().is_some_and(|run| run.automatic) {
                    self.cancel_agent(
                        app,
                        "Automatic investigation stopped while you review your plan.",
                    );
                }
                self.reviewing = true;
                self.review_scroll = 0;
                self.acknowledgement.clear();
                self.signals_ack = false;
                self.moves_ack = false;
            }
            KeyCode::Char('M') => self.motion = !self.motion,
            KeyCode::Char('R') => {
                let enabled = !self.online_research;
                match care::set_online_research(&app.account_home, enabled) {
                    Ok(()) => {
                        self.online_research = enabled;
                        self.note=Some(if enabled{"Online research enabled. The app may fetch only its fixed Apple documentation URLs; local paths, traces, process arguments, and generated queries are excluded."}else{"Online research disabled. Bundled reference notes remain available."}.into())
                    }
                    Err(error) => {
                        self.note = Some(format!("Research preference could not be saved: {error}"))
                    }
                }
            }
            KeyCode::Char('A') => {
                thread::spawn(|| {
                    let _ = Command::new("/usr/bin/open")
                        .args(["-b", "com.apple.systempreferences"])
                        .status();
                });
            }
            KeyCode::Char('d') => {
                self.detail = !self.detail;
                self.detail_scroll = 0;
            }
            KeyCode::Char('r') => {
                if self.plan.is_empty() {
                    self.recheck(app);
                } else {
                    self.note = Some("Review or clear the pending plan before rechecking.".into());
                }
            }
            KeyCode::Char('?') => {
                self.help = true;
                self.detail_scroll = 0;
            }
            KeyCode::Char('P') => {
                app.phase = Phase::Processes;
                self.legacy = true;
            }
            KeyCode::Char('i') => {
                if let Some(finding) = self.selected().cloned() {
                    self.start_investigation(app, &finding, false);
                    // Narrow screens have no side pane, so show the
                    // investigation as it runs instead of finishing unseen.
                    if app.terminal_width < SPLIT_WIDTH {
                        self.detail = true;
                        self.detail_scroll = 0;
                    }
                    if self.agent.as_ref().is_some_and(agent::AgentRun::by_model) {
                        self.note = Some(
                            "Local AI is investigating with read-only tools. The timeline appears in details.".into(),
                        );
                    }
                }
            }
            KeyCode::Char('/') => {
                if self.model_ready() {
                    self.asking = Some(String::new());
                    self.help = false;
                } else {
                    self.note = Some(format!(
                        "Ask needs Apple Intelligence on this Mac. {}",
                        self.ai_framework.description()
                    ));
                }
            }
            KeyCode::Char('f') => {
                self.show_all = !self.show_all;
                self.pinned = None;
                self.cursor = 0;
            }
            KeyCode::Char('K') => {
                if let Some(f) = self.selected() {
                    if matches!(f.target, Target::System) {
                        self.note =
                            Some("System pressure stays visible until readings change.".into());
                    } else {
                        self.kept.insert(f.id.clone());
                    }
                }
                self.cursor = self.cursor.min(self.visible().len().saturating_sub(1));
            }
            KeyCode::Char('e') if self.screen == Screen::Overview => self.inspect(app),
            KeyCode::Char('e') if self.screen == Screen::Explore => {
                self.detail = false;
                self.detail_scroll = 0;
                app.open_consumer();
            }
            KeyCode::Char(' ') if self.screen == Screen::Overview => self.add_selected(app, false),
            KeyCode::Char('x') if self.screen == Screen::Overview => self.add_selected(app, true),
            KeyCode::Char('m') => {
                if !app.analysis_only {
                    app.open_relocation_sources();
                    self.legacy = true;
                }
            }
            KeyCode::Char('o') => {
                let path = if self.screen == Screen::Explore {
                    app.explorer_items()
                        .get(app.explorer_cursor)
                        .map(|i| i.path.clone())
                } else {
                    self.selected().and_then(|f| match &f.target {
                        Target::Cache(p) | Target::Folder(p) => Some(p.clone()),
                        _ => None,
                    })
                };
                if let Some(path) = path {
                    thread::spawn(move || {
                        let _ = Command::new("/usr/bin/open").arg("-R").arg(path).status();
                    });
                }
            }
            KeyCode::Down | KeyCode::Char('j') => match self.screen {
                Screen::Overview => {
                    self.cursor = (self.cursor + 1).min(self.visible().len().saturating_sub(1))
                }
                Screen::Explore => {
                    app.explorer_cursor =
                        (app.explorer_cursor + 1).min(app.explorer_items().len().saturating_sub(1))
                }
                Screen::History => {
                    self.history_cursor =
                        (self.history_cursor + 1).min(self.history.len().saturating_sub(1))
                }
            },
            KeyCode::Up | KeyCode::Char('k') => match self.screen {
                Screen::Overview => self.cursor = self.cursor.saturating_sub(1),
                Screen::Explore => app.explorer_cursor = app.explorer_cursor.saturating_sub(1),
                Screen::History => self.history_cursor = self.history_cursor.saturating_sub(1),
            },
            KeyCode::Enter => {
                if self.screen == Screen::Explore && !self.detail {
                    if app
                        .explorer_items()
                        .get(app.explorer_cursor)
                        .is_some_and(|item| item.kind != StorageItemKind::Directory)
                    {
                        self.detail = true;
                    } else {
                        app.open_consumer();
                    }
                    self.detail_scroll = 0;
                } else {
                    self.detail = true;
                    self.detail_scroll = 0;
                }
            }
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Left => {
                if self.screen == Screen::Explore {
                    app.explorer_back();
                } else if key.code == KeyCode::Esc && self.agent.is_some() {
                    self.cancel_agent(
                        app,
                        "Investigation stopped. Evidence collected so far is kept.",
                    );
                } else {
                    self.note = None;
                    self.detail = false;
                }
            }
            KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(5),
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(5),
            _ => {}
        }
        if matches!(
            key.code,
            KeyCode::Down | KeyCode::Up | KeyCode::Char('j') | KeyCode::Char('k')
        ) {
            self.detail_scroll = 0;
        }
        true
    }
    pub(super) fn mouse(&mut self, app: &mut App, event: MouseEvent) -> bool {
        if app.terminal_width < MIN_WIDTH || app.terminal_height < MIN_HEIGHT {
            return true;
        }
        if self.legacy && event.row < 3 {
            return true;
        }
        if self.legacy && app.phase == Phase::Processes {
            return true;
        }
        if self.legacy {
            return false;
        }
        let modal = self.reviewing
            || self.clearing_history
            || self.work.is_some()
            || self.help
            || self.asking.is_some()
            || self.palette.is_some()
            || self.awaiting_approval();
        let key = match event.kind {
            MouseEventKind::ScrollDown => Some(KeyCode::Down),
            MouseEventKind::ScrollUp => Some(KeyCode::Up),
            _ => None,
        };
        if let Some(key) = key {
            return self.key(app, KeyEvent::new(key, KeyModifiers::NONE));
        }
        if event.kind != MouseEventKind::Down(MouseButton::Left) {
            return true;
        }
        let control = self
            .hits
            .borrow()
            .iter()
            .find(|(r, _)| {
                event.column >= r.x
                    && event.column < r.right()
                    && event.row >= r.y
                    && event.row < r.bottom()
            })
            .map(|(_, c)| *c);
        if modal && !matches!(control, Some(Control::Key(_))) {
            return true;
        }
        if control.is_none() {
            let path = app.hit_regions.borrow().iter().find_map(|(rect, hit)| {
                if rect_contains(*rect, event.column, event.row) {
                    if let HitTarget::MapNode(index) = hit {
                        app.map_paths.borrow().get(*index).cloned()
                    } else {
                        None
                    }
                } else {
                    None
                }
            });
            if let Some(path) = path {
                app.open_map_path(&path);
                self.navigate(2);
                return true;
            }
        }
        match control {
            Some(Control::Nav(index)) => self.go_to(app, index),
            Some(Control::Row(index)) => {
                self.user_moved = true;
                self.detail_scroll = 0;
                match self.screen {
                    Screen::Overview => self.cursor = index,
                    Screen::Explore => app.explorer_cursor = index,
                    Screen::History => self.history_cursor = index,
                }
            }
            Some(Control::Key(code)) => {
                self.key(app, KeyEvent::new(code, KeyModifiers::NONE));
            }
            _ => {}
        }
        true
    }
}

fn text_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    title: &str,
    text: String,
    scroll: u16,
) {
    let text = Paragraph::new(text)
        .style(Style::default().fg(app.color(INK)))
        .wrap(Wrap { trim: false });
    let mut block = panel(app, title);
    let inner = block.inner(area);
    let count = text.line_count(inner.width).min(u16::MAX as usize) as u16;
    let max_scroll = count.saturating_sub(inner.height);
    let scroll = scroll.min(max_scroll);
    if max_scroll > 0 {
        block = block.title_bottom(Line::from(format!(
            " PgUp/PgDn · {}–{} of {} lines ",
            scroll + 1,
            (scroll + inner.height).min(count),
            count
        )));
    }
    frame.render_widget(text.block(block).scroll((scroll, 0)), area);
}
fn button(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    w: &Workspace,
    label: String,
    control: Control,
    active: bool,
) {
    let style = if active {
        selected_row_style(app).fg(app.color(BLUE))
    } else {
        Style::default().fg(app.color(MUTED))
    };
    frame.render_widget(Paragraph::new(label).style(style), area);
    w.hits.borrow_mut().push((area, control));
}
fn execute_actions(
    mut actions: Vec<Action>,
    home: PathBuf,
    root: PathBuf,
    allowlist: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
    sender: mpsc::Sender<WorkMessage>,
    mut session: Session,
) {
    fn checkpoint(home: &Path, session: &mut Session, sender: &mpsc::Sender<WorkMessage>) {
        session.updated = care::timestamp();
        if let Err(e) = care::save_session(home, session) {
            session.state = format!("{} · history unavailable: {e}", session.state);
        }
        let _ = sender.send(WorkMessage::Progress(session.clone()));
    }
    fn sample_window(cancel: &AtomicBool, mut progress: impl FnMut(usize)) -> Option<Metrics> {
        let mut sampler = care::Sampler::default();
        let mut weighted = HashMap::<u32, f64>::new();
        let mut swap_in = Some(0.);
        let mut swap_out = Some(0.);
        let mut disk_weighted = HashMap::<String, f64>::new();
        let mut elapsed = 0.;
        let mut result = None;
        for index in 0..6 {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let metrics = sampler.sample(&review_processes().ok()?);
            if metrics.error.is_some() {
                return None;
            }
            elapsed += metrics.interval_secs;
            for (pid, cpu) in &metrics.cpu {
                *weighted.entry(*pid).or_default() += cpu * metrics.interval_secs;
            }
            if index > 0 {
                swap_in = swap_in
                    .zip(metrics.swap_in_kb_s)
                    .map(|(sum, rate)| sum + rate * metrics.interval_secs);
                swap_out = swap_out
                    .zip(metrics.swap_out_kb_s)
                    .map(|(sum, rate)| sum + rate * metrics.interval_secs);
                if index == 1 {
                    disk_weighted = metrics
                        .disk_mb_s
                        .iter()
                        .map(|(name, rate)| (name.clone(), rate * metrics.interval_secs))
                        .collect();
                } else {
                    disk_weighted.retain(|name, total| {
                        if let Some(rate) = metrics.disk_mb_s.get(name) {
                            *total += rate * metrics.interval_secs;
                            true
                        } else {
                            false
                        }
                    });
                }
            }
            result = Some(metrics);
            progress(index + 1);
            if index < 5 {
                for _ in 0..20 {
                    if cancel.load(Ordering::Relaxed) {
                        return None;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
        let mut result = result?;
        result.interval_secs = elapsed;
        result.cpu = weighted
            .into_iter()
            .map(|(pid, value)| (pid, if elapsed > 0. { value / elapsed } else { 0. }))
            .collect();
        result.swap_in_kb_s = swap_in
            .filter(|_| elapsed > 0.)
            .map(|total| total / elapsed);
        result.swap_out_kb_s = swap_out
            .filter(|_| elapsed > 0.)
            .map(|total| total / elapsed);
        result.disk_mb_s = disk_weighted
            .into_iter()
            .filter(|_| elapsed > 0.)
            .map(|(name, total)| (name, total / elapsed))
            .collect();
        Some(result)
    }

    // Activity readings only matter when the plan stops processes; storage
    // results are verified by measured removal and free space instead.
    let signals = actions.iter().any(|a| matches!(a, Action::Signal(..)));
    if signals {
        session.state = "Measuring activity before stopping processes".into();
        checkpoint(&home, &mut session, &sender);
        session.before = sample_window(&cancel, |completed| {
            let mut update = session.clone();
            update.state =
                format!("Measuring activity before stopping processes · sample {completed}/6");
            let _ = sender.send(WorkMessage::Progress(update));
        });
    }
    session.free_before_kb =
        crate::storage::read_volume_stats(&crate::storage::accounting_path(&root, &home))
            .ok()
            .map(|v| v.disk_free_kb());
    let before_processes = review_processes().ok();
    let mut signalled = Vec::new();
    let mut cleaned = Vec::new();
    actions.sort_by_key(|a| {
        if matches!(a, Action::Signal(..)) {
            0
        } else {
            1
        }
    });
    for action in actions {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        session.state = format!(
            "Running {}",
            action.description().lines().next().unwrap_or("action")
        );
        checkpoint(&home, &mut session, &sender);
        let review = matches!(action, Action::ReviewClean(_));
        let (target, result, removed_kb) = match action {
            Action::Clean(mut entry) | Action::ReviewClean(mut entry) => {
                let target = entry.spec.path.display().to_string();
                let outcome = if review {
                    clean_review_data(&mut entry, &allowlist, &Whitelist::load(&home))
                } else {
                    clean_cache(&mut entry, &allowlist, true, &Whitelist::load(&home))
                };
                crate::history::record(&home, &entry, &outcome);
                if matches!(outcome, CleanupOutcome::Cleared { .. }) {
                    cleaned.push((session.actions.len(), entry.spec.clone(), entry.size_kb));
                }
                match outcome {
                    CleanupOutcome::Cleared { removed_kb, method } => (
                        target,
                        format!("Cleared · {}", method.explanation()),
                        removed_kb,
                    ),
                    CleanupOutcome::SafetySkipped(reason) => {
                        (target, format!("Skipped: {reason}"), 0)
                    }
                    CleanupOutcome::Failed { error, removed_kb } => {
                        (target, format!("Failed: {error}"), removed_kb)
                    }
                }
            }
            Action::Signal(mut process, signal) => {
                signalled.push((session.actions.len(), process.clone()));
                let outcome = signal_process(&mut process, signal);
                (
                    format!(
                        "PID {} · {}",
                        process.pid,
                        ai::display_text(&process.command)
                    ),
                    format!("{outcome:?}"),
                    0,
                )
            }
            Action::Move(plan) => {
                let report = relocation::execute(&plan);
                (
                    plan.source.display().to_string(),
                    format!(
                        "{:?}: {}",
                        report.status,
                        report.message.unwrap_or_default()
                    ),
                    0,
                )
            }
        };
        session.actions.push(RecordedAction {
            target,
            result,
            removed_kb,
        });
        checkpoint(&home, &mut session, &sender);
    }
    session.state = "Verifying results".into();
    checkpoint(&home, &mut session, &sender);
    if signals {
        session.after = sample_window(&cancel, |completed| {
            let mut update = session.clone();
            update.state = format!("Verifying results · sample {completed}/6");
            let _ = sender.send(WorkMessage::Progress(update));
        });
    }
    if !cancel.load(Ordering::Relaxed) {
        let after_processes = review_processes().ok();
        for (index, process) in signalled {
            if let Some(record) = session.actions.get_mut(index) {
                let observed = match (&before_processes, &after_processes) {
                    (Some(before), Some(after)) => {
                        let still_present = after
                            .iter()
                            .any(|p| p.pid == process.pid && p.start_time == process.start_time);
                        let newly_matching = after
                            .iter()
                            .filter(|p| {
                                p.command == process.command
                                    && !before.iter().any(|old| {
                                        old.pid == p.pid && old.start_time == p.start_time
                                    })
                            })
                            .map(|p| p.pid.to_string())
                            .collect::<Vec<_>>();
                        format!(
                            "Verification: original identity {}. {}",
                            if still_present {
                                "still observed"
                            } else {
                                "not observed"
                            },
                            if newly_matching.is_empty() {
                                "No new matching executable observed.".into()
                            } else {
                                format!(
                                    "New matching executable at PID {}; a restart is possible, not proven.",
                                    newly_matching.join(", ")
                                )
                            }
                        )
                    }
                    _ => "Process verification unavailable.".into(),
                };
                record.result.push_str(&format!("\n{observed}"));
            }
        }
        for (index, spec, after_cleanup) in cleaned {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let current = cache::scan_cache(&spec, true);
            if let Some(record) = session.actions.get_mut(index) {
                record.result.push_str(&if matches!(
                    current.status,
                    CacheStatus::ScanError | CacheStatus::Symlink | CacheStatus::Invalid
                ) {
                    "\nSize verification unavailable.".into()
                } else {
                    format!(
                        "\nRecheck: {} remains. {}",
                        format_kb(current.size_kb),
                        if current.size_kb > after_cleanup {
                            "Growth observed after cleanup; the writer is unknown."
                        } else {
                            "No measured regrowth."
                        }
                    )
                });
            }
        }
    }
    session.free_after_kb =
        crate::storage::read_volume_stats(&crate::storage::accounting_path(&root, &home))
            .ok()
            .map(|v| v.disk_free_kb());
    session.state = if cancel.load(Ordering::Relaxed) {
        "Stopped · verification incomplete"
    } else if session.before.is_none() || session.after.is_none() {
        "Completed actions · performance verification unavailable"
    } else {
        "Completed · observations recorded"
    }
    .into();
    checkpoint(&home, &mut session, &sender);
    let _ = sender.send(WorkMessage::Finished(session));
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    fn fixture() -> (tempfile::TempDir, App, Workspace) {
        let home = tempfile::tempdir().unwrap();
        let cli = Cli::parse_from(["diskray"]);
        let mut app = App::new(&cli, home.path()).unwrap();
        app.phase = Phase::Review;
        app.no_color = false;
        app.retention_worker = None;
        let spec = scan_specs(home.path(), home.path())
            .into_iter()
            .find(|s| s.label == "Python cache")
            .unwrap();
        fs::create_dir_all(&spec.path).unwrap();
        app.entries.push(CacheEntry {
            identity: PathIdentity::capture(&spec.path),
            spec,
            status: CacheStatus::Ready,
            size_kb: 1_048_576,
            outcome: None,
        });
        let mut w = Workspace::empty();
        w.auto_requested = true;
        w.complete = true;
        w.stage = "Assessment complete · synthetic preview".into();
        w.rebuild(&app);
        w.navigate(0);
        (home, app, w)
    }
    fn press(w: &mut Workspace, app: &mut App, key: KeyCode) {
        assert!(w.key(app, KeyEvent::new(key, KeyModifiers::NONE)));
    }
    fn report_for(finding: &Finding, summary: &str, suggestions: Vec<String>) -> ReportView {
        ReportView {
            case_id: "case".into(),
            subject: Some(subject_for_finding(finding)),
            sampled_at: None,
            summary: summary.into(),
            cited: vec!["E1".into()],
            suggestions,
            by_model: true,
        }
    }
    /// Tick until the running investigation finishes.
    fn settle(w: &mut Workspace, app: &mut App) {
        let started = Instant::now();
        while w.agent.is_some() && started.elapsed() < Duration::from_secs(20) {
            w.tick(app);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(w.agent.is_none(), "the investigation should finish");
    }

    fn screen_text(app: &App, w: &Workspace, width: u16, height: u16, preview: &str) -> String {
        let mut terminal =
            Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), app, w))
            .unwrap();
        let buffer = terminal.backend().buffer();
        if let Some(dir) = std::env::var_os("CARE_PREVIEW_DIR") {
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                PathBuf::from(dir).join(format!("{preview}-{width}.svg")),
                super::super::tests::buffer_svg(buffer),
            )
            .unwrap();
        }
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn finding_details_have_the_same_keys_at_every_width() {
        for width in [60, 80, 120, 160] {
            let (_home, mut app, mut w) = fixture();
            app.terminal_width = width;
            press(&mut w, &mut app, KeyCode::Enter);
            assert!(w.detail);
            assert!(w.screen == Screen::Overview);
            let cursor = w.cursor;
            press(&mut w, &mut app, KeyCode::Down);
            assert_eq!(
                w.cursor, cursor,
                "Scrolling details must not change the subject"
            );
            assert_eq!(w.detail_scroll, 1);
            press(&mut w, &mut app, KeyCode::Esc);
            assert!(!w.detail);
            assert_eq!(w.detail_scroll, 0);
            w.detail = true;
            w.coverage = true;
            w.navigate(1);
            assert!(!w.detail && !w.coverage);
        }
    }

    #[test]
    fn compact_explore_never_shows_an_unrelated_finding_or_ai_explanation() {
        let (home, mut app, mut w) = fixture();
        let root = fs::canonicalize(home.path()).unwrap();
        let docs = root.join("Unique documents");
        fs::create_dir(&docs).unwrap();
        fs::write(docs.join("notes.txt"), b"important").unwrap();
        app.inventory = Some(StorageInventory::scan(home.path(), home.path()));
        app.explorer_path = Some(root.clone());
        app.explorer_cursor = app
            .explorer_items()
            .iter()
            .position(|i| i.path == docs)
            .unwrap();
        w.report = Some(report_for(
            w.selected().unwrap(),
            "UNRELATED CACHE EXPLANATION",
            vec![],
        ));
        w.navigate(1);
        press(&mut w, &mut app, KeyCode::Char('d'));
        let text = screen_text(&app, &w, 60, 24, "explore-details");
        assert!(text.contains("Unique documents"));
        assert!(!text.contains("Python cache"));
        assert!(!text.contains("UNRELATED CACHE"));
        press(&mut w, &mut app, KeyCode::Esc);
        assert_eq!(
            app.explorer_path.as_deref(),
            Some(root.as_path()),
            "Back closes details before changing folders"
        );
        press(&mut w, &mut app, KeyCode::Enter);
        assert_eq!(app.explorer_path.as_deref(), Some(docs.as_path()));
        press(&mut w, &mut app, KeyCode::Enter);
        assert!(w.detail, "Enter on a file must open useful details");
        let text = screen_text(&app, &w, 60, 24, "file-details");
        assert!(text.contains("notes.txt"));
        assert!(text.contains("o shows it in Finder"), "{text}");
    }

    #[test]
    fn history_opens_results_and_navigation_closes_them() {
        let (_home, mut app, mut w) = fixture();
        w.history.push(Session {
            state: "Completed fixture actions".into(),
            ..Default::default()
        });
        w.navigate(2);
        press(&mut w, &mut app, KeyCode::Enter);
        assert!(w.detail);
        let text = screen_text(&app, &w, 60, 16, "history-results");
        assert!(text.contains("RESULTS"));
        assert!(text.contains("Completed fixture actions"));
        press(&mut w, &mut app, KeyCode::Esc);
        assert!(!w.detail);
    }

    #[test]
    fn help_is_scrollable_and_captures_actions_and_navigation_clicks() {
        let (_home, mut app, mut w) = fixture();
        press(&mut w, &mut app, KeyCode::Char('?'));
        let text = screen_text(&app, &w, 60, 16, "help");
        assert!(text.contains("HELP"));
        press(&mut w, &mut app, KeyCode::Char(' '));
        assert!(w.plan.is_empty());
        w.mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 16,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(w.help);
        assert!(w.screen == Screen::Overview);
        press(&mut w, &mut app, KeyCode::PageDown);
        assert_eq!(w.detail_scroll, 5);
        press(&mut w, &mut app, KeyCode::Esc);
        assert!(!w.help);
    }

    #[test]
    fn progress_and_messages_remain_visible_on_compact_screens() {
        let (_home, app, mut w) = fixture();
        w.complete = false;
        w.motion = false;
        w.progress = care::AssessmentProgress::Cleanup {
            completed: 7,
            total: 20,
        };
        w.stage = "Measuring Python cache".into();
        let text = screen_text(&app, &w, 60, 16, "checking");
        assert!(text.contains("7/20"));
        w.progress = care::AssessmentProgress::Inventory(crate::storage::ScanProgress {
            items: 321,
            size_kb: 2048,
            errors: 2,
            path: PathBuf::from("/Users/demo/Documents"),
        });
        w.note = Some("This folder is protected from cleanup.".into());
        let text = screen_text(&app, &w, 60, 16, "scanning");
        assert!(text.contains("321 files"), "{text}");
        assert!(
            !text.contains('%'),
            "Unknown total must not get a fabricated percentage"
        );
        assert!(text.contains("This folder is protected"));
        assert!(text.lines().last().unwrap().contains("? help"));
    }

    #[test]
    fn finished_empty_and_read_only_states_explain_what_to_do() {
        let (_home, mut app, mut w) = fixture();
        w.findings.clear();
        app.analysis_only = true;
        let text = screen_text(&app, &w, 60, 16, "empty");
        assert!(text.contains("read-only"));
        assert!(text.contains("Nothing large enough to list"), "{text}");
        assert!(!text.contains("Gathering"));
        assert!(!text.lines().last().unwrap().contains("Space"));
        w.reviewing = true;
        let text = screen_text(&app, &w, 60, 16, "empty-plan");
        assert!(text.contains("YOUR PLAN IS EMPTY"));
        assert!(!text.contains("Type CLEAN"));
    }

    #[test]
    fn review_and_running_states_keep_confirmation_and_stop_visible() {
        let (_home, mut app, mut w) = fixture();
        press(&mut w, &mut app, KeyCode::Char(' '));
        press(&mut w, &mut app, KeyCode::Char('p'));
        let text = screen_text(&app, &w, 60, 16, "review");
        assert!(text.contains("Type CLEAN then Enter"));
        assert!(text.contains("nothing has run yet"));
        assert!(w.work.is_none());
        let (_sender, receiver) = mpsc::channel();
        w.work = Some(Work {
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        w.reviewing = false;
        w.session.state = "Measuring pre-action baseline · sample 3/6".into();
        let text = screen_text(&app, &w, 60, 16, "running");
        assert!(text.contains("sample 3/6"));
        assert!(text.contains("0 / 1"));
        press(&mut w, &mut app, KeyCode::Esc);
        let text = screen_text(&app, &w, 60, 16, "stopping");
        assert!(text.contains("STOP REQUESTED"));
        assert!(text.contains("Stop requested"));
        assert!(w.work.as_ref().unwrap().cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn no_color_disables_scan_motion_and_keeps_textual_status() {
        let (_home, mut app, mut w) = fixture();
        app.no_color = true;
        w.complete = false;
        w.progress = care::AssessmentProgress::Cleanup {
            completed: 2,
            total: 10,
        };
        let before = screen_text(&app, &w, 60, 16, "no-color");
        w.started -= Duration::from_millis(400);
        let after = screen_text(&app, &w, 60, 16, "no-color");
        assert_eq!(before, after);
        assert!(after.contains("2/10"));
    }

    #[test]
    fn coverage_never_queues_an_action_for_a_hidden_finding() {
        let (_home, mut app, mut w) = fixture();
        press(&mut w, &mut app, KeyCode::Char('v'));
        press(&mut w, &mut app, KeyCode::Char(' '));
        assert!(w.plan.is_empty());
        let text = screen_text(&app, &w, 60, 16, "coverage-pending");
        assert!(text.contains("coverage is not available yet"));
        assert!(!text.lines().last().unwrap().contains("Space"));
        press(&mut w, &mut app, KeyCode::Esc);
        assert!(!w.coverage);
    }
    #[test]
    fn menu_focus_never_adds_actions_and_read_only_stays_locked() {
        let (_home, mut app, mut w) = fixture();
        w.focus = 0;
        press(&mut w, &mut app, KeyCode::Char(' '));
        assert!(w.plan.is_empty());
        press(&mut w, &mut app, KeyCode::Tab);
        press(&mut w, &mut app, KeyCode::Char(' '));
        assert_eq!(w.plan.len(), 1);
        w.plan.clear();
        app.analysis_only = true;
        press(&mut w, &mut app, KeyCode::Char(' '));
        assert!(w.plan.is_empty());
    }
    #[test]
    fn space_adds_exactly_the_shared_action_for_the_finding() {
        let (_home, mut app, mut w) = fixture();
        let finding = w.selected().unwrap().clone();
        let expected = action_for_finding(&app, &finding.target, false)
            .ok()
            .flatten()
            .unwrap()
            .id();
        press(&mut w, &mut app, KeyCode::Char(' '));
        assert_eq!(
            w.plan.iter().map(Action::id).collect::<Vec<_>>(),
            vec![expected]
        );
        app.entries[0].status = CacheStatus::Whitelisted;
        assert!(action_for_finding(&app, &finding.target, false).is_err());
    }
    #[test]
    fn the_active_tab_is_highlighted_without_a_focus_mode() {
        let (_home, mut app, mut w) = fixture();
        app.terminal_width = 120;
        app.terminal_height = 30;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &app, &w))
            .unwrap();
        let (overview, explore) = (
            terminal.backend().buffer()[(13, 0)].style(),
            terminal.backend().buffer()[(24, 0)].style(),
        );
        assert_ne!(overview.bg, explore.bg, "the current tab stands out");
        press(&mut w, &mut app, KeyCode::Tab);
        assert!(w.screen == Screen::Explore && w.focus == 1);
    }
    #[test]
    fn clearing_history_requires_a_separate_phrase_and_writable_session() {
        let (home, mut app, mut w) = fixture();
        care::save_session(home.path(), &Session::default()).unwrap();
        w.navigate(2);
        press(&mut w, &mut app, KeyCode::Delete);
        assert!(w.clearing_history);
        press(&mut w, &mut app, KeyCode::Enter);
        assert_eq!(care::sessions(home.path()).len(), 1);
        for c in "CLEAR".chars() {
            press(&mut w, &mut app, KeyCode::Char(c));
        }
        press(&mut w, &mut app, KeyCode::Enter);
        assert!(care::sessions(home.path()).is_empty());
        care::save_session(home.path(), &Session::default()).unwrap();
        app.analysis_only = true;
        press(&mut w, &mut app, KeyCode::Delete);
        assert!(!w.clearing_history);
        assert_eq!(care::sessions(home.path()).len(), 1);
    }
    #[test]
    fn low_capacity_shows_in_the_header_before_inventory() {
        let (_home, app, mut w) = fixture();
        w.complete = false;
        w.volume = Some(crate::storage::VolumeStats {
            accounting_path: PathBuf::from("/"),
            filesystem: "fixture".into(),
            capacity_kb: 100 * 1_048_576,
            used_kb: 98 * 1_048_576,
            free_kb: 2 * 1_048_576,
            container_free_kb: None,
            device: 1,
        });
        w.rebuild(&app);
        let text = screen_text(&app, &w, 120, 30, "low-capacity");
        assert!(
            text.lines().next().unwrap().contains("2.0 GiB free"),
            "{text}"
        );
        assert!(
            !w.visible().iter().any(|f| f.id == "system:disk"),
            "the alert is not repeated as a list row"
        );
        assert!(
            w.findings.iter().any(|f| f.id == "system:disk"),
            "f still lists it"
        );
    }
    #[test]
    fn review_requires_exact_phrase_and_tiny_terminal_blocks_hidden_confirmation() {
        let (_home, mut app, mut w) = fixture();
        w.add_selected(&mut app, false);
        w.reviewing = true;
        press(&mut w, &mut app, KeyCode::Enter);
        assert!(w.work.is_none());
        w.acknowledgement = "CLEAN".into();
        app.terminal_width = 40;
        press(&mut w, &mut app, KeyCode::Enter);
        assert!(w.work.is_none());
        press(&mut w, &mut app, KeyCode::Esc);
        assert!(!w.reviewing);
        assert_eq!(w.plan.len(), 1);
    }
    #[test]
    fn changed_evidence_invalidates_ai_and_overlapping_paths_cannot_be_queued() {
        let (_home, mut app, mut w) = fixture();
        let f = w.selected().unwrap().clone();
        w.report = Some(report_for(&f, "Cache can be downloaded again.", vec![]));
        app.entries[0].status = CacheStatus::InUse;
        w.rebuild(&app);
        assert!(w.report.is_none());
        assert!(w.ai_error.as_deref().unwrap().contains("Evidence changed"));
        let entry = app.entries[0].clone();
        w.add_action(Action::Clean(entry.clone()));
        let mut child = entry;
        child.spec.path.push("child");
        w.add_action(Action::Clean(child));
        assert_eq!(w.plan.len(), 1);
    }
    #[test]
    fn a_report_is_rejected_when_evidence_changes_during_the_investigation() {
        let (_home, mut app, mut w) = fixture();
        let finding = w.selected().unwrap().clone();
        w.start_investigation(&app, &finding, false);
        assert!(w.agent.is_some());
        app.entries[0].status = CacheStatus::Whitelisted;
        w.rebuild(&app);
        settle(&mut w, &mut app);
        assert!(w.report.is_none());
        assert!(w.ai_error.as_deref().unwrap().contains("Evidence changed"));
        let case = w.investigation_case.as_ref().unwrap();
        assert!(!case.tool_calls.is_empty(), "measured checks still ran");
        assert!(
            w.session
                .investigations
                .iter()
                .any(|saved| saved.id == case.id)
        );
    }
    #[test]
    fn measured_investigation_without_ai_records_a_timeline_and_conclusion() {
        let (_home, mut app, mut w) = fixture();
        let finding = w.selected().unwrap().clone();
        press(&mut w, &mut app, KeyCode::Char('i'));
        assert!(w.agent.as_ref().is_some_and(|run| !run.by_model()));
        settle(&mut w, &mut app);
        let report = w.report.as_ref().expect("a measured conclusion");
        assert!(!report.by_model);
        assert!(report.suggestions.is_empty());
        let case = w.investigation_case.as_ref().unwrap();
        assert_eq!(case.phase, investigation::CasePhase::Inconclusive);
        assert!(case.tool_calls.iter().all(|call| !call.chosen_by_model));
        assert!(
            case.tool_calls
                .iter()
                .any(|call| call.tool == "cleanup_rule")
        );
        w.detail = true;
        let text = screen_text(&app, &w, 80, 40, "measured-investigation");
        assert!(text.contains("WHAT THE CHECKS FOUND"), "{text}");
        assert!(text.contains("HOW THIS WAS CHECKED"));
        assert!(
            text.contains("cleanup_rule("),
            "expanded details list each call"
        );
        w.detail = false;
        let text = screen_text(&app, &w, 140, 40, "measured-investigation-compact");
        assert!(
            text.contains("cleanup rule") && !text.contains("cleanup_rule("),
            "{text}"
        );
        assert_eq!(
            w.agent_subject.as_ref().map(|s| s.id.as_str()),
            Some(finding.id.as_str())
        );
    }
    #[test]
    fn triage_fallback_keeps_measured_ids_selection_and_source_clear() {
        let (_home, mut app, mut w) = fixture();
        let first = w.findings[0].clone();
        let mut second = first.clone();
        second.id = "finding:second".into();
        second.title = "Second measured finding".into();
        w.findings.push(second);
        w.cursor = 1;
        let selected = w.selected().unwrap().id.clone();
        let mut first_subject = subject_for_finding(&first);
        first_subject.id = "item1".into();
        let request = ai::Request {
            revision: w.revision,
            subjects: vec![first_subject],
        };
        let (sender, receiver) = mpsc::channel();
        w.triage_work = Some(TriageWork {
            receiver,
            worker: None,
            cancel: Arc::new(AtomicBool::new(false)),
            request,
            id_map: HashMap::from([("item1".into(), first.id.clone())]),
        });
        sender
            .send(Err("AI returned unsupported triage references.".into()))
            .unwrap();
        w.tick(&mut app);
        assert_eq!(w.selected().unwrap().id, selected);
        assert!(!w.triage_is_ai);
        assert_eq!(w.triage.unwrap().quick_win_ids, vec![first.id]);
        assert!(
            w.ai_error
                .unwrap()
                .contains("Showing measured priority order")
        );
    }
    #[test]
    fn growth_since_the_last_check_becomes_an_investigable_finding() {
        let (home, mut app, mut w) = fixture();
        let grown = home.path().join("Library/Caches/Grown");
        w.growth = vec![
            crate::growth::Delta {
                path: grown.display().to_string(),
                before_kb: 1_048_576,
                after_kb: 4 * 1_048_576,
            },
            crate::growth::Delta {
                path: home.path().join("Shrunk").display().to_string(),
                before_kb: 4 * 1_048_576,
                after_kb: 1_048_576,
            },
        ];
        w.rebuild(&app);
        let growth: Vec<_> = w
            .findings
            .iter()
            .filter(|f| f.id.starts_with("growth:"))
            .collect();
        assert_eq!(growth.len(), 1, "only increases become findings");
        assert!(growth[0].title.starts_with("Grew 3.0 GiB"));
        assert!(matches!(&growth[0].target, Target::Folder(path) if path == &grown));
        assert!(
            suggestion_id(&app, &growth[0].target).is_none(),
            "growth never suggests deletion"
        );
        let finding = growth[0].clone();
        w.start_investigation(&app, &finding, false);
        assert_eq!(
            w.investigation_case.as_ref().unwrap().family,
            investigation::InvestigationFamily::StorageGrowth
        );
        press(&mut w, &mut app, KeyCode::Esc);
    }

    #[test]
    fn agent_proposals_are_rechecked_and_only_reach_the_review_plan() {
        let (home, app, mut w) = fixture();
        let finding = w.selected().unwrap().clone();
        let Target::Cache(path) = &finding.target else {
            panic!("fixture finding is a cache");
        };
        let plan = crate::pending::PendingPlan {
            schema: "diskray.pending/1".into(),
            created: care::timestamp(),
            client: "Claude Code".into(),
            reason: "rebuildable".into(),
            actions: vec![
                crate::pending::ProposedAction {
                    id: suggestion_id(&app, &finding.target).unwrap(),
                    path: path.display().to_string(),
                    label: "Python cache".into(),
                    size_kb: 1,
                },
                crate::pending::ProposedAction {
                    id: "clean:/somewhere/else".into(),
                    path: "/somewhere/else".into(),
                    label: "Stale target".into(),
                    size_kb: 1,
                },
            ],
        };
        let saved = crate::pending::save(home.path(), &plan).unwrap();
        w.load_proposal((saved.clone(), plan));
        w.apply_proposal(&app);
        assert_eq!(w.plan.len(), 1, "only the still-eligible action is added");
        assert!(w.reviewing, "the review screen opens");
        assert!(
            w.work.is_none(),
            "nothing runs without the typed confirmation"
        );
        assert!(w.note.as_deref().unwrap().contains("Stale target"));
        assert!(!saved.exists(), "a loaded proposal is consumed");
        assert!(path.exists());
    }

    #[test]
    fn suggestion_ids_match_the_plan_action_they_describe() {
        let (_home, app, w) = fixture();
        let finding = w.selected().unwrap().clone();
        let action = action_for_finding(&app, &finding.target, false)
            .ok()
            .flatten()
            .unwrap();
        assert_eq!(suggestion_id(&app, &finding.target), Some(action.id()));
        let process = ProcessEntry {
            pid: 77,
            parent_pid: 1,
            uid: 501,
            state: "S".into(),
            elapsed: "01:00".into(),
            cpu_percent: "0".into(),
            command: "/Applications/Example.app/Contents/MacOS/Example".into(),
            rss_kb: None,
            system_owned: false,
            health: ProcessHealth::Running,
            signalable: true,
            signal_block_reason: None,
            outcome: None,
            start_time: "start".into(),
        };
        let signal = Action::Signal(process.clone(), ProcessSignal::Terminate);
        assert_eq!(
            care::suggestion_id(&[], &[process], &Target::Process(77, "start".into())),
            Some(signal.id())
        );
    }
    #[test]
    fn single_key_legacy_execution_is_unreachable_in_the_shipped_interface() {
        let (_home, mut app, _w) = fixture();
        app.care = Some(Workspace::empty());
        app.selected.insert(0);
        app.phase = Phase::ReviewConfirm;
        app.begin_cleanup();
        assert_eq!(
            app.phase,
            Phase::ReviewConfirm,
            "no cleanup starts from a bare y"
        );
        assert!(app.cleanup_queue.is_empty());
        app.phase = Phase::RelocationConfirm;
        app.begin_relocation();
        assert_eq!(app.phase, Phase::RelocationConfirm);
    }

    #[test]
    fn measured_triage_never_moves_the_selection_to_another_finding() {
        let (home, mut app, mut w) = fixture();
        app.entries.clear();
        let mut findings: Vec<Finding> = (0..5)
            .map(|index| Finding {
                id: format!("process:{index}:start"),
                title: format!("Process {index}"),
                observation: "busy".into(),
                consequence: "review".into(),
                size_kb: 0,
                quick_win: false,
                target: Target::Process(index, "start".into()),
                related_pids: vec![index],
            })
            .collect();
        let cache = Finding {
            id: "path:/cache".into(),
            title: "Large cache".into(),
            observation: "9 GB".into(),
            consequence: "review".into(),
            size_kb: 9 * 1_048_576,
            quick_win: false,
            target: Target::Cache(home.path().join("cache")),
            related_pids: vec![],
        };
        findings.insert(0, cache.clone());
        w.findings = findings;
        w.cursor = 0;
        assert_eq!(w.selected().unwrap().id, cache.id);
        let subjects: Vec<ai::Subject> = w.findings.iter().map(subject_for_finding).collect();
        let triage = ai::deterministic_triage(&ai::Request {
            revision: w.revision,
            subjects,
        });
        assert!(
            !triage.key_area_ids.contains(&cache.id),
            "the cache ranks outside the top five"
        );
        w.apply_triage(triage, false);
        assert_eq!(w.selected().unwrap().id, cache.id, "selection is kept");
        press(&mut w, &mut app, KeyCode::Char('f'));
        press(&mut w, &mut app, KeyCode::Char('f'));
        assert!(w.pinned.is_none(), "toggling the list releases the pin");
    }
    #[test]
    fn triage_aliases_reject_unknown_ids_and_drop_unverified_reasons() {
        let map = HashMap::from([("item1".into(), "finding:real".into())]);
        let valid = ai::Triage {
            key_area_ids: vec!["item1".into()],
            quick_win_ids: vec![],
            reasons: vec!["Unverified claim".into()],
        };
        let resolved = resolve_triage_ids(valid, &map).unwrap();
        assert_eq!(resolved.key_area_ids, vec!["finding:real"]);
        assert!(resolved.reasons.is_empty());
        assert!(
            resolve_triage_ids(
                ai::Triage {
                    key_area_ids: vec!["unknown".into()],
                    quick_win_ids: vec![],
                    reasons: vec![]
                },
                &map
            )
            .is_none()
        );
    }
    #[test]
    fn triage_rank_expires_when_measurements_change() {
        let (_home, app, mut w) = fixture();
        let id = w.findings[0].id.clone();
        w.apply_triage(
            ai::Triage {
                key_area_ids: vec![],
                quick_win_ids: vec![id],
                reasons: vec![],
            },
            true,
        );
        assert!(w.triage_is_ai);
        w.revision += 1;
        w.rebuild(&app);
        assert!(w.triage.is_none());
        assert!(!w.triage_is_ai);
    }
    #[test]
    fn process_report_expires_once_live_readings_move_on() {
        let (_home, mut app, mut w) = fixture();
        app.entries.clear();
        app.specs.clear();
        let process = ProcessEntry {
            pid: 4242,
            parent_pid: 1,
            uid: 501,
            state: "R".into(),
            elapsed: "00:20".into(),
            cpu_percent: "12.0".into(),
            command: "/Applications/Example.app/Contents/MacOS/Example".into(),
            rss_kb: Some(700 * 1024),
            system_owned: false,
            health: ProcessHealth::Running,
            signalable: true,
            signal_block_reason: None,
            outcome: None,
            start_time: "example-start".into(),
        };
        app.processes = vec![process];
        w.metrics.sampled_at = 10;
        w.metrics.rss_kb.insert(4242, 700 * 1024);
        w.rebuild(&app);
        let finding = w
            .findings
            .iter()
            .find(|finding| matches!(finding.target, Target::Process(4242, _)))
            .cloned()
            .expect("process finding");
        let mut report = report_for(&finding, "The process stayed present.", vec![]);
        report.sampled_at = Some(10);
        w.report = Some(report);
        w.metrics.sampled_at = 12;
        w.rebuild(&app);
        assert!(
            w.report.is_some(),
            "a few seconds of drift keeps the report"
        );
        w.metrics.sampled_at = 10 + REPORT_LIFETIME_SECS + 1;
        w.rebuild(&app);
        assert!(w.report.is_none());
        assert!(
            w.ai_error
                .as_deref()
                .is_some_and(|message| message.contains("older than the live"))
        );
        let suggestion = suggestion_id(&app, &finding.target).unwrap();
        assert!(
            suggestion.ends_with("SIGTERM"),
            "only a graceful stop is suggested"
        );
    }
    #[test]
    fn investigation_history_keeps_prior_cases_in_one_session() {
        let (_home, app, mut w) = fixture();
        let first = investigation::InvestigationCase::new_storage("/tmp/first", 1, false);
        let second = investigation::InvestigationCase::new_process("Example", 2, false, true);
        w.investigation_case = Some(first.clone());
        w.persist(&app);
        w.investigation_case = Some(second.clone());
        w.persist(&app);
        assert_eq!(w.session.investigations.len(), 2);
        assert_eq!(w.session.investigations[0].id, first.id);
        assert_eq!(w.session.investigations[1].id, second.id);
    }
    #[test]
    fn compact_workspace_uses_a_full_width_list_and_detail_page() {
        let (_home, mut app, mut w) = fixture();
        app.terminal_width = 60;
        app.terminal_height = 16;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(60, 16)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &app, &w))
            .unwrap();
        let list_text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(list_text.contains("WHERE THE SPACE WENT"));
        press(&mut w, &mut app, KeyCode::Enter);
        assert!(w.detail);
        terminal
            .draw(|frame| render(frame, frame.area(), &app, &w))
            .unwrap();
        let detail_text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(detail_text.contains("DETAILS"));
    }
    #[test]
    fn unified_screens_render_at_supported_sizes_and_export_previews() {
        let (_home, mut app, mut w) = fixture();
        use crate::storage::{StorageCategory, StorageRoot};
        let demo = Path::new("/Users/demo");
        let specs = scan_specs(demo, demo);
        for (label, size, status) in [
            ("Homebrew cache", 800 * 1024, CacheStatus::Ready),
            ("Xcode device support", 10 * 1_048_576, CacheStatus::Review),
            ("npm package cache", 3 * 1_048_576, CacheStatus::InUse),
            ("Playwright browsers", 1_048_576, CacheStatus::Optional),
        ] {
            if let Some(spec) = specs.iter().find(|s| s.label == label) {
                app.entries.push(CacheEntry {
                    spec: spec.clone(),
                    size_kb: size,
                    status,
                    outcome: None,
                    identity: app.entries[0].identity,
                });
            }
        }
        let path = demo.join("Library/Caches/Python");
        app.entries[0].spec.path = path.clone();
        let item = StorageItem {
            path: path.clone(),
            size_kb: 1_048_576,
            kind: StorageItemKind::Directory,
            category: StorageCategory::DeveloperData,
        };
        let children = vec![
            StorageItem {
                path: path.join("wheels"),
                size_kb: 720 * 1024,
                kind: StorageItemKind::Directory,
                category: StorageCategory::DeveloperData,
            },
            StorageItem {
                path: path.join("http downloads"),
                size_kb: 304 * 1024,
                kind: StorageItemKind::Directory,
                category: StorageCategory::DeveloperData,
            },
        ];
        let mut index = std::collections::BTreeMap::new();
        index.insert(path.clone(), children);
        index.insert(path.parent().unwrap().to_path_buf(), vec![item.clone()]);
        app.inventory = Some(StorageInventory {
            volume: None,
            volume_error: None,
            roots: vec![StorageRoot {
                path: path.parent().unwrap().to_path_buf(),
                size_kb: item.size_kb,
                device: 1,
                scan_errors: 0,
            }],
            scanned_kb: item.size_kb,
            scanned_on_volume_kb: item.size_kb,
            unaccounted_kb: 0,
            inventory_overage_kb: 0,
            local_snapshots: vec![],
            scanned_items: 3,
            scan_errors: 0,
            scan_error_paths: vec![],
            complete: true,
            top_level: vec![item.clone()],
            largest: vec![item],
            children: index,
        });
        w.rebuild(&app);
        let first = w.findings[0].clone();
        let suggestion = suggestion_id(&app, &first.target).into_iter().collect();
        w.report = Some(report_for(
            &first,
            "E2 shows most of the cache is wheels last changed months ago, and E3 found no open files. It can be rebuilt; clearing it trades disk space for a future download.",
            suggestion,
        ));
        let mut preview_case = investigation::InvestigationCase::new_developer(
            "/Users/example/Library/Caches/pip",
            w.revision,
            false,
        );
        preview_case.phase = investigation::CasePhase::Complete;
        preview_case.decision_budget = agent::EXPLICIT_CALLS;
        preview_case.decision_count = 2;
        for (id, tool, label, summary, supports) in [
            (
                "E2",
                "folder_age",
                "folder_age(~/Library/Caches/pip)",
                "~/Library/Caches/pip: 412 files, 1.0 GB. Changed within 7 days 3% · older 71%.",
                "measured_children",
            ),
            (
                "E3",
                "open_handles",
                "open_handles(~/Library/Caches/pip)",
                "No process has files open under ~/Library/Caches/pip right now.",
                "rebuildable_data",
            ),
        ] {
            let mut evidence = investigation::evidence(
                id,
                investigation::EvidenceKind::VolumeContext,
                label,
                summary,
                &[supports],
                &[],
                investigation::EvidenceStatus::Complete,
            );
            evidence.observed_at = 0;
            preview_case.add_evidence(evidence);
            preview_case.tool_calls.push(investigation::ToolCallRecord {
                tool: tool.into(),
                label: label.into(),
                evidence_id: Some(id.into()),
                status: investigation::EvidenceStatus::Complete,
                elapsed_ms: 400,
                chosen_by_model: true,
                rejected: None,
            });
        }
        w.investigation_case = Some(preview_case);
        w.agent_subject = Some(subject_for_finding(&first));
        w.volume = Some(crate::storage::VolumeStats {
            accounting_path: PathBuf::from("/"),
            filesystem: "synthetic".into(),
            capacity_kb: 250 * 1_048_576,
            used_kb: 232 * 1_048_576,
            free_kb: 18 * 1_048_576,
            container_free_kb: None,
            device: 1,
        });
        w.metrics.pressure = Some(1);
        w.trend = VecDeque::from(vec![8, 12, 11, 22, 35, 28, 16, 12, 8, 7, 9, 10]);
        for (width, height) in [(60, 16), (80, 24), (120, 36), (160, 44)] {
            app.terminal_width = width;
            app.terminal_height = height;
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render(frame, frame.area(), &app, &w))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let first = (0..width)
                .map(|x| buffer[(x, 0)].symbol())
                .collect::<String>();
            assert!(first.contains("Overview"));
            assert!(first.contains("History"));
            if let Some(dir) = std::env::var_os("CARE_PREVIEW_DIR") {
                fs::create_dir_all(&dir).unwrap();
                fs::write(
                    PathBuf::from(dir).join(format!("care-{width}.svg")),
                    super::super::tests::buffer_svg(buffer),
                )
                .unwrap();
            }
            w.navigate(0);
            w.user_moved = true;
            let reported = w
                .report
                .as_ref()
                .and_then(|r| r.subject.as_ref())
                .unwrap()
                .id
                .clone();
            w.cursor = w.visible().iter().position(|f| f.id == reported).unwrap();
            let overview = screen_text(&app, &w, width, height, "overview");
            assert!(overview.contains("WHERE"), "{overview}");
            if width == 120 {
                w.detail = true;
                let details = screen_text(&app, &w, width, height, "details");
                let order: Vec<usize> = [
                    "AI suggests clearing",
                    "LOCAL AI",
                    "WHAT YOU CAN DO",
                    "HOW THIS WAS CHECKED",
                ]
                .iter()
                .map(|label| {
                    details
                        .find(label)
                        .unwrap_or_else(|| panic!("{label}: {details}"))
                })
                .collect();
                assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{details}");
                w.detail = false;
            }
            w.palette = Some((String::new(), 0));
            let palette = screen_text(&app, &w, width, height, "palette");
            assert!(palette.contains("COMMANDS"), "{palette}");
            w.palette = None;
            w.navigate(0);
            w.reviewing = true;
            terminal
                .draw(|frame| render(frame, frame.area(), &app, &w))
                .unwrap();
            w.reviewing = false;
            w.navigate(2);
            terminal
                .draw(|frame| render(frame, frame.area(), &app, &w))
                .unwrap();
            w.navigate(0);
        }
    }
    #[test]
    fn overview_ranks_disk_items_by_size_with_plain_verdicts() {
        let (_home, mut app, mut w) = fixture();
        assert!(
            Workspace::empty().screen == Screen::Overview,
            "Overview is the landing screen"
        );
        let text = screen_text(&app, &w, 80, 24, "overview");
        assert!(text.contains("Overview") && text.contains("Explore") && text.contains("History"));
        assert!(text.contains("Python cache"), "{text}");
        assert!(text.contains("Safe to clear"), "{text}");
        assert!(text.contains("Quick wins"), "{text}");
        // `a` adds every quick win; Space on the row toggles it back out.
        press(&mut w, &mut app, KeyCode::Char('a'));
        assert_eq!(w.plan.len(), 1);
        let text = screen_text(&app, &w, 80, 24, "overview-planned");
        assert!(text.contains("✓ In plan"), "{text}");
        press(&mut w, &mut app, KeyCode::Char(' '));
        assert!(w.plan.is_empty());
        // Enter explains in place; Esc returns to the same row.
        press(&mut w, &mut app, KeyCode::Enter);
        assert!(w.screen == Screen::Overview && w.detail);
        press(&mut w, &mut app, KeyCode::Esc);
        assert!(!w.detail && w.cursor == 0);
        for (key, screen) in [
            (KeyCode::Char('3'), Screen::History),
            (KeyCode::Char('2'), Screen::Explore),
            (KeyCode::Tab, Screen::History),
            (KeyCode::Tab, Screen::Overview),
            (KeyCode::BackTab, Screen::History),
            (KeyCode::Char('1'), Screen::Overview),
        ] {
            press(&mut w, &mut app, key);
            assert!(w.screen == screen);
        }
    }
    #[test]
    fn default_list_hides_unreadable_targets_and_ordinary_processes() {
        let (_home, mut app, mut w) = fixture();
        app.entries[0].status = CacheStatus::ScanError;
        app.entries[0].size_kb = 0;
        w.rebuild(&app);
        assert!(w.visible().is_empty(), "an unreadable target is not a row");
        let text = screen_text(&app, &w, 80, 24, "unreadable");
        assert!(text.contains("Couldn't read 1 locations"), "{text}");
        press(&mut w, &mut app, KeyCode::Char('f'));
        assert_eq!(w.visible().len(), 1, "f lists everything");
    }
    #[test]
    fn command_palette_filters_and_runs_the_chosen_command() {
        let (_home, mut app, mut w) = fixture();
        press(&mut w, &mut app, KeyCode::Char(':'));
        assert!(w.palette.is_some());
        for c in "hist".chars() {
            press(&mut w, &mut app, KeyCode::Char(c));
        }
        assert_eq!(palette_matches("hist").len(), 1);
        let text = screen_text(&app, &w, 80, 24, "palette");
        assert!(text.contains("COMMANDS"), "{text}");
        assert!(text.contains("Go to History"));
        press(&mut w, &mut app, KeyCode::Enter);
        assert!(w.palette.is_none());
        assert!(w.screen == Screen::History);
        // Typing never triggers single-letter commands while the palette is open.
        press(&mut w, &mut app, KeyCode::Char(':'));
        press(&mut w, &mut app, KeyCode::Char('q'));
        assert!(!app.quit);
        press(&mut w, &mut app, KeyCode::Esc);
        assert!(w.palette.is_none() && !app.quit);
        assert!(palette_matches("zzz").is_empty());
        assert_eq!(palette_matches("").len(), PALETTE.len());
    }
    #[test]
    fn answer_follow_ups_prefill_the_ask_box() {
        let (_home, mut app, mut w) = fixture();
        let mut case = investigation::InvestigationCase::new_question(
            "Why is my disk almost full?",
            w.revision,
        );
        case.phase = investigation::CasePhase::Complete;
        w.investigation_case = Some(case);
        w.answer_open = true;
        let follow = follow_ups("why is my disk almost full?");
        assert_eq!(follow.len(), 3);
        assert!(!follow.contains(&"Why is my disk almost full?"));
        press(&mut w, &mut app, KeyCode::Char('2'));
        assert!(!w.answer_open);
        assert_eq!(w.asking.as_deref(), Some(follow[1]));
    }
    #[test]
    fn review_groups_actions_and_totals_the_space() {
        let (_home, mut app, mut w) = fixture();
        press(&mut w, &mut app, KeyCode::Char(' '));
        press(&mut w, &mut app, KeyCode::Char('p'));
        let text = screen_text(&app, &w, 100, 30, "review");
        assert!(text.contains("CLEAR REBUILDABLE CONTENTS"), "{text}");
        assert!(text.contains("up to 1.0 GiB to reclaim"), "{text}");
        assert!(text.contains("Type CLEAN"));
    }
    #[test]
    fn reduced_motion_does_not_animate_inference_border() {
        let (_home, app, mut w) = fixture();
        w.motion = false;
        let (_tx, rx) = mpsc::channel();
        w.triage_work = Some(TriageWork {
            receiver: rx,
            worker: None,
            cancel: Arc::new(AtomicBool::new(false)),
            request: ai::Request {
                revision: 0,
                subjects: vec![],
            },
            id_map: HashMap::new(),
        });
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 36)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &app, &w))
            .unwrap();
        let before = terminal.backend().buffer().clone();
        w.started -= Duration::from_millis(500);
        terminal
            .draw(|frame| render(frame, frame.area(), &app, &w))
            .unwrap();
        assert_eq!(&before, terminal.backend().buffer());
    }

    #[test]
    fn administrator_diagnostic_waits_for_approval_and_decline_is_not_evidence() {
        let (_home, mut app, mut w) = fixture();
        let mut case = investigation::InvestigationCase::new_fseventsd("fseventsd", 1, false);
        w.agent = Some(agent::start(
            &mut case,
            agent::Subject::default(),
            false,
            false,
        ));
        w.investigation_case = Some(case);
        w.tick(&mut app);
        assert!(w.awaiting_approval());
        assert_eq!(
            w.investigation_case.as_ref().unwrap().phase,
            investigation::CasePhase::AwaitingApproval
        );
        let text = screen_text(&app, &w, 60, 16, "approval");
        assert!(text.contains("APPROVAL NEEDED"));
        press(&mut w, &mut app, KeyCode::Char(' '));
        assert!(w.plan.is_empty(), "approval owns the keyboard");
        press(&mut w, &mut app, KeyCode::Esc);
        assert!(!w.awaiting_approval());
        let case = w.investigation_case.as_ref().unwrap();
        assert_eq!(
            case.evidence[0].status,
            investigation::EvidenceStatus::Cancelled
        );
        assert!(case.hypotheses.iter().all(|hypothesis| {
            hypothesis.supporting_evidence.is_empty()
                && hypothesis.status == investigation::HypothesisStatus::Open
        }));
    }
    #[test]
    fn ask_box_is_a_bounded_modal_that_starts_a_question_case() {
        let (_home, mut app, mut w) = fixture();
        press(&mut w, &mut app, KeyCode::Char('/'));
        assert!(w.asking.is_none(), "Ask needs a ready model");
        assert!(w.note.as_deref().unwrap().contains("Apple Intelligence"));
        w.ai_framework = ai::FrameworkStatus::Available {
            detail: String::new(),
        };
        for width in [60, 80, 120] {
            press(&mut w, &mut app, KeyCode::Char('/'));
            assert_eq!(w.asking.as_deref(), Some(""));
            let text = screen_text(&app, &w, width, 24, "ask");
            assert!(text.contains("ASK ABOUT THIS MAC"), "{width}");
            press(&mut w, &mut app, KeyCode::Esc);
            assert!(w.asking.is_none());
        }
        press(&mut w, &mut app, KeyCode::Char('/'));
        press(&mut w, &mut app, KeyCode::Enter);
        assert_eq!(
            w.asking.as_deref(),
            Some(""),
            "an empty question is not sent"
        );
        for c in "quit?".chars() {
            press(&mut w, &mut app, KeyCode::Char(c));
        }
        press(&mut w, &mut app, KeyCode::Char('\u{1b}'));
        assert!(!app.quit, "q is a letter while typing");
        assert_eq!(w.asking.as_deref(), Some("quit?"));
        for _ in 0..(agent::QUESTION_LIMIT + 20) {
            press(&mut w, &mut app, KeyCode::Char('x'));
        }
        assert_eq!(
            w.asking.as_ref().unwrap().chars().count(),
            agent::QUESTION_LIMIT
        );
        press(&mut w, &mut app, KeyCode::Enter);
        assert!(w.asking.is_none());
        assert!(w.answer_open);
        let case = w.investigation_case.as_ref().unwrap();
        assert_eq!(case.family, investigation::InvestigationFamily::Question);
        assert!(case.question.as_deref().unwrap().starts_with("quit?"));
        settle(&mut w, &mut app);
        let text = screen_text(&app, &w, 80, 24, "answer");
        assert!(text.contains("QUESTION"));
        press(&mut w, &mut app, KeyCode::Esc);
        assert!(!w.answer_open);
    }
    #[test]
    fn ai_suggestions_are_badges_that_never_change_the_plan() {
        let (_home, mut app, mut w) = fixture();
        let finding = w.selected().unwrap().clone();
        let id = suggestion_id(&app, &finding.target).expect("a ready quick win");
        w.report = Some(report_for(
            &finding,
            "E1 shows a rebuildable cache.",
            vec![id],
        ));
        let text = screen_text(&app, &w, 120, 30, "suggested");
        assert!(text.contains("AI SUGGESTS"));
        assert!(w.plan.is_empty(), "a suggestion is never added by itself");
        press(&mut w, &mut app, KeyCode::Char(' '));
        assert_eq!(w.plan.len(), 1, "Space adds the suggested action");
        w.plan.clear();
        app.entries[0].status = CacheStatus::InUse;
        assert!(suggestion_id(&app, &finding.target).is_none());
        assert!(!text_contains_badge(&app, &w));
        app.entries[0].status = CacheStatus::Review;
        assert!(
            suggestion_id(&app, &finding.target).is_none(),
            "review data is never suggested"
        );
        assert!(matches!(
            action_for_finding(&app, &finding.target, false),
            Ok(Some(Action::ReviewClean(_)))
        ));
    }
    fn text_contains_badge(app: &App, w: &Workspace) -> bool {
        screen_text(app, w, 120, 30, "suggested-stale").contains("AI SUGGESTS")
    }
    fn add_key_area(w: &mut Workspace, home: &Path) {
        w.findings.push(Finding {
            id: format!("path:{}", home.display()),
            title: "Large folder".into(),
            observation: "2 GB measured · size does not establish waste".into(),
            consequence: "Inspect contents.".into(),
            size_kb: 2 * 1_048_576,
            quick_win: false,
            target: Target::Folder(home.to_path_buf()),
            related_pids: vec![],
        });
    }
    #[test]
    fn automatic_investigation_starts_once_after_triage_and_yields() {
        let (home, mut app, mut w) = fixture();
        add_key_area(&mut w, home.path());
        w.ai_framework = ai::FrameworkStatus::Available {
            detail: String::new(),
        };
        w.metrics.pressure = Some(1);
        w.maybe_start_automatic(&app);
        assert!(w.agent.is_none(), "waits for triage to settle");
        w.apply_triage(
            ai::Triage {
                key_area_ids: vec![],
                quick_win_ids: vec![],
                reasons: vec![],
            },
            false,
        );
        w.metrics.pressure = Some(2);
        w.maybe_start_automatic(&app);
        assert!(w.agent.is_none(), "elevated pressure blocks automatic work");
        w.metrics.pressure = Some(1);
        w.maybe_start_automatic(&app);
        let run = w.agent.as_ref().expect("automatic run");
        assert!(run.automatic && run.budget == agent::AUTOMATIC_CALLS);
        assert!(
            !run.by_model(),
            "the missing test helper falls back to measured checks"
        );
        press(&mut w, &mut app, KeyCode::Char('p'));
        assert!(w.agent.is_none(), "opening review stops automatic work");
        press(&mut w, &mut app, KeyCode::Esc);
        w.maybe_start_automatic(&app);
        assert!(
            w.agent.is_none(),
            "only one automatic investigation per assessment"
        );
        let finding = w.selected().unwrap().clone();
        w.start_investigation(&app, &finding, false);
        assert!(w.agent.is_some());
        press(&mut w, &mut app, KeyCode::Esc);
        assert!(w.agent.is_none(), "Esc stops a running investigation");
        let case = w.investigation_case.as_ref().unwrap();
        assert_eq!(case.phase, investigation::CasePhase::Inconclusive);
        assert!(
            w.session
                .investigations
                .iter()
                .any(|saved| saved.id == case.id)
        );
    }
    #[test]
    fn critical_pressure_pauses_automatic_model_work_and_resumes_once() {
        let (home, app, mut w) = fixture();
        add_key_area(&mut w, home.path());
        let finding = w.selected().unwrap().clone();
        w.metrics.pressure = Some(1);
        w.start_investigation(&app, &finding, true);
        assert!(w.agent.is_some());
        w.metrics.pressure = Some(2);
        w.respond_to_pressure(&app);
        assert!(
            w.agent.is_some(),
            "measured checks are not model work and keep running"
        );
        w.agent = None;
        w.auto_agent_resume = true;
        w.auto_agent_started = true;
        w.ai_framework = ai::FrameworkStatus::Available {
            detail: String::new(),
        };
        w.apply_triage(
            ai::Triage {
                key_area_ids: vec![],
                quick_win_ids: vec![],
                reasons: vec![],
            },
            false,
        );
        w.metrics.pressure = Some(1);
        w.maybe_start_automatic(&app);
        assert!(w.agent.is_some(), "a paused automatic run resumes once");
        assert!(!w.auto_agent_resume);
    }
}
