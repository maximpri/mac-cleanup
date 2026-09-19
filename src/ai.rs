//! Bounded local inference. Structured suggestions never execute actions.
use crate::investigation::{AgentDecision, AvailableCheck, CasePhase, InvestigationCase};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Bump when the helper instructions or response interpretation changes.
pub const PROMPT_VERSION: &str = "care-agent-v4";
pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameworkStatus {
    Detecting,
    Available { detail: String },
    Unavailable { detail: String },
    Missing { detail: String },
}

impl FrameworkStatus {
    pub fn description(&self) -> String {
        match self {
            Self::Detecting => "Apple Foundation Models · detecting".into(),
            Self::Available { detail } => format!("Apple Foundation Models · available{detail}"),
            Self::Unavailable { detail } => {
                format!("Apple Foundation Models · unavailable · {detail}")
            }
            Self::Missing { detail } => {
                format!("helper missing · Apple Foundation Models not checked · {detail}")
            }
        }
    }

    pub fn compact(&self) -> &'static str {
        match self {
            Self::Detecting => "AI detecting",
            Self::Available { .. } => "AI ready",
            Self::Unavailable { .. } => "AI unavailable",
            Self::Missing { .. } => "AI helper missing",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Subject {
    pub id: String,
    pub title: String,
    pub observation: String,
    pub consequence: String,
    pub action_ids: Vec<String>,
    /// Rust-computed policy facts. The model may rank these, but cannot change them.
    pub quick_win: bool,
    pub priority: u8,
    pub disruption: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct Request {
    pub revision: u64,
    pub checks: std::collections::HashMap<String, Vec<String>>,
    pub subjects: Vec<Subject>,
    pub investigation: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Insight {
    pub summary: String,
    pub evidence_ids: Vec<String>,
    pub action_ids: Vec<String>,
    pub next_checks: Vec<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Triage {
    pub key_area_ids: Vec<String>,
    pub quick_win_ids: Vec<String>,
    pub reasons: Vec<String>,
}
pub fn cache_key(request: &Request) -> String {
    format!(
        "{}:{}",
        PROMPT_VERSION,
        serde_json::to_string(request).unwrap_or_default()
    )
}
#[derive(Deserialize)]
struct Response {
    protocol: u32,
    request_id: Option<String>,
    available: bool,
    error: Option<String>,
    insight: Option<Insight>,
    triage: Option<Triage>,
    decision: Option<AgentDecision>,
    capabilities: Option<Capabilities>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Capabilities {
    helper_version: String,
    provider: String,
    context_size: Option<usize>,
    token_counting: bool,
    dynamic_schemas: bool,
}

/// Detect the framework used by the bundled local model helper without starting
/// an inference request. This runs on a worker because model availability may
/// involve checking downloaded Apple Intelligence assets.
pub fn framework_status() -> FrameworkStatus {
    let helper = match std::env::current_exe() {
        Ok(path) => path.with_file_name("mac-cleanup-ai"),
        Err(error) => {
            return FrameworkStatus::Missing {
                detail: display_text(&error.to_string()),
            };
        }
    };
    if !helper.is_file() {
        return FrameworkStatus::Missing {
            detail: "build or install the release bundle".into(),
        };
    }
    let mut child = match Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return FrameworkStatus::Unavailable {
                detail: display_text(&error.to_string()),
            };
        }
    };
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return FrameworkStatus::Unavailable {
            detail: "helper input unavailable".into(),
        };
    };
    if writeln!(stdin, "{{\"protocol\":{PROTOCOL_VERSION},\"request_id\":\"availability\",\"operation\":\"capabilities\"}}").is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return FrameworkStatus::Unavailable {
            detail: "helper request failed".into(),
        };
    }
    drop(stdin);
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return FrameworkStatus::Unavailable {
            detail: "helper output unavailable".into(),
        };
    };
    let reader = thread::spawn(move || {
        let mut output = String::new();
        stdout
            .take(8_193)
            .read_to_string(&mut output)
            .map(|_| output)
    });
    let started = Instant::now();
    loop {
        if started.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return FrameworkStatus::Unavailable {
                detail: "availability check timed out".into(),
            };
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = match reader.join() {
                    Ok(Ok(output)) => output,
                    _ => {
                        return FrameworkStatus::Unavailable {
                            detail: "helper response could not be read".into(),
                        };
                    }
                };
                if !status.success() {
                    return FrameworkStatus::Unavailable {
                        detail: "availability helper failed".into(),
                    };
                }
                let response: Response = match serde_json::from_str(output.trim()) {
                    Ok(response) => response,
                    Err(_) => {
                        return FrameworkStatus::Unavailable {
                            detail: "invalid availability response".into(),
                        };
                    }
                };
                if response.protocol != PROTOCOL_VERSION
                    || response.request_id.as_deref() != Some("availability")
                {
                    return FrameworkStatus::Unavailable {
                        detail: "helper protocol mismatch".into(),
                    };
                }
                return if response.available {
                    let detail = response
                        .capabilities
                        .map(|capabilities| {
                            format!(
                                " · {} · {} · context {} · {} · {}",
                                display_text(&capabilities.provider),
                                display_text(&capabilities.helper_version),
                                capabilities
                                    .context_size
                                    .map(|size| size.to_string())
                                    .unwrap_or_else(|| "unknown".into()),
                                if capabilities.dynamic_schemas {
                                    "constrained decisions"
                                } else {
                                    "basic schemas"
                                },
                                if capabilities.token_counting {
                                    "token preflight"
                                } else {
                                    "byte preflight"
                                }
                            )
                        })
                        .unwrap_or_default();
                    FrameworkStatus::Available { detail }
                } else {
                    FrameworkStatus::Unavailable {
                        detail: display_text(&response.error.unwrap_or_else(|| {
                            "enable Apple Intelligence in System Settings".into()
                        })),
                    }
                };
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return FrameworkStatus::Unavailable {
                    detail: display_text(&error.to_string()),
                };
            }
        }
    }
}

