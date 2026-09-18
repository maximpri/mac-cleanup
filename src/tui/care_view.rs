//! Unified visual care workspace. The existing domain engines retain action authority.
use super::*;
use crate::cache;
use crate::{
    ai,
    care::{self, Finding, Metrics, RecordedAction, Session, Target},
    investigation,
};
use std::collections::{HashMap, VecDeque};

const AI_VIOLET: Color = Color::Rgb(172, 151, 255);
#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Findings,
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
                "Clear {} · {}\n{}\nTradeoff: {}",
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
    started: Instant,
}
struct TriageWork {
    receiver: Receiver<Result<ai::Triage, String>>,
    worker: Option<thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    request: ai::Request,
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
enum InvestigationResult {
    Inventory(Box<StorageInventory>),
    Note(String),
}
struct Investigation {
    receiver: Receiver<InvestigationResult>,
    stop: Arc<AtomicBool>,
    deadline: Instant,
    check: String,
    checks: VecDeque<String>,
    target: Target,
}
impl Drop for Investigation {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub(super) struct Workspace {
    assessment: Option<care::Assessment>,
    findings: Vec<Finding>,
    screen: Screen,
    cursor: usize,
    nav: usize,
    focus: usize,
    detail: bool,
    coverage: bool,
    detail_scroll: u16,
    show_all: bool,
    kept: HashSet<String>,
    stage: String,
    complete: bool,
    volume: Option<crate::storage::VolumeStats>,
    metrics: Metrics,
    trend: VecDeque<u64>,
    revision: u64,
    started: Instant,
    triage_work: Option<TriageWork>,
    triage: Option<ai::Triage>,
    triage_reasons: HashMap<String, String>,
    insight_work: Option<InsightWork>,
    insight: Option<ai::Insight>,
    insight_cache: HashMap<String, ai::Insight>,
    result_work: Option<InsightWork>,
    result_insight: Option<ai::Insight>,
    result_summary_session: Option<u64>,
    insight_scope: Vec<String>,
    insight_inputs: Vec<ai::Subject>,
    ai_error: Option<String>,
    ai_framework: ai::FrameworkStatus,
    ai_framework_work: Option<Receiver<ai::FrameworkStatus>>,
    auto_requested: bool,
    auto_investigation_started: bool,
    investigation: Option<Investigation>,
    investigation_case: Option<investigation::InvestigationCase>,
    pending_checks: VecDeque<String>,
    investigation_deadline: Option<Instant>,
    investigation_target: Option<Target>,
    investigation_subject: Option<String>,
    check_results: HashMap<String, Vec<String>>,
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
    legacy: bool,
    motion: bool,
    focused: bool,
    flash: Option<Instant>,
    hits: std::cell::RefCell<Vec<(Rect, Control)>>,
}
#[derive(Clone, Copy)]
enum Control {
    Nav(usize),
    Row(usize),
    Review,
    Add,
    Investigate,
    Explore,
    Open,
    Findings,
}

impl Workspace {
    pub(super) fn new(app: &App) -> Self {
        let mut result = Self::empty();
        result.history = care::sessions(&app.account_home);
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
            screen: Screen::Findings,
            cursor: 0,
            nav: 0,
            focus: 1,
            detail: false,
            coverage: false,
            detail_scroll: 0,
            show_all: false,
            kept: HashSet::new(),
            stage: "Starting assessment".into(),
            complete: false,
            volume: None,
            metrics: Metrics::default(),
            trend: VecDeque::new(),
            revision: 0,
            started: Instant::now(),
            triage_work: None,
            triage: None,
            triage_reasons: HashMap::new(),
            insight_work: None,
            insight: None,
            insight_cache: HashMap::new(),
            result_work: None,
            result_insight: None,
            result_summary_session: None,
            insight_scope: vec![],
            insight_inputs: vec![],
            ai_error: None,
            ai_framework: ai::FrameworkStatus::Detecting,
            ai_framework_work: None,
            auto_requested: false,
            auto_investigation_started: false,
            investigation: None,
            investigation_case: None,
            pending_checks: VecDeque::new(),
            investigation_deadline: None,
            investigation_target: None,
            investigation_subject: None,
            check_results: HashMap::new(),
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
            legacy: false,
            motion: std::env::var_os("REDUCE_MOTION").is_none(),
            focused: true,
            flash: None,
            hits: Default::default(),
        }
    }
    fn visible(&self) -> Vec<&Finding> {
        let mut quick = 0;
        let mut areas = 0;
        self.findings
            .iter()
            .filter(|f| !self.kept.contains(&f.id))
            .filter(|f| {
                if self.show_all {
                    return true;
                }
                if f.quick_win {
                    quick += 1;
                    quick <= 3
                } else {
                    areas += 1;
                    areas <= 5 || matches!(f.target, Target::System)
                }
            })
            .collect()
    }
    fn selected(&self) -> Option<&Finding> {
        self.visible().get(self.cursor).copied()
    }
    fn apply_triage(&mut self, triage: ai::Triage) {
        let mut rank = HashMap::new();
        let mut reasons = HashMap::new();
        for (index, id) in triage.quick_win_ids.iter().enumerate() {
            rank.insert(id.clone(), (0_u8, index));
        }
        for (index, id) in triage.key_area_ids.iter().enumerate() {
            rank.insert(id.clone(), (1_u8, index));
        }
        for (index, id) in triage
            .quick_win_ids
            .iter()
            .chain(triage.key_area_ids.iter())
            .enumerate()
        {
            if let Some(reason) = triage.reasons.get(index) {
                reasons.insert(id.clone(), ai::display_text(reason));
            }
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
        self.triage_reasons = reasons;
        self.triage = Some(triage);
        self.cursor = self.cursor.min(self.visible().len().saturating_sub(1));
    }
    fn rebuild(&mut self, app: &App) {
        let selected = self.selected().map(|f| f.id.clone());
        let mut incoming = care::findings(
            &app.entries,
            &app.processes,
            &self.metrics,
            app.inventory.as_ref(),
        );
        if let Some(volume) = &self.volume {
            let free = volume.disk_free_kb();
            let ratio = free as f64 / volume.capacity_kb.max(1) as f64;
            if free < 10 * 1_048_576 || ratio < 0.10 {
                let urgent = free < 5 * 1_048_576 || ratio < 0.05;
                incoming.insert(0,Finding{id:"system:disk".into(),title:if urgent{"Disk space is very low"}else{"Disk space needs attention"}.into(),observation:format!("{} free · {:.1}% of capacity. Start with confirmed quick wins, then inspect large useful data.",format_kb(free),ratio*100.),consequence:"Capacity is measured; reclaimable space is only the eligible cleanup targets. Large useful folders may be moved instead of deleted.".into(),size_kb:0,quick_win:false,target:Target::System,related_pids:vec![]});
            }
        }
        let old: HashMap<_, _> = self
            .findings
            .iter()
            .enumerate()
            .map(|(i, f)| (f.id.clone(), i))
            .collect();
        incoming.sort_by_key(|f| old.get(&f.id).copied().unwrap_or(usize::MAX));
        self.findings = incoming;
        if let Some(id) = selected {
            self.cursor = self
                .visible()
                .iter()
                .position(|f| f.id == id)
                .unwrap_or(self.cursor);
        }
        self.cursor = self.cursor.min(self.visible().len().saturating_sub(1));
        if self
            .insight_inputs
            .iter()
            .any(|s| !self.findings.iter().any(|f| supports_subject(f, s)))
        {
            self.insight = None;
        }
        if self.triage.as_ref().is_some_and(|triage| {
            triage
                .key_area_ids
                .iter()
                .chain(triage.quick_win_ids.iter())
                .any(|id| !self.findings.iter().any(|finding| &finding.id == id))
        }) {
            self.triage = None;
            self.triage_reasons.clear();
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
                    if self.metrics.pressure == Some(4)
                        && (self.insight_work.is_some() || self.triage_work.is_some())
                    {
                        self.insight_work = None;
                        self.triage_work = None;
                        self.ai_error =
                            Some("Local AI paused while memory pressure is critical.".into());
                    }
                    self.trend
                        .push_back(self.metrics.cpu.values().sum::<f64>().round() as u64);
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
                care::Event::Finished => {
                    self.complete = true;
                    self.auto_requested = false;
                    self.stage = if app.inventory.as_ref().is_some_and(|i| i.complete) {
                        "Assessment complete"
                    } else {
                        "Assessment partial · inspect coverage"
                    }
                    .into();
                    self.session.measurements = app
                        .entries
                        .iter()
                        .filter(|e| {
                            !matches!(
                                e.status,
                                CacheStatus::ScanError
                                    | CacheStatus::Missing
                                    | CacheStatus::Symlink
                                    | CacheStatus::Invalid
                            )
                        })
                        .map(|e| (e.spec.path.display().to_string(), e.size_kb))
                        .collect();
                    if let Some(inventory) = app.inventory.as_ref().filter(|i| i.complete) {
                        self.session.measurements.extend(
                            inventory
                                .top_level
                                .iter()
                                .map(|i| (i.path.display().to_string(), i.size_kb)),
                        );
                    }
                    self.session.state = self.stage.clone();
                    self.session.after = Some(self.metrics.clone());
                    if let Some(volume) = &self.volume {
                        self.session.free_after_kb = Some(volume.disk_free_kb());
                    }
                    self.persist(app);
                    self.history = care::sessions(&app.account_home);
                }
            }
        }
        if changed {
            self.rebuild(app);
        }
        if !self.auto_investigation_started
            && self.started.elapsed() >= Duration::from_secs(20)
            && (self.metrics.pressure.is_some_and(|level| level >= 2)
                || self.findings.iter().any(|finding| {
                    matches!(finding.target, Target::Process(..))
                        && finding.title.eq_ignore_ascii_case("fseventsd")
                }))
            && self.investigation.is_none()
            && self.insight_work.is_none()
            && let Some(target_id) = self
                .findings
                .iter()
                .find(|finding| {
                    matches!(finding.target, Target::Process(..))
                        && finding.title.eq_ignore_ascii_case("fseventsd")
                })
                .map(|finding| finding.id.clone())
        {
            self.show_all = true;
            if let Some(index) = self
                .visible()
                .iter()
                .position(|finding| finding.id == target_id)
            {
                self.cursor = index;
            }
            self.auto_investigation_started = true;
            self.note = Some(
                "Automatic investigation started for sustained fseventsd memory pressure.".into(),
            );
            self.start_ai(app, true, true, true);
        }
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
                    self.apply_triage(triage);
                    self.flash = Some(Instant::now());
                    self.ai_error = None;
                }
                Ok(_) => {
                    self.ai_error = Some(
                        "Evidence changed while triage was running. The measured order is still used.".into(),
                    );
                }
                Err(error) => self.ai_error = Some(error),
            }
        }
        if let Some(result) =
            self.insight_work
                .as_ref()
                .and_then(|work| match work.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Disconnected) => Some(Err("AI worker stopped".into())),
                    Err(TryRecvError::Empty) => None,
                })
        {
            let work = self.insight_work.take().expect("AI work");
            match result {
                Ok(insight) => {
                    let valid =
                        work.request.subjects.iter().all(|subject| {
                            self.findings.iter().any(|f| supports_subject(f, subject))
                        });
                    if valid {
                        self.insight_scope = insight.evidence_ids.clone();
                        self.insight_inputs = work.request.subjects.clone();
                        self.insight_cache
                            .insert(ai::cache_key(&work.request), insight.clone());
                        self.flash = Some(Instant::now());
                        self.ai_error = None;
                        if work.request.investigation {
                            self.pending_checks = insight.next_checks.iter().cloned().collect();
                            let selected_is_fseventsd = self
                                .investigation_target
                                .as_ref()
                                .and_then(|target| match target {
                                    Target::Process(pid, identity) => app
                                        .processes
                                        .iter()
                                        .find(|process| {
                                            process.pid == *pid && process.start_time == *identity
                                        })
                                        .map(|process| {
                                            Path::new(&process.command)
                                                .file_name()
                                                .and_then(|name| name.to_str())
                                                .is_some_and(|name| {
                                                    name.eq_ignore_ascii_case("fseventsd")
                                                })
                                        }),
                                    _ => None,
                                })
                                .unwrap_or(false);
                            if selected_is_fseventsd
                                && !self.pending_checks.iter().any(|check| check == "fs_usage")
                            {
                                self.pending_checks.push_back("fs_usage".into());
                            }
                            if self.pending_checks.is_empty() {
                                self.pending_checks.push_back("inspect_children".into());
                            }
                        }
                        self.insight = Some(insight);
                    } else {
                        self.ai_error = Some(
                            "Evidence changed. Select Investigate to refresh the insight.".into(),
                        );
                    }
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
        if let Some(result) = self
            .investigation
            .as_ref()
            .and_then(|work| work.receiver.try_recv().ok())
        {
            let work = self.investigation.take().expect("investigation");
            if let Some(case) = &mut self.investigation_case {
                case.record_check(work.check.clone());
                let (kind, summary, supports) = match &result {
                    InvestigationResult::Inventory(inventory) => (
                        investigation::EvidenceKind::VolumeContext,
                        format!(
                            "Measured folder evidence: {} observed, complete={}, {} unreadable entries.",
                            format_kb(inventory.scanned_kb),
                            inventory.complete,
                            inventory.scan_errors
                        ),
                        vec!["volume_specific"],
                    ),
                    InvestigationResult::Note(note) => {
                        let kind = match work.check.as_str() {
                            "fs_usage" => investigation::EvidenceKind::FilesystemActivity,
                            "volume_context" => investigation::EvidenceKind::VolumeContext,
                            "research_sources" => investigation::EvidenceKind::Research,
                            _ => investigation::EvidenceKind::ProcessSample,
                        };
                        let supports = match kind {
                            investigation::EvidenceKind::FilesystemActivity => {
                                vec!["filesystem_activity"]
                            }
                            investigation::EvidenceKind::VolumeContext => {
                                vec!["volume_specific"]
                            }
                            _ => Vec::new(),
                        };
                        (kind, ai::display_text(note), supports)
                    }
                };
                let mut evidence = investigation::evidence(
                    format!("{}:{}", case.id, work.check),
                    kind,
                    &case.target,
                    summary,
                    &supports,
                    &[],
                    true,
                );
                if kind == investigation::EvidenceKind::Research {
                    evidence.source = Some("fixed Apple source catalog".into());
                }
                case.add_evidence(evidence);
            }
            let check_observation = match &result {
                InvestigationResult::Inventory(inventory) => format!(
                    "Read-only folder check at {}: {} observed; complete={}; {} unreadable entries. Largest children: {}",
                    care::timestamp(),
                    format_kb(inventory.scanned_kb),
                    inventory.complete,
                    inventory.scan_errors,
                    inventory
                        .roots
                        .first()
                        .and_then(|r| inventory.children.get(&r.path))
                        .map(|items| items
                            .iter()
                            .take(3)
                            .map(|i| format!(
                                "{} {}",
                                ai::display_text(
                                    &i.path.file_name().unwrap_or_default().to_string_lossy()
                                ),
                                format_kb(i.size_kb)
                            ))
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or_default()
                ),
                InvestigationResult::Note(note) => {
                    format!("Read-only check at {}: {note}", care::timestamp())
                }
            };
            if let Some(id) = &self.investigation_subject {
                let notes = self.check_results.entry(id.clone()).or_default();
                notes.push(check_observation);
                if notes.len() > 3 {
                    notes.remove(0);
                }
            }
            match result {
                InvestigationResult::Inventory(inventory) => {
                    let inventory = *inventory;
                    let complete = inventory.complete;
                    if let Some(existing) = &mut app.inventory {
                        existing.children.extend(inventory.children);
                    } else {
                        app.inventory = Some(inventory);
                    }
                    self.note = Some(if complete{"Selected folder measured. Explore shows its children."}else{"Selected folder scan is partial. Unreadable or cancelled entries are not empty."}.into());
                    self.revision += 1;
                    self.flash = Some(Instant::now());
                }
                InvestigationResult::Note(note) => self.note = Some(note),
            }
            self.pending_checks = work.checks.clone();
            if let Some(case) = &mut self.investigation_case {
                let decision = case.next_local_decision();
                case.apply_decision(&decision);
                match decision {
                    investigation::AgentDecision::Check { check, .. } => {
                        if !self.pending_checks.iter().any(|pending| pending == &check) {
                            self.pending_checks.push_front(check);
                        }
                    }
                    investigation::AgentDecision::Research { .. } => {
                        if !self
                            .pending_checks
                            .iter()
                            .any(|pending| pending == "research_sources")
                        {
                            self.pending_checks.push_front("research_sources".into());
                        }
                    }
                    investigation::AgentDecision::AwaitApproval { .. }
                    | investigation::AgentDecision::Finish { .. } => {}
                }
            }
            if self.pending_checks.is_empty() {
                let deadline = self.investigation_deadline;
                if self
                    .investigation_subject
                    .as_ref()
                    .is_some_and(|id| self.selected().is_some_and(|f| &f.id == id))
                    && !self.reviewing
                    && !self.clearing_history
                    && self.work.is_none()
                {
                    self.start_ai(app, true, false, false);
                    self.investigation_deadline = deadline;
                } else {
                    self.investigation_deadline = None;
                }
            }
            self.investigation_target = Some(work.target.clone());
        }
        if self
            .investigation
            .as_ref()
            .is_some_and(|work| Instant::now() > work.deadline)
        {
            self.investigation = None;
            self.pending_checks.clear();
            self.note = Some(
                "Investigation time limit reached. Existing evidence remains available.".into(),
            );
        }
        if self
            .insight_work
            .as_ref()
            .is_some_and(|work| !work.request.checks.is_empty())
            && self
                .investigation_deadline
                .is_some_and(|deadline| Instant::now() > deadline)
        {
            self.insight_work = None;
            self.ai_error = Some(
                "Investigation time limit reached. Measured check results remain available.".into(),
            );
        }
        if self.investigation.is_none()
            && self.insight_work.is_none()
            && let Some(check) = self.pending_checks.pop_front()
        {
            self.start_check(app, check);
        }
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
                    self.screen = Screen::History;
                    self.nav = 2;
                    self.history = care::sessions(&app.account_home);
                    self.history.insert(0, self.session.clone());
                    self.history.dedup_by_key(|s| s.id);
                    self.history_cursor = 0;
                    self.stage = "Results ready · Recheck to assess again".into();
                    self.flash = Some(Instant::now());
                    self.insight = None;
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
            && self.started.elapsed() > Duration::from_secs(10)
            && (self.findings.len() >= 3 || self.complete)
            && !self.findings.is_empty()
            && self.work.is_none()
            && self.insight_work.is_none()
            && self.triage_work.is_none()
            && self.investigation.is_none()
            && !self.reviewing
            && !self.legacy
        {
            self.start_triage();
        }
    }
    fn persist(&mut self, app: &App) {
        self.session.updated = care::timestamp();
        if let Err(error) = care::save_session(&app.account_home, &self.session) {
            self.note = Some(format!("History could not be saved: {error}"));
        }
    }
    fn start_ai(&mut self, app: &App, selected_only: bool, investigation: bool, automatic: bool) {
        if self.metrics.pressure == Some(4) {
            self.auto_requested = true;
            self.ai_error = Some("Local AI paused while memory pressure is critical.".into());
            return;
        }
        self.insight_work = None;
        self.investigation = None;
        self.pending_checks.clear();
        self.investigation_deadline = Some(Instant::now() + Duration::from_secs(60));
        let subjects: Vec<_> = if selected_only {
            self.selected().into_iter().collect()
        } else {
            self.visible().into_iter().take(8).collect()
        };
        if subjects.is_empty() {
            return;
        }
        let request = ai::Request {
            revision: self.revision,
            checks: subjects
                .iter()
                .filter_map(|f| {
                    self.check_results
                        .get(&f.id)
                        .map(|notes| (f.id.clone(), notes.clone()))
                })
                .collect(),
            investigation,
            subjects: subjects.iter().map(|f| subject_for_finding(f)).collect(),
        };
        let target = subjects.first().map(|f| f.target.clone());
        let subject = investigation.then(|| subjects[0].id.clone());
        let selected_is_fseventsd = target.as_ref().and_then(|target| match target {
            Target::Process(pid, identity) => app
                .processes
                .iter()
                .find(|process| process.pid == *pid && process.start_time == *identity)
                .map(|process| {
                    Path::new(&process.command)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.eq_ignore_ascii_case("fseventsd"))
                }),
            _ => None,
        }) == Some(true);
        if investigation {
            self.investigation_case = selected_is_fseventsd.then(|| {
                investigation::InvestigationCase::new_fseventsd(
                    "fseventsd",
                    self.revision,
                    automatic,
                )
            });
        }
        let cache_key = ai::cache_key(&request);
        if let Some(cached) = self.insight_cache.get(&cache_key).cloned() {
            self.insight_scope = cached.evidence_ids.clone();
            self.insight_inputs = request.subjects.clone();
            if investigation {
                self.pending_checks = cached.next_checks.iter().cloned().collect();
                if selected_is_fseventsd
                    && !self.pending_checks.iter().any(|check| check == "fs_usage")
                {
                    self.pending_checks.push_back("fs_usage".into());
                }
            }
            self.investigation_target = target;
            self.investigation_subject = subject;
            self.insight = Some(cached);
            self.ai_error = None;
            self.auto_requested = true;
            return;
        }
        self.investigation_target = target;
        self.investigation_subject = subject;
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let input = request.clone();
        let worker = thread::spawn(move || {
            let _ = sender.send(ai::explain(&input, &flag));
        });
        self.insight_work = Some(InsightWork {
            receiver,
            worker: Some(worker),
            cancel,
            request,
            started: Instant::now(),
        });
        self.ai_error = None;
        self.auto_requested = true;
    }
    fn start_triage(&mut self) {
        if self.metrics.pressure == Some(4) {
            self.auto_requested = true;
            self.ai_error = Some("Local AI paused while memory pressure is critical.".into());
            return;
        }
        let subjects: Vec<_> = self
            .findings
            .iter()
            .filter(|finding| !self.kept.contains(&finding.id))
            .take(8)
            .map(subject_for_finding)
            .collect();
        if subjects.is_empty() {
            return;
        }
        let request = ai::Request {
            revision: self.revision,
            checks: Default::default(),
            investigation: false,
            subjects,
        };
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let input = request.clone();
        let worker = thread::spawn(move || {
            let result =
                ai::triage(&input, &flag).unwrap_or_else(|_| ai::deterministic_triage(&input));
            let _ = sender.send(Ok(result));
        });
        self.triage_work = Some(TriageWork {
            receiver,
            worker: Some(worker),
            cancel,
            request,
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
            checks: Default::default(),
            investigation: false,
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
            started: Instant::now(),
        });
    }
    fn start_check(&mut self, app: &mut App, check: String) {
        let Some(target) = self.investigation_target.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let home = app.account_home.clone();
        let selected = target.clone();
        let selected_command = match &target {
            Target::Process(pid, identity) => app
                .processes
                .iter()
                .find(|process| process.pid == *pid && process.start_time == *identity)
                .map(|process| process.command.clone()),
            _ => None,
        };
        let history = self.history.clone();
        let check_name = check.clone();
        thread::spawn(move || {
            let path = match &selected {
                Target::Cache(path) | Target::Folder(path) => Some(path.clone()),
                _ => None,
            };
            let result = match check.as_str() {
                "inspect_children" => {
                    if let Some(path) = path {
                        InvestigationResult::Inventory(Box::new(
                            StorageInventory::scan_with_cancel(&path, &home, &flag),
                        ))
                    } else {
                        InvestigationResult::Note("Process evidence refreshes every two seconds. Inspect the family and sample window.".into())
                    }
                }
                "check_open_handles" => InvestigationResult::Note(if let Some(path) = path {
                    match cache::path_is_open(&path){Ok(true)=>"An open handle was found. Keep this data while it is in use.".into(),Ok(false)=>"No open handles were found by this check. Cleanup policy still applies.".into(),Err(e)=>format!("Open-handle check unavailable: {e}")}
                } else {
                    "Select a folder for an open-handle check.".into()
                }),
                "compare_history" => InvestigationResult::Note(if let Some(path) = path {
                    let values: Vec<_> = history
                        .iter()
                        .filter_map(|s| s.measurements.get(&path.display().to_string()))
                        .take(2)
                        .collect();
                    if values.len() == 2 {
                        format!(
                            "Prior complete measurements: {} and {}. This does not identify the writer.",
                            format_kb(*values[0]),
                            format_kb(*values[1])
                        )
                    } else {
                        "No comparable complete history yet.".into()
                    }
                } else {
                    "Process history records outcomes; it does not prove ownership of generated files.".into()
                }),
                "refresh_processes" => {
                    match &selected {
                        Target::Process(pid, identity) => match review_processes() {
                            Ok(processes) => {
                                let exact = processes
                                    .iter()
                                    .find(|process| process.pid == *pid && process.start_time == *identity);
                                InvestigationResult::Note(match exact {
                                    Some(process) => format!(
                                        "Current process identity is still present: PID {} · {} · parent {}. CPU and memory readings continue in the assessment window.",
                                        process.pid,
                                        ai::display_text(&process.command),
                                        process.parent_pid
                                    ),
                                    None => "The selected process identity is no longer present in the current-account sample. A replacement process is not assumed to be the same workload.".into(),
                                })
                            }
                            Err(error) => InvestigationResult::Note(format!(
                                "Process refresh unavailable: {error}"
                            )),
                        },
                        _ => InvestigationResult::Note(
                            "The current assessment already refreshes process readings every two seconds; no process family is verified for this folder.".into(),
                        ),
                    }
                }
                "fs_usage" => {
                    let is_fseventsd = selected_command.as_deref().is_some_and(|command| {
                        Path::new(command)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.eq_ignore_ascii_case("fseventsd"))
                    });
                    if is_fseventsd {
                        InvestigationResult::Note(
                            care::observe_fseventsd(&flag).unwrap_or_else(|error| {
                                format!(
                                    "Filesystem activity probe unavailable: {error}. It uses sudo -n and never opens a password prompt."
                                )
                            }),
                        )
                    } else {
                        InvestigationResult::Note(
                            "Filesystem activity tracing is limited to the selected fseventsd daemon.".into(),
                        )
                    }
                }
                "volume_context" => InvestigationResult::Note(
                    care::observe_volume_context(&flag).unwrap_or_else(|error| {
                        format!("Mounted-volume context unavailable: {error}")
                    }),
                ),
                "research_sources" => InvestigationResult::Note(
                    care::research_sources(&flag)
                        .unwrap_or_else(|error| format!("Source research unavailable: {error}")),
                ),
                _ => InvestigationResult::Note("Unsupported investigation check.".into()),
            };
            if !flag.load(Ordering::Relaxed) {
                let _ = sender.send(result);
            }
        });
        self.investigation = Some(Investigation {
            receiver,
            stop,
            deadline: *self
                .investigation_deadline
                .get_or_insert_with(|| Instant::now() + Duration::from_secs(60)),
            check: check_name,
            checks: self.pending_checks.clone(),
            target,
        });
        self.pending_checks.clear();
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
        match finding.target {
            Target::Cache(path) => {
                if let Some(entry) = app.entries.iter().find(|e| e.spec.path == path).cloned() {
                    if matches!(
                        entry.status,
                        CacheStatus::Ready | CacheStatus::Optional | CacheStatus::InUse
                    ) {
                        self.add_action(Action::Clean(entry));
                    } else if entry.status == CacheStatus::Review {
                        self.add_action(Action::ReviewClean(entry));
                    } else {
                        self.note = Some(entry.status.explanation().into());
                    }
                }
            }
            Target::Process(pid, identity) => {
                if let Some(process) = app
                    .processes
                    .iter()
                    .find(|p| p.pid == pid && p.start_time == identity)
                {
                    if process.signalable {
                        self.add_action(Action::Signal(
                            process.clone(),
                            if force {
                                ProcessSignal::Kill
                            } else {
                                ProcessSignal::Terminate
                            },
                        ));
                    } else {
                        self.note = process.signal_block_reason.clone();
                    }
                }
            }
            _ => {
                self.note = Some(
                    "Inspect this area or choose a supported action on one of its findings.".into(),
                )
            }
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
        self.assessment = None;
        self.insight_work = None;
        self.triage_work = None;
        self.result_work = None;
        self.investigation = None;
        self.pending_checks.clear();
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
    fn navigate(&mut self, index: usize) {
        self.nav = index.min(2);
        self.screen = [Screen::Findings, Screen::Explore, Screen::History][self.nav];
        self.detail_scroll = 0;
    }
    fn inspect(&mut self, app: &mut App) {
        if let Some(finding) = self.selected().cloned() {
            match finding.target {
                Target::Folder(path) | Target::Cache(path) => {
                    app.explorer_path = Some(path.clone());
                    app.explorer_cursor = 0;
                    self.navigate(1);
                    if !app
                        .inventory
                        .as_ref()
                        .is_some_and(|i| i.children.contains_key(&path))
                    {
                        self.investigation_target = Some(Target::Folder(path));
                        self.start_check(app, "inspect_children".into());
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
        self.check_results.clear();
        self.investigation_subject = None;
        self.assessment = None;
        self.insight_work = None;
        self.triage_work = None;
        self.result_work = None;
        self.investigation = None;
        self.pending_checks.clear();
        self.findings.clear();
        app.entries.clear();
        app.inventory = None;
        self.kept.clear();
        self.insight = None;
        self.insight_cache.clear();
        self.result_insight = None;
        self.result_summary_session = None;
        self.triage = None;
        self.triage_reasons.clear();
        self.plan.clear();
        self.session = Session::default();
        self.started = Instant::now();
        self.complete = false;
        self.auto_requested = false;
        self.stage = "Rechecking storage and activity".into();
        self.navigate(0);
        self.cursor = 0;
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
                            self.insight = None;
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
                KeyCode::Right | KeyCode::Down => self.navigate((self.nav + 1).min(2)),
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
        if self.detail && key.code == KeyCode::Esc {
            self.detail = false;
            self.detail_scroll = 0;
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
                self.navigate(0);
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
            self.insight_work = None;
            self.investigation_subject = None;
            self.pending_checks.clear();
            self.investigation_deadline = None;
            if let Some(item) = app.explorer_items().get(app.explorer_cursor) {
                self.investigation_target = Some(Target::Folder(item.path.clone()));
                self.start_check(app, "inspect_children".into());
            }
            return true;
        }
        match key.code {
            KeyCode::Char('q')=>app.quit=true,
            KeyCode::Tab=>self.focus=(self.focus+1)%2,
            KeyCode::BackTab=>self.focus=(self.focus+1)%2,
            KeyCode::Left if self.focus==0=>self.navigate(self.nav.saturating_sub(1)),
            KeyCode::Right if self.focus==0=>self.navigate((self.nav+1).min(2)),
            KeyCode::Enter if self.focus==0=>self.focus=1,
            KeyCode::Char('p')=>{self.reviewing=true;self.review_scroll=0;self.acknowledgement.clear();self.signals_ack=false;self.moves_ack=false;},
            KeyCode::Char('M')=>self.motion= !self.motion,
            KeyCode::Char('A')=>{thread::spawn(||{let _=Command::new("/usr/bin/open").args(["-b","com.apple.systempreferences"]).status();});},
            KeyCode::Char('d')=>{self.detail= !self.detail;self.detail_scroll=0;},
            KeyCode::Char('r')=>{if self.plan.is_empty(){self.recheck(app);}else{self.note=Some("Review or clear the pending plan before rechecking.".into());}},
            KeyCode::Char('?')=>self.note=Some("Tab: menu/content · arrows: choose · Enter: inspect · Space: add/remove action · i: AI investigation · e: explore · P: processes · m: move · p: review plan · f: all findings · K: keep · r: recheck · M: motion · q: quit".into()),
            KeyCode::Char('P')=>{app.phase=Phase::Processes;self.legacy=true;},
            KeyCode::Char('i')=>self.start_ai(app,true,true,false),
            KeyCode::Char('f')=>{self.show_all= !self.show_all;self.cursor=0;},
            KeyCode::Char('K')=>{if let Some(f)=self.selected(){if matches!(f.target,Target::System){self.note=Some("System pressure stays visible until readings change.".into());}else{self.kept.insert(f.id.clone());}}self.cursor=self.cursor.min(self.visible().len().saturating_sub(1));},
            KeyCode::Char('e')=>self.inspect(app),
            KeyCode::Char(' ') if self.screen==Screen::Findings=>self.add_selected(app,false),
            KeyCode::Char('x') if self.screen==Screen::Findings=>self.add_selected(app,true),
            KeyCode::Char('m')=>{if !app.analysis_only{app.open_relocation_sources();self.legacy=true;}},
            KeyCode::Char('o')=>{
                let path=if self.screen==Screen::Explore{app.explorer_items().get(app.explorer_cursor).map(|i|i.path.clone())}else{self.selected().and_then(|f|match &f.target{Target::Cache(p)|Target::Folder(p)=>Some(p.clone()),_=>None})};
                if let Some(path)=path {thread::spawn(move||{let _=Command::new("/usr/bin/open").arg("-R").arg(path).status();});}
            },
            KeyCode::Down|KeyCode::Char('j')=>match self.screen{Screen::Findings=>self.cursor=(self.cursor+1).min(self.visible().len().saturating_sub(1)),Screen::Explore=>app.explorer_cursor=(app.explorer_cursor+1).min(app.explorer_items().len().saturating_sub(1)),Screen::History=>self.history_cursor=(self.history_cursor+1).min(self.history.len().saturating_sub(1))},
            KeyCode::Up|KeyCode::Char('k')=>match self.screen{Screen::Findings=>self.cursor=self.cursor.saturating_sub(1),Screen::Explore=>app.explorer_cursor=app.explorer_cursor.saturating_sub(1),Screen::History=>self.history_cursor=self.history_cursor.saturating_sub(1)},
            KeyCode::Enter=>if self.screen==Screen::Explore{app.open_consumer();}else if self.screen==Screen::Findings{self.inspect(app);},
            KeyCode::Esc|KeyCode::Backspace|KeyCode::Left=>if self.screen==Screen::Explore{app.explorer_back();}else{self.note=None;self.detail=false;},
            KeyCode::PageDown=>self.detail_scroll=self.detail_scroll.saturating_add(5),
            KeyCode::PageUp=>self.detail_scroll=self.detail_scroll.saturating_sub(5),_=>{}
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
        if self.reviewing || self.clearing_history || self.work.is_some() {
            return true;
        }
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
                self.navigate(1);
                return true;
            }
        }
        match control {
            Some(Control::Nav(index)) => {
                self.navigate(index);
                self.focus = 1;
            }
            Some(Control::Row(index)) => match self.screen {
                Screen::Findings => self.cursor = index,
                Screen::Explore => app.explorer_cursor = index,
                Screen::History => self.history_cursor = index,
            },
            Some(control) => {
                let code = match control {
                    Control::Open => {
                        app.open_consumer();
                        return true;
                    }
                    Control::Findings => 'f',
                    Control::Review => 'p',
                    Control::Add => ' ',
                    Control::Investigate => 'i',
                    Control::Explore => 'e',
                    _ => return true,
                };
                self.key(app, KeyEvent::new(KeyCode::Char(code), KeyModifiers::NONE));
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
    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(app.color(INK)))
            .block(panel(app, title))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
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
    let style = if active && w.focus == 0 {
        selected_row_style(app)
    } else if active {
        Style::default().fg(app.color(BLUE)).bold()
    } else {
        Style::default().fg(app.color(MUTED))
    };
    frame.render_widget(Paragraph::new(label).style(style), area);
    w.hits.borrow_mut().push((area, control));
}
pub(super) fn render(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
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
    w.hits.borrow_mut().clear();
    let regions = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(2),
    ])
    .split(area);
    let menu = Layout::horizontal([
        Constraint::Length(if area.width < 90 { 3 } else { 15 }),
        Constraint::Length(11),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Min(0),
        Constraint::Length(22),
    ])
    .split(regions[0]);
    frame.render_widget(
        Paragraph::new(if area.width < 90 {
            " M "
        } else {
            " MAC CLEANUP"
        })
        .style(Style::default().fg(app.color(INK)).bold()),
        menu[0],
    );
    for (i, label) in [" Findings", " Explore", " History"].iter().enumerate() {
        button(
            frame,
            menu[i + 1],
            app,
            w,
            label.to_string(),
            Control::Nav(i),
            w.nav == i,
        );
    }
    button(
        frame,
        menu[5],
        app,
        w,
        format!(" Review plan ({}) [p]", w.plan.len()),
        Control::Review,
        w.reviewing,
    );
    let disk = w
        .volume
        .as_ref()
        .map(|v| format!("{} free", format_kb(v.disk_free_kb())))
        .unwrap_or_else(|| "Measuring disk".into());
    let status = format!(
        " {}  ·  {}  ·  Memory {}  ·  {}{}",
        disk,
        w.ai_framework.compact(),
        w.metrics.pressure_label(),
        w.stage,
        if app.analysis_only {
            " · READ ONLY"
        } else {
            ""
        }
    );
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(app.color(
            if w.metrics.pressure == Some(4) {
                CORAL
            } else {
                MUTED
            },
        ))),
        regions[1],
    );
    let body = regions[2];
    if w.legacy {
        match app.phase {
            Phase::Processes => {
                let split =
                    Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                        .split(body);
                render_process_table(frame, split[0], app);
                render_process_details(frame, split[1], app);
            }
            Phase::RelocationSources => render_relocation_sources(frame, body, app),
            Phase::RelocationDestination => render_relocation_destination(frame, body, app),
            Phase::RelocationPlanning | Phase::Relocating => {
                render_relocation_progress(frame, body, app)
            }
            Phase::RelocationResult => render_relocation_result(frame, body, app),
            Phase::ReviewConfirm => render_review_confirmation(frame, body, app),
            Phase::ProcessConfirm => render_process_confirmation(frame, body, app),
            _ => render_scan_page(frame, body, app),
        }
    } else if w.work.is_some() {
        text_panel(
            frame,
            body,
            app,
            " RUNNING REVIEWED PLAN ",
            format!(
                "{}\n\n{}\n\nEsc requests a stop after the current action. Results stay here.",
                w.session.state,
                w.session
                    .actions
                    .iter()
                    .map(|a| format!("{}\n{}", a.target, a.result))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            ),
            0,
        );
    } else if w.clearing_history {
        text_panel(
            frame,
            body,
            app,
            " CLEAR LOCAL SESSION HISTORY ",
            format!(
                "Remove saved assessment and action-session records from this Mac.\nThe separate deletion audit log is retained.\n\nType CLEAR and Enter: {}\nEsc cancels.",
                w.acknowledgement
            ),
            0,
        );
    } else if w.reviewing {
        let word = if w.plan.iter().any(|a| matches!(a, Action::ReviewClean(_))) {
            "DELETE"
        } else if w
            .plan
            .iter()
            .any(|a| matches!(a, Action::Signal(..) | Action::Move(_)))
        {
            "APPLY"
        } else {
            "CLEAN"
        };
        let review = Layout::vertical([Constraint::Min(4), Constraint::Length(4)]).split(body);
        text_panel(
            frame,
            review[0],
            app,
            " REVIEW EXACT ACTIONS · ↑↓ scroll ",
            format!(
                "{} actions · signals first · targets revalidated\n\n{}",
                w.plan.len(),
                w.plan
                    .iter()
                    .enumerate()
                    .map(|(i, a)| format!("{}. {}", i + 1, a.description()))
                    .collect::<Vec<_>>()
                    .join("\n\n")
            ),
            w.review_scroll,
        );
        frame.render_widget(
            Paragraph::new(format!(
                " Type {word} then Enter: {}\n {}{}\n Esc returns · Delete clears plan\n {}",
                w.acknowledgement,
                if w.plan.iter().any(|a| matches!(a, Action::Signal(..))) {
                    if w.signals_ack {
                        "✓ signals acknowledged · "
                    } else {
                        "s acknowledge signals · "
                    }
                } else {
                    ""
                },
                if w.plan.iter().any(|a| matches!(a, Action::Move(..))) {
                    if w.moves_ack {
                        "✓ relocation acknowledged"
                    } else {
                        "m acknowledge relocation"
                    }
                } else {
                    ""
                },
                w.note.as_deref().unwrap_or("")
            ))
            .style(Style::default().fg(app.color(CORAL)))
            .wrap(Wrap { trim: false }),
            review[1],
        );
    } else if w.detail {
        render_evidence(frame, body, app, w);
    } else {
        let split = if body.width >= 100 {
            Layout::horizontal([Constraint::Percentage(47), Constraint::Percentage(53)])
                .spacing(1)
                .split(body)
        } else {
            Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).split(body)
        };
        match w.screen {
            Screen::Findings => render_findings(frame, split[0], app, w),
            Screen::Explore => render_folders(frame, split[0], app, w),
            Screen::History => render_history_list(frame, split[0], app, w),
        }
        render_evidence(frame, split[1], app, w);
    }
    let commands = if w.reviewing {
        " PgUp/PgDn targets   Esc cancel   Delete clear plan   Enter confirm typed phrase"
    } else if w.work.is_some() {
        " Esc stop after current action · results will stay open"
    } else if w.screen == Screen::History && !w.reviewing && !w.clearing_history {
        " Tab menu   ↑↓ sessions   PgUp/PgDn details   r recheck   Delete clear history   q quit"
    } else if w.legacy && app.phase == Phase::Processes {
        " Esc back   ↑↓ choose   Space SIGTERM plan   x SIGKILL plan"
    } else if w.legacy {
        " Esc back   ↑↓ choose   Enter continue"
    } else {
        " Tab menu   ↑↓ choose   Enter inspect   Space plan   i investigate   P processes   v coverage   ? help"
    };
    frame.render_widget(
        Paragraph::new(commands)
            .style(Style::default().fg(app.color(BLUE)))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(app.color(FAINT))),
            ),
        regions[3],
    );
}
fn render_findings(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let block = panel(
        app,
        if w.triage_work.is_some() {
            " AI PRIORITIZING MEASURED AREAS · f all "
        } else if w.show_all {
            " ALL FINDINGS · f fewer "
        } else if !w.visible().iter().any(|f| f.quick_win) {
            " NO CONFIRMED QUICK WINS · f all "
        } else {
            " YOUR NEXT DECISIONS · f all "
        },
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let visible = w.visible();
    let capacity = (inner.height as usize / 3).max(1);
    let start = w.cursor.saturating_sub(capacity - 1);
    if visible.is_empty() {
        frame.render_widget(Paragraph::new("Gathering evidence from storage and running processes.\n\nFindings appear as checks finish.\n\nExplore remains available during assessment.").wrap(Wrap{trim:false}).style(Style::default().fg(app.color(MUTED))),inner);
        return;
    }
    for (row, (index, f)) in visible
        .iter()
        .enumerate()
        .skip(start)
        .take(capacity)
        .enumerate()
    {
        let rect = Rect::new(
            inner.x,
            inner.y + row as u16 * 3,
            inner.width,
            3.min(inner.height.saturating_sub(row as u16 * 3)),
        );
        let queued = w.plan.iter().any(|a| match (&f.target, a) {
            (Target::Cache(p), Action::Clean(e) | Action::ReviewClean(e)) => p == &e.spec.path,
            (Target::Process(pid, _), Action::Signal(p, _)) => *pid == p.pid,
            _ => false,
        });
        let cited = w
            .insight
            .as_ref()
            .is_some_and(|i| i.evidence_ids.contains(&f.id));
        let impact = if f.size_kb > 0 {
            format_kb(f.size_kb)
        } else {
            String::new()
        };
        let reason = w
            .triage_reasons
            .get(&f.id)
            .cloned()
            .unwrap_or_else(|| match &f.target {
                Target::Cache(path) => app
                    .entries
                    .iter()
                    .find(|e| &e.spec.path == path)
                    .map(|e| match e.status {
                        CacheStatus::Ready if f.quick_win => "Rebuildable cache",
                        CacheStatus::Ready => "Review what will be removed",
                        CacheStatus::Optional => "May need a download again",
                        CacheStatus::InUse => "Check the active application",
                        CacheStatus::Review => "App-managed data · inspect first",
                        CacheStatus::Whitelisted => "Protected from cleanup",
                        _ => "Incomplete · inspect coverage",
                    })
                    .unwrap_or("Inspect the measured evidence")
                    .to_string(),
                Target::Folder(_) => "Large folder · inspect or move".into(),
                _ => f.observation.clone(),
            });
        let title = format!(
            "{} {}  {}",
            if queued {
                "✓"
            } else if cited {
                "✦"
            } else if f.quick_win {
                "+"
            } else {
                "›"
            },
            truncate_middle(
                &f.title,
                (inner.width as usize).saturating_sub(impact.len() + 4)
            ),
            impact
        );
        let lines = vec![
            Line::styled(
                title,
                Style::default()
                    .fg(app.color(if f.quick_win { MINT } else { INK }))
                    .bold(),
            ),
            Line::styled(
                format!(
                    "  {}{} · {}",
                    if cited { "AI · " } else { "" },
                    if f.quick_win { "QUICK WIN" } else { "KEY AREA" },
                    reason
                ),
                Style::default().fg(app.color(MUTED)),
            ),
            Line::raw(""),
        ];
        frame.render_widget(
            Paragraph::new(lines).style(if index == w.cursor {
                selected_row_style(app)
            } else {
                Style::default()
            }),
            rect,
        );
        w.hits.borrow_mut().push((rect, Control::Row(index)));
    }
}
fn render_folders(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let title = format!(
        " {} · Enter opens · ← parent ",
        app.explorer_path
            .as_ref()
            .map(|p| ai::display_text(&p.display().to_string()))
            .unwrap_or_else(|| "Storage · largest first".into())
    );
    let block = panel(app, title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let items = app.explorer_items();
    let start = app
        .explorer_cursor
        .saturating_sub(inner.height.saturating_sub(1) as usize);
    if items.is_empty() {
        frame.render_widget(Paragraph::new("No measured children yet. The assessment or selected-folder investigation is still needed.").wrap(Wrap{trim:false}),inner);
    }
    for (row, (index, item)) in items
        .iter()
        .enumerate()
        .skip(start)
        .take(inner.height as usize)
        .enumerate()
    {
        let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        let name = item
            .path
            .file_name()
            .unwrap_or(item.path.as_os_str())
            .to_string_lossy();
        frame.render_widget(
            Paragraph::new(format!(
                "{} {:>10}  {}",
                if item.kind == StorageItemKind::Directory {
                    "▸"
                } else {
                    "·"
                },
                format_kb(item.size_kb),
                ai::display_text(&name)
            ))
            .style(if index == app.explorer_cursor {
                selected_row_style(app)
            } else {
                Style::default().fg(app.color(INK))
            }),
            rect,
        );
        w.hits.borrow_mut().push((rect, Control::Row(index)));
    }
}
fn render_history_list(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    let block = panel(app, " HISTORY · 30 days · Delete clears ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let start = w
        .history_cursor
        .saturating_sub(inner.height.saturating_sub(1) as usize / 2);
    if w.history.is_empty() {
        frame.render_widget(Paragraph::new("Completed assessments and action results appear here.\n\nHistory stays on this Mac.").wrap(Wrap{trim:false}),inner);
    }
    for (row, (index, s)) in w
        .history
        .iter()
        .enumerate()
        .skip(start)
        .take(inner.height as usize / 2)
        .enumerate()
    {
        let rect = Rect::new(inner.x, inner.y + row as u16 * 2, inner.width, 2);
        frame.render_widget(
            Paragraph::new(format!(
                "{} · {} actions\n{}",
                crate::history::format_timestamp(
                    std::time::UNIX_EPOCH + Duration::from_secs(s.updated)
                ),
                s.actions.len(),
                s.state
            ))
            .style(if index == w.history_cursor {
                selected_row_style(app)
            } else {
                Style::default().fg(app.color(MUTED))
            }),
            rect,
        );
        w.hits.borrow_mut().push((rect, Control::Row(index)));
    }
}
fn render_evidence(frame: &mut Frame<'_>, area: Rect, app: &App, w: &Workspace) {
    if w.coverage {
        frame.render_widget(
            Paragraph::new(coverage_lines(app))
                .block(panel(app, " SCAN COVERAGE · v close · PgUp/PgDn scroll "))
                .wrap(Wrap { trim: false })
                .scroll((w.detail_scroll, 0)),
            area,
        );
        return;
    }
    if w.screen == Screen::History {
        let text = w
            .history
            .get(w.history_cursor)
            .map(|s| {
                let outcome = w
                    .result_insight
                    .as_ref()
                    .filter(|_| w.result_summary_session == Some(s.id))
                    .map(|insight| format!("\n\nLOCAL AI OUTCOME\n{}", insight.summary))
                    .or_else(|| {
                        (w.result_summary_session == Some(s.id))
                            .then_some(w.result_work.as_ref())
                            .flatten()
                            .map(|_| "\n\nLOCAL AI OUTCOME\nInterpreting the recorded observations…".into())
                    })
                    .unwrap_or_default();
                format!(
                    "{}\n\nMeasured removal: {}\nFree space: {} → {}\n\nBEFORE\n{}\n\nAFTER\n{}\n\nThese are observations, not proof of a performance improvement.\n\n{}{}",
                    s.state,
                    format_kb(s.actions.iter().map(|a| a.removed_kb).sum()),
                    s.free_before_kb.map(format_kb).unwrap_or_else(|| "unavailable".into()),
                    s.free_after_kb.map(format_kb).unwrap_or_else(|| "unavailable".into()),
                    s.before.as_ref().map(Metrics::describe).unwrap_or_else(|| "not measured".into()),
                    s.after.as_ref().map(Metrics::describe).unwrap_or_else(|| "not measured".into()),
                    s.actions.iter().map(|a| format!("{}\n{}", a.target, a.result)).collect::<Vec<_>>().join("\n\n"),
                    outcome
                )
            })
            .unwrap_or_else(|| w.note.clone().unwrap_or_else(|| "No session selected.".into()));
        text_panel(
            frame,
            area,
            app,
            " RESULTS · PgUp/PgDn scroll ",
            text,
            w.detail_scroll,
        );
        return;
    }
    let parts = Layout::vertical([
        Constraint::Length(7.min(area.height / 3)),
        Constraint::Percentage(38),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .split(area);
    let active = w.insight_work.is_some() || w.investigation.is_some();
    let title = if let Some(work) = &w.insight_work {
        format!(
            " LOCAL AI · comparing evidence · {}s ",
            work.started.elapsed().as_secs()
        )
    } else if w.investigation.is_some() {
        " INVESTIGATION · measuring selected evidence ".into()
    } else if w.insight.is_some() {
        " LOCAL AI · evidence snapshot ".into()
    } else {
        " INSIGHT ".into()
    };
    let investigation_summary = w
        .investigation_case
        .as_ref()
        .map(|case| {
            let leading = case
                .hypotheses
                .iter()
                .filter(|hypothesis| {
                    matches!(
                        hypothesis.status,
                        investigation::HypothesisStatus::Leading
                            | investigation::HypothesisStatus::Supported
                    )
                })
                .map(|hypothesis| hypothesis.label.as_str())
                .collect::<Vec<_>>();
            format!(
                "\n\nINVESTIGATION · {:?}\nChecks {} / {} · Evidence {}\nLeading: {}",
                case.phase,
                case.decision_count,
                case.decision_budget,
                case.evidence.len(),
                if leading.is_empty() {
                    "none yet".into()
                } else {
                    leading.join("; ")
                }
            )
        })
        .unwrap_or_default();
    let framework_line = format!("Framework detected: {}", w.ai_framework.description());
    let ai_text = if active {
        format!(
            "{framework_line}\nReading measured evidence. Your findings and controls remain available.{investigation_summary}"
        )
    } else if let Some(insight) = &w.insight {
        format!(
            "{framework_line}\n{}\nFor: {}{}",
            insight.summary,
            insight
                .evidence_ids
                .iter()
                .filter_map(|id| w
                    .findings
                    .iter()
                    .find(|f| &f.id == id)
                    .map(|f| f.title.as_str()))
                .collect::<Vec<_>>()
                .join(", "),
            investigation_summary
        )
    } else if let Some(error) = &w.ai_error {
        format!(
            "{framework_line}\nAI unavailable · {error}\nMeasured evidence remains available. i retries; A opens System Settings."
        )
    } else {
        format!(
            "{framework_line}\nInsights will highlight useful next steps after the first measurements. No chat, no cloud upload."
        )
    };
    let block = panel(app, title).border_style(
        Style::default().fg(app.color(
            if active
                || w.flash
                    .is_some_and(|t| t.elapsed() < Duration::from_millis(900))
            {
                AI_VIOLET
            } else {
                FAINT
            },
        )),
    );
    frame.render_widget(
        Paragraph::new(ai_text)
            .block(block)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(app.color(INK))),
        parts[0],
    );
    if active && w.motion && w.focused && !app.no_color && parts[0].width > 4 {
        let offset = (w.started.elapsed().as_millis() / 70) as u16 % (parts[0].width - 2);
        for j in 0..3 {
            let x = parts[0].x + 1 + (offset + j) % (parts[0].width - 2);
            frame.render_widget(
                Paragraph::new("━").style(Style::default().fg(if j == 1 {
                    BLUE
                } else {
                    AI_VIOLET
                })),
                Rect::new(x, parts[0].y, 1, 1),
            );
        }
    }
    let selected_item = if w.screen == Screen::Explore {
        app.explorer_items()
            .get(app.explorer_cursor)
            .map(|i| (*i).clone())
    } else {
        w.selected().and_then(|f| match &f.target {
            Target::Cache(path) | Target::Folder(path) => app
                .inventory
                .as_ref()
                .and_then(|i| {
                    i.children
                        .values()
                        .flatten()
                        .chain(i.top_level.iter())
                        .find(|item| &item.path == path)
                })
                .cloned(),
            _ => None,
        })
    };
    if let Some(item) = &selected_item {
        folder_map::render_care_map(frame, parts[1], app, item);
    } else if w.screen == Screen::Explore
        || w.selected()
            .is_some_and(|f| matches!(f.target, Target::Cache(_) | Target::Folder(_)))
    {
        text_panel(frame,parts[1],app," FOLDER MAP ","Folder details are still unmeasured. Enter or e explores this finding and measures its children.".into(),0);
    } else if w.selected().is_some_and(|f| f.id == "system:disk")
        && let Some(volume) = &w.volume
    {
        frame.render_widget(
            Gauge::default()
                .block(panel(app, " DISK CAPACITY · not a reclaimable total "))
                .gauge_style(Style::default().fg(app.color(AMBER)))
                .ratio(
                    (volume.disk_used_kb() as f64 / volume.capacity_kb.max(1) as f64).clamp(0., 1.),
                )
                .label(format!(
                    "{} used · {} free",
                    format_kb(volume.disk_used_kb()),
                    format_kb(volume.disk_free_kb())
                )),
            parts[1],
        );
    } else if w.metrics.cpu.is_empty() {
        text_panel(
            frame,
            parts[1],
            app,
            " RESOURCE EVIDENCE ",
            "A CPU rate needs two complete samples. Missing readings remain unavailable.".into(),
            0,
        );
    } else {
        let data: Vec<_> = w.trend.iter().copied().collect();
        frame.render_widget(
            ratatui::widgets::Sparkline::default()
                .block(panel(
                    app,
                    " OBSERVED CPU · sampled current-account processes ",
                ))
                .data(&data)
                .style(Style::default().fg(app.color(BLUE))),
            parts[1],
        );
    }
    let detail = if w.screen == Screen::Explore {
        selected_item.map(|i|format!("{}\n{} on disk\n\nEach child is measured separately. Parent totals include descendants. Size alone does not mean waste.\n\nEnter opens children · o reveals in Finder",ai::display_text(&i.path.display().to_string()),format_kb(i.size_kb))).unwrap_or_else(||"Select a folder to inspect its children.".into())
    } else {
        w.selected().map(|f|format!("{}\n\n{}\n\n{}\n\n{}",f.title,f.observation,f.consequence,if f.related_pids.is_empty(){"No verified application/process relationship for this finding.".into()}else{format!("Associated process IDs: {:?}. An association does not prove a process wrote these files.",f.related_pids)})).unwrap_or_else(||"Progressive assessment is running. Findings will appear on the left.".into())
    };
    let mut detail = format!(
        "{}{}",
        detail,
        w.note
            .as_ref()
            .map(|n| format!("\n\n{n}"))
            .unwrap_or_default()
    );
    if w.screen == Screen::Findings
        && let Some(f) = w.selected()
    {
        if matches!(f.target, Target::Process(..) | Target::System) {
            detail.push_str(&format!("\n\n{}", w.metrics.describe()));
        }
        for pid in f.related_pids.iter().take(4) {
            if let Some(p) = app.processes.iter().find(|p| &p.pid == pid) {
                detail.push_str(&format!(
                    "\nPID {} ← parent {} · RSS {} · {}",
                    p.pid,
                    p.parent_pid,
                    w.metrics
                        .rss_kb
                        .get(pid)
                        .copied()
                        .map(format_kb)
                        .unwrap_or_else(|| "unavailable".into()),
                    ai::display_text(&p.command)
                ));
            }
        }
    }
    text_panel(
        frame,
        parts[2],
        app,
        " EVIDENCE & TRADEOFFS ",
        detail,
        w.detail_scroll,
    );
    let buttons = Layout::horizontal([
        Constraint::Percentage(34),
        Constraint::Percentage(33),
        Constraint::Percentage(33),
    ])
    .split(parts[3]);
    if w.screen == Screen::Explore {
        button(
            frame,
            buttons[0],
            app,
            w,
            " Enter Open".into(),
            Control::Open,
            false,
        );
        button(
            frame,
            buttons[1],
            app,
            w,
            " i Measure".into(),
            Control::Investigate,
            false,
        );
        button(
            frame,
            buttons[2],
            app,
            w,
            " f Actions".into(),
            Control::Findings,
            false,
        );
        return;
    }
    button(
        frame,
        buttons[0],
        app,
        w,
        " Space Add / remove".into(),
        Control::Add,
        false,
    );
    button(
        frame,
        buttons[1],
        app,
        w,
        " i Investigate".into(),
        Control::Investigate,
        false,
    );
    button(
        frame,
        buttons[2],
        app,
        w,
        " e Explore".into(),
        Control::Explore,
        false,
    );
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
    fn sample_window(cancel: &AtomicBool) -> Option<Metrics> {
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

    session.state = "Measuring pre-action baseline".into();
    checkpoint(&home, &mut session, &sender);
    session.before = sample_window(&cancel);
    session.free_before_kb = crate::storage::read_volume_stats(&root)
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
    session.after = sample_window(&cancel);
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
    session.free_after_kb = crate::storage::read_volume_stats(&root)
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
        let cli = Cli::parse_from(["mac-cleanup"]);
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
        (home, app, w)
    }
    fn press(w: &mut Workspace, app: &mut App, key: KeyCode) {
        assert!(w.key(app, KeyEvent::new(key, KeyModifiers::NONE)));
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
    fn tab_focus_has_a_visible_single_line_menu_highlight() {
        let (_home, mut app, mut w) = fixture();
        app.terminal_width = 120;
        app.terminal_height = 30;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &app, &w))
            .unwrap();
        let content_style = terminal.backend().buffer()[(16, 0)].style();
        press(&mut w, &mut app, KeyCode::Tab);
        terminal
            .draw(|frame| render(frame, frame.area(), &app, &w))
            .unwrap();
        let menu_style = terminal.backend().buffer()[(16, 0)].style();
        assert_ne!(content_style.bg, menu_style.bg);
        press(&mut w, &mut app, KeyCode::Tab);
        assert_eq!(w.focus, 1);
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
    fn low_capacity_is_visible_before_inventory_and_cannot_be_kept_away() {
        let (_home, mut app, mut w) = fixture();
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
        w.cursor = w
            .visible()
            .iter()
            .position(|f| f.id == "system:disk")
            .unwrap();
        press(&mut w, &mut app, KeyCode::Char('K'));
        assert!(w.visible().iter().any(|f| f.id == "system:disk"));
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
        w.insight = Some(ai::Insight {
            summary: "Cache can be downloaded again.".into(),
            evidence_ids: vec![f.id.clone()],
            action_ids: vec![],
            next_checks: vec![],
        });
        w.insight_inputs = vec![ai::Subject {
            id: f.id,
            title: f.title,
            observation: f.observation,
            consequence: f.consequence,
            action_ids: vec![],
            quick_win: f.quick_win,
            priority: 1,
            disruption: "routine".into(),
        }];
        app.entries[0].status = CacheStatus::InUse;
        w.rebuild(&app);
        assert!(w.insight.is_none());
        let entry = app.entries[0].clone();
        w.add_action(Action::Clean(entry.clone()));
        let mut child = entry;
        child.spec.path.push("child");
        w.add_action(Action::Clean(child));
        assert_eq!(w.plan.len(), 1);
    }
    #[test]
    fn live_response_is_rejected_when_evidence_changes_during_generation() {
        let (_home, mut app, mut w) = fixture();
        let f = w.selected().unwrap().clone();
        let (tx, rx) = mpsc::channel();
        w.insight_work = Some(InsightWork {
            receiver: rx,
            worker: None,
            cancel: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
            request: ai::Request {
                revision: 0,
                checks: Default::default(),
                investigation: false,
                subjects: vec![ai::Subject {
                    id: f.id.clone(),
                    title: f.title,
                    observation: f.observation,
                    consequence: f.consequence,
                    action_ids: vec![],
                    quick_win: f.quick_win,
                    priority: 1,
                    disruption: "routine".into(),
                }],
            },
        });
        app.entries[0].status = CacheStatus::Whitelisted;
        w.rebuild(&app);
        tx.send(Ok(ai::Insight {
            summary: "Old evidence".into(),
            evidence_ids: vec![f.id],
            action_ids: vec![],
            next_checks: vec![],
        }))
        .unwrap();
        w.tick(&mut app);
        assert!(w.insight.is_none());
        assert!(w.ai_error.is_some());
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
        w.insight=Some(ai::Insight{summary:"The package cache can be rebuilt. Clearing it trades disk space for a future download.".into(),evidence_ids:vec![w.findings[0].id.clone()],action_ids:vec![],next_checks:vec![]});
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
            assert!(first.contains("Findings"));
            assert!(first.contains("History"));
            if let Some(dir) = std::env::var_os("CARE_PREVIEW_DIR") {
                fs::create_dir_all(&dir).unwrap();
                fs::write(
                    PathBuf::from(dir).join(format!("care-{width}.svg")),
                    super::super::tests::buffer_svg(buffer),
                )
                .unwrap();
            }
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
    fn reduced_motion_does_not_animate_inference_border() {
        let (_home, app, mut w) = fixture();
        w.motion = false;
        let (_tx, rx) = mpsc::channel();
        w.insight_work = Some(InsightWork {
            receiver: rx,
            worker: None,
            cancel: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
            request: ai::Request {
                revision: 0,
                checks: Default::default(),
                subjects: vec![],
                investigation: false,
            },
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
}