/// Inspected labels are data, including control and bidirectional characters.
pub fn display_text(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_control() && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(2000)
        .collect()
}
pub fn validate(request: &Request, insight: &Insight) -> Result<(), String> {
    let evidence: HashSet<_> = request.subjects.iter().map(|s| s.id.as_str()).collect();
    let actions: HashSet<_> = request
        .subjects
        .iter()
        .flat_map(|s| &s.action_ids)
        .map(String::as_str)
        .collect();
    if insight.summary.trim().is_empty()
        || insight.summary.len() > 4000
        || insight.evidence_ids.is_empty()
        || insight.evidence_ids.len() > 8
        || insight.action_ids.len() > 3
        || insight
            .evidence_ids
            .iter()
            .any(|id| !evidence.contains(id.as_str()))
        || insight
            .action_ids
            .iter()
            .any(|id| !actions.contains(id.as_str()))
    {
        return Err("AI returned unsupported evidence or actions.".into());
    }
    let checks = [
        "inspect_children",
        "refresh_processes",
        "check_open_handles",
        "compare_history",
        "fs_usage",
        "volume_context",
        "research_sources",
    ];
    if insight.next_checks.len() > 3
        || (!request.investigation && !insight.next_checks.is_empty())
        || insight
            .next_checks
            .iter()
            .any(|check| !checks.contains(&check.as_str()))
    {
        return Err("AI requested an unsupported investigation.".into());
    }
    Ok(())
}
pub fn validate_triage(request: &Request, triage: &Triage) -> Result<(), String> {
    let subjects: HashMap<_, _> = request
        .subjects
        .iter()
        .map(|subject| (subject.id.as_str(), subject))
        .collect();
    let mut chosen = HashSet::new();
    let duplicate = triage
        .key_area_ids
        .iter()
        .chain(triage.quick_win_ids.iter())
        .any(|id| !chosen.insert(id.as_str()));
    if (triage.key_area_ids.is_empty() && triage.quick_win_ids.is_empty())
        || triage.key_area_ids.len() > 5
        || triage.quick_win_ids.len() > 3
        || triage.reasons.len() > triage.key_area_ids.len() + triage.quick_win_ids.len()
        || duplicate
        || triage
            .key_area_ids
            .iter()
            .chain(triage.quick_win_ids.iter())
            .any(|id| !subjects.contains_key(id.as_str()))
        || triage
            .quick_win_ids
            .iter()
            .any(|id| !subjects[id.as_str()].quick_win)
        || triage
            .key_area_ids
            .iter()
            .any(|id| subjects[id.as_str()].quick_win)
    {
        return Err("AI returned unsupported triage references.".into());
    }
    Ok(())
}

/// Keep the measured ordering usable when the local model returns malformed
/// references or is unavailable. This path never invents a recommendation.
pub fn deterministic_triage(request: &Request) -> Triage {
    let mut quick = request
        .subjects
        .iter()
        .filter(|subject| subject.quick_win)
        .collect::<Vec<_>>();
    let mut areas = request
        .subjects
        .iter()
        .filter(|subject| !subject.quick_win)
        .collect::<Vec<_>>();
    let order = |left: &&Subject, right: &&Subject| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.id.cmp(&right.id))
    };
    quick.sort_by(order);
    areas.sort_by(order);
    let quick_win_ids = quick
        .into_iter()
        .take(3)
        .map(|subject| subject.id.clone())
        .collect::<Vec<_>>();
    let key_area_ids = areas
        .into_iter()
        .take(5)
        .map(|subject| subject.id.clone())
        .collect::<Vec<_>>();
    Triage {
        key_area_ids,
        quick_win_ids,
        reasons: vec![],
    }
}
pub fn explain(request: &Request, cancelled: &Arc<AtomicBool>) -> Result<Insight, String> {
    match explain_once(request, cancelled) {
        Err(error)
            if error.to_lowercase().contains("context")
                && request.subjects.len() > 1
                && !cancelled.load(Ordering::Relaxed) =>
        {
            let mut smaller = request.clone();
            smaller
                .subjects
                .truncate((request.subjects.len() / 2).max(1));
            explain_once(&smaller, cancelled)
        }
        result => result,
    }
}
pub fn triage(request: &Request, cancelled: &Arc<AtomicBool>) -> Result<Triage, String> {
    let helper = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("mac-cleanup-ai");
    if !helper.is_file() {
        return Err("Apple AI helper missing. Install the release bundle or build with scripts/build-release.sh.".into());
    }
    let prompt = serde_json::to_string(request).map_err(|e| e.to_string())?;
    if prompt.len() > 10_000 {
        return Err("Select fewer findings for local triage.".into());
    }
    let request_id = format!("triage:{}", request.revision);
    let key_areas = request
        .subjects
        .iter()
        .filter(|subject| !subject.quick_win)
        .map(|subject| subject.id.clone())
        .collect::<Vec<_>>();
    let quick_wins = request
        .subjects
        .iter()
        .filter(|subject| subject.quick_win)
        .map(|subject| subject.id.clone())
        .collect::<Vec<_>>();
    let payload = serde_json::json!({"protocol":PROTOCOL_VERSION,"request_id":request_id,"operation":"triage","prompt":prompt,"allowed_key_areas":key_areas,"allowed_quick_wins":quick_wins});
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("Missing helper input")?;
    if let Err(e) = writeln!(stdin, "{payload}") {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e.to_string());
    }
    drop(stdin);
    let stdout = child.stdout.take().ok_or("Missing helper output")?;
    let reader = thread::spawn(move || {
        let mut output = String::new();
        stdout
            .take(32_769)
            .read_to_string(&mut output)
            .map(|_| output)
    });
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err("Local triage stopped. Measured findings remain available.".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = reader
                    .join()
                    .map_err(|_| "AI reader stopped")?
                    .map_err(|e| e.to_string())?;
                if !status.success() || output.len() > 32_768 {
                    return Err("Apple AI helper failed.".into());
                }
                let response: Response =
                    serde_json::from_str(output.trim()).map_err(|_| "Invalid Apple AI response")?;
                if response.protocol != PROTOCOL_VERSION
                    || response.request_id.as_deref() != Some(request_id.as_str())
                {
                    return Err("Apple AI helper version mismatch.".into());
                }
                if !response.available {
                    return Err(response.error.unwrap_or_else(|| {
                        "Enable Apple Intelligence and allow its model to download.".into()
                    }));
                }
                if let Some(error) = response.error {
                    return Err(display_text(&error));
                }
                let triage = response.triage.ok_or("No triage returned")?;
                validate_triage(request, &triage)?;
                return Ok(triage);
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(error.to_string());
            }
        }
    }
}

/// Ask the on-device model for exactly one next diagnostic step. Rust supplies
/// the complete allowed set and validates the returned references before the
/// caller may execute anything.
pub fn decide(
    case: &InvestigationCase,
    available: &[AvailableCheck],
    cancelled: &Arc<AtomicBool>,
) -> Result<AgentDecision, String> {
    if available.is_empty() {
        return Ok(AgentDecision::Finish {
            conclusion: case.conclusion_text(),
            phase: CasePhase::Inconclusive,
            evidence_ids: case
                .evidence
                .iter()
                .filter(|evidence| evidence.status.can_support_hypothesis())
                .map(|evidence| evidence.id.clone())
                .take(4)
                .collect(),
        });
    }
    let helper = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name("mac-cleanup-ai");
    if !helper.is_file() {
        return Err("Apple AI helper missing. Install the release bundle or build with scripts/build-release.sh.".into());
    }
    let packet = serde_json::json!({
        "case_id": case.id,
        "revision": case.revision,
        "target": case.target,
        "phase": case.phase,
        "decision_count": case.decision_count,
        "decision_budget": case.decision_budget,
        "hypotheses": case.hypotheses,
        "evidence": case.evidence,
        "available_checks": available,
    });
    let prompt = serde_json::to_string(&packet).map_err(|error| error.to_string())?;
    if prompt.len() > 14_000 {
        return Err("Investigation evidence exceeds the local decision context.".into());
    }
    let request_id = format!("decision:{}:{}", case.id, case.decision_count);
    let payload = serde_json::json!({
        "protocol": PROTOCOL_VERSION,
        "request_id": request_id,
        "operation": "decide",
        "prompt": prompt,
        "allowed_checks": available.iter().map(|check| check.id.clone()).collect::<Vec<_>>(),
        "allowed_evidence": case.evidence.iter().map(|evidence| evidence.id.clone()).collect::<Vec<_>>(),
        "allowed_hypotheses": case.hypotheses.iter().map(|hypothesis| hypothesis.id.clone()).collect::<Vec<_>>(),
    });
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let mut stdin = child.stdin.take().ok_or("Missing helper input")?;
    if let Err(error) = writeln!(stdin, "{payload}") {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.to_string());
    }
    drop(stdin);
    let stdout = child.stdout.take().ok_or("Missing helper output")?;
    let reader = thread::spawn(move || {
        let mut output = String::new();
        stdout
            .take(32_769)
            .read_to_string(&mut output)
            .map(|_| output)
    });
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err("Local investigation decision stopped.".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = reader
                    .join()
                    .map_err(|_| "AI reader stopped")?
                    .map_err(|error| error.to_string())?;
                if !status.success() || output.len() > 32_768 {
                    return Err("Apple AI helper failed.".into());
                }
                let response: Response =
                    serde_json::from_str(output.trim()).map_err(|_| "Invalid Apple AI response")?;
                if response.protocol != PROTOCOL_VERSION
                    || response.request_id.as_deref() != Some(request_id.as_str())
                {
                    return Err("Apple AI helper version or request mismatch.".into());
                }
                if !response.available {
                    return Err(response.error.unwrap_or_else(|| {
                        "Enable Apple Intelligence and allow its model to download.".into()
                    }));
                }
                if let Some(error) = response.error {
                    return Err(display_text(&error));
                }
                let mut decision = response
                    .decision
                    .ok_or("No investigation decision returned")?;
                match &mut decision {
                    AgentDecision::Check { reason, .. } => *reason = display_text(reason),
                    AgentDecision::Finish { conclusion, .. } => {
                        *conclusion = display_text(conclusion)
                    }
                    AgentDecision::Research { .. } | AgentDecision::AwaitApproval { .. } => {}
                }
                case.validate_decision(&decision, available)?;
                return Ok(decision);
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(error.to_string());
            }
        }
    }
}
/// Summarize already-completed outcomes. The request contains no executable targets.
pub fn summarize(request: &Request, cancelled: &Arc<AtomicBool>) -> Result<Insight, String> {
    let helper = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("mac-cleanup-ai");
    if !helper.is_file() {
        return Err("Apple AI helper missing. Install the release bundle or build with scripts/build-release.sh.".into());
    }
    let prompt = serde_json::to_string(request).map_err(|e| e.to_string())?;
    if prompt.len() > 10_000 {
        return Err("Action results are too large for a local summary.".into());
    }
    let request_id = format!("result:{}", request.revision);
    let payload = serde_json::json!({
        "protocol":PROTOCOL_VERSION,
        "request_id":request_id,
        "operation":"result",
        "prompt":prompt,
        "allowed_evidence":request.subjects.iter().map(|subject| subject.id.clone()).collect::<Vec<_>>(),
        "allowed_actions":[],
        "allowed_checks":[],
    });
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("Missing helper input")?;
    if let Err(e) = writeln!(stdin, "{payload}") {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e.to_string());
    }
    drop(stdin);
    let stdout = child.stdout.take().ok_or("Missing helper output")?;
    let reader = thread::spawn(move || {
        let mut output = String::new();
        stdout
            .take(32_769)
            .read_to_string(&mut output)
            .map(|_| output)
    });
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err("Local result summary stopped. Measurements remain available.".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = reader
                    .join()
                    .map_err(|_| "AI reader stopped")?
                    .map_err(|e| e.to_string())?;
                if !status.success() || output.len() > 32_768 {
                    return Err("Apple AI helper failed.".into());
                }
                let response: Response =
                    serde_json::from_str(output.trim()).map_err(|_| "Invalid Apple AI response")?;
                if response.protocol != PROTOCOL_VERSION
                    || response.request_id.as_deref() != Some(request_id.as_str())
                {
                    return Err("Apple AI helper version mismatch.".into());
                }
                if !response.available {
                    return Err(response.error.unwrap_or_else(|| {
                        "Enable Apple Intelligence and allow its model to download.".into()
                    }));
                }
                if let Some(error) = response.error {
                    return Err(display_text(&error));
                }
                let insight = response.insight.ok_or("No result summary returned")?;
                validate(request, &insight)?;
                return Ok(Insight {
                    summary: display_text(&insight.summary),
                    evidence_ids: insight.evidence_ids,
                    action_ids: vec![],
                    next_checks: vec![],
                });
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(error.to_string());
            }
        }
    }
}
fn explain_once(request: &Request, cancelled: &Arc<AtomicBool>) -> Result<Insight, String> {
    let helper = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("mac-cleanup-ai");
    if !helper.is_file() {
        return Err("Apple AI helper missing. Install the release bundle or build with scripts/build-release.sh.".into());
    }
    let prompt = serde_json::to_string(request).map_err(|e| e.to_string())?;
    if prompt.len() > 10_000 {
        return Err("Select fewer findings for a local insight.".into());
    }
    let request_id = format!("explain:{}", request.revision);
    let payload = serde_json::json!({
        "protocol":PROTOCOL_VERSION,
        "request_id":request_id,
        "operation":"explain",
        "prompt":prompt,
        "allowed_evidence":request.subjects.iter().map(|subject| subject.id.clone()).collect::<Vec<_>>(),
        "allowed_actions":request.subjects.iter().flat_map(|subject| subject.action_ids.clone()).collect::<Vec<_>>(),
        "allowed_checks":if request.investigation { vec!["inspect_children","refresh_processes","check_open_handles","compare_history","fs_usage","volume_context","research_sources"] } else { vec![] },
    });
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("Missing helper input")?;
    if let Err(e) = writeln!(stdin, "{payload}") {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e.to_string());
    }
    drop(stdin);
    let stdout = child.stdout.take().ok_or("Missing helper output")?;
    let reader = thread::spawn(move || {
        let mut output = String::new();
        stdout
            .take(32_769)
            .read_to_string(&mut output)
            .map(|_| output)
    });
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err("Local insight stopped. Measured findings remain available.".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = reader
                    .join()
                    .map_err(|_| "AI reader stopped")?
                    .map_err(|e| e.to_string())?;
                if !status.success() || output.len() > 32_768 {
                    return Err("Apple AI helper failed.".into());
                }
                let response: Response =
                    serde_json::from_str(output.trim()).map_err(|_| "Invalid Apple AI response")?;
                if response.protocol != PROTOCOL_VERSION
                    || response.request_id.as_deref() != Some(request_id.as_str())
                {
                    return Err("Apple AI helper version mismatch.".into());
                }
                if !response.available {
                    return Err(response.error.unwrap_or_else(|| {
                        "Enable Apple Intelligence and allow its model to download.".into()
                    }));
                }
                if let Some(error) = response.error {
                    return Err(display_text(&error));
                }
                let mut insight = response.insight.ok_or("No insight returned")?;
                validate(request, &insight)?;
                insight.summary = display_text(&insight.summary);
                return Ok(insight);
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(error.to_string());
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invented_actions_and_investigations() {
        let request = Request {
            revision: 1,
            checks: Default::default(),
            investigation: false,
            subjects: vec![Subject {
                id: "a".into(),
                title: "cache".into(),
                observation: "measured".into(),
                consequence: "rebuild".into(),
                action_ids: vec!["clean:a".into()],
                quick_win: true,
                priority: 1,
                disruption: "routine".into(),
            }],
        };
        let mut insight = Insight {
            summary: "A cache was measured.".into(),
            evidence_ids: vec!["a".into()],
            action_ids: vec!["clean:a".into()],
            next_checks: vec![],
        };
        assert!(validate(&request, &insight).is_ok());
        insight.action_ids.push("delete:/".into());
        assert!(validate(&request, &insight).is_err());
        insight.action_ids.clear();
        insight.evidence_ids = vec!["unknown".into()];
        assert!(validate(&request, &insight).is_err());
        insight.evidence_ids = vec!["a".into()];
        insight.next_checks = vec!["shell".into()];
        assert!(validate(&request, &insight).is_err());
        let investigation_request = Request {
            investigation: true,
            ..request
        };
        insight.next_checks = vec!["fs_usage".into()];
        assert!(validate(&investigation_request, &insight).is_ok());
        insight.next_checks = vec!["volume_context".into(), "research_sources".into()];
        assert!(validate(&investigation_request, &insight).is_ok());
    }
    #[test]
    fn triage_cannot_change_policy_or_refer_to_unknown_subjects() {
        let request = Request {
            revision: 1,
            checks: Default::default(),
            investigation: false,
            subjects: vec![
                Subject {
                    id: "quick".into(),
                    title: "cache".into(),
                    observation: "measured".into(),
                    consequence: "rebuild".into(),
                    action_ids: vec![],
                    quick_win: true,
                    priority: 1,
                    disruption: "routine".into(),
                },
                Subject {
                    id: "area".into(),
                    title: "folder".into(),
                    observation: "measured".into(),
                    consequence: "inspect".into(),
                    action_ids: vec![],
                    quick_win: false,
                    priority: 2,
                    disruption: "review".into(),
                },
            ],
        };
        assert!(
            validate_triage(
                &request,
                &Triage {
                    key_area_ids: vec!["area".into()],
                    quick_win_ids: vec!["quick".into()],
                    reasons: vec!["low disruption".into()],
                }
            )
            .is_ok()
        );
        assert!(
            validate_triage(
                &request,
                &Triage {
                    key_area_ids: vec!["quick".into()],
                    quick_win_ids: vec!["missing".into()],
                    reasons: vec![],
                }
            )
            .is_err()
        );
    }
    #[test]
    fn strips_terminal_controls() {
        assert_eq!(display_text("a\n\x1b\u{202e}b"), "ab");
    }
}
