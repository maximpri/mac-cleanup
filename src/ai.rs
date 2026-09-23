// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded local inference. Structured suggestions never execute actions.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

pub const PROTOCOL_VERSION: u32 = 3;
/// Longest helper line accepted in either direction.
pub const MAX_LINE_BYTES: usize = 32_768;
const HELPER_NAME: &str = "diskray-ai";
const MISSING_HELPER: &str =
    "Apple AI helper missing. Install the release bundle or build with scripts/build-release.sh.";

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
    pub subjects: Vec<Subject>,
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
#[derive(Deserialize)]
struct Response {
    protocol: u32,
    request_id: Option<String>,
    available: bool,
    error: Option<String>,
    #[serde(default)]
    error_code: Option<String>,
    insight: Option<Insight>,
    triage: Option<Triage>,
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

#[cfg(test)]
thread_local! {
    static HELPER_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Point this test thread at a fake helper executable.
#[cfg(test)]
pub(crate) fn set_test_helper(path: Option<PathBuf>) {
    HELPER_OVERRIDE.with(|helper| *helper.borrow_mut() = path);
}

/// The helper ships beside the main executable. Only tests may substitute it.
pub fn helper_path() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(path) = HELPER_OVERRIDE.with(|helper| helper.borrow().clone()) {
        return Ok(path);
    }
    let executable = std::env::current_exe().map_err(|error| display_text(&error.to_string()))?;
    // Homebrew links `bin/diskray` into the Cellar; resolve the link so the
    // helper is found next to the real binary or in the formula's libexec.
    let executable = executable.canonicalize().unwrap_or(executable);
    let candidates = [
        executable.with_file_name(HELPER_NAME),
        executable
            .parent()
            .and_then(Path::parent)
            .map(|prefix| prefix.join("libexec").join(HELPER_NAME))
            .unwrap_or_default(),
    ];
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| MISSING_HELPER.into())
}

/// Plain-language text for helper error codes. Raw framework enum names are
/// kept out of the interface.
pub fn friendly_error(code: Option<&str>, message: &str) -> String {
    match code {
        Some("not_eligible") => "This Mac does not support Apple Intelligence.".into(),
        Some("not_enabled") => {
            "Apple Intelligence is turned off. Press A to open System Settings.".into()
        }
        Some("model_not_ready") => {
            "Apple Intelligence is still preparing its model. Try again once the download finishes."
                .into()
        }
        Some("context") => "The evidence was too large for the on-device model.".into(),
        Some("guardrail") => "Apple's on-device safety guardrails declined this request.".into(),
        Some("rate_limited") | Some("concurrent_requests") => {
            "The on-device model is busy. Try again shortly.".into()
        }
        Some("assets_unavailable") => {
            "Apple Intelligence model assets are unavailable right now.".into()
        }
        Some("unsupported_locale") => {
            "The on-device model does not support the current language or region.".into()
        }
        Some("refusal") => "The on-device model declined to answer.".into(),
        Some("decoding") => "The on-device model returned an unreadable answer.".into(),
        Some("tool_budget") => "The model kept requesting tools after its budget.".into(),
        _ => display_text(message),
    }
}

/// One helper process with non-blocking line I/O. The child is killed and
/// reaped when this value is dropped, so cancellation is `drop`.
pub struct HelperProcess {
    child: Child,
    lines: Receiver<Result<Value, String>>,
    writer: Option<Sender<String>>,
}

impl HelperProcess {
    pub fn spawn() -> Result<Self, String> {
        let helper = helper_path()?;
        let mut child = Command::new(helper)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| display_text(&error.to_string()))?;
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Apple AI helper pipes unavailable".into());
        };
        let (line_sender, lines) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut buffer = Vec::new();
                let read = (&mut reader)
                    .take(MAX_LINE_BYTES as u64 + 1)
                    .read_until(b'\n', &mut buffer);
                match read {
                    Ok(0) | Err(_) => break,
                    Ok(_) if buffer.len() > MAX_LINE_BYTES => {
                        let _ = line_sender.send(Err("Apple AI helper line too large".into()));
                        break;
                    }
                    Ok(_) => {
                        let text = String::from_utf8_lossy(&buffer);
                        if text.trim().is_empty() {
                            continue;
                        }
                        let parsed = serde_json::from_str::<Value>(text.trim())
                            .map_err(|_| "Invalid Apple AI response".to_string());
                        if line_sender.send(parsed).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let (writer, outgoing) = mpsc::channel::<String>();
        thread::spawn(move || {
            for line in outgoing {
                if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            lines,
            writer: Some(writer),
        })
    }

    pub fn send(&self, value: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(value).map_err(|error| error.to_string())?;
        if line.len() > MAX_LINE_BYTES {
            return Err("Apple AI request too large".into());
        }
        line.push('\n');
        self.writer
            .as_ref()
            .ok_or("Apple AI helper input closed")?
            .send(line)
            .map_err(|_| "Apple AI helper input closed".into())
    }

    /// Signal end of input. One-shot helpers answer and exit; long-lived
    /// sessions treat this as cancellation.
    pub fn close_input(&mut self) {
        self.writer = None;
    }

    /// Non-blocking read. `None` means no line is ready yet.
    pub fn try_recv(&self) -> Option<Result<Value, String>> {
        match self.lines.try_recv() {
            Ok(line) => Some(line),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("Apple AI helper stopped".into())),
        }
    }

    fn recv_timeout(&self, timeout: Duration) -> Option<Result<Value, String>> {
        match self.lines.recv_timeout(timeout) {
            Ok(line) => Some(line),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Err("Apple AI helper stopped".into())),
        }
    }
}

impl Drop for HelperProcess {
    fn drop(&mut self) {
        self.writer = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Send one request and wait for its single response line. `stopped` is the
/// message returned for cancellation or timeout.
fn request_once(
    payload: &Value,
    request_id: &str,
    timeout: Duration,
    cancelled: &AtomicBool,
    stopped: &str,
) -> Result<Response, String> {
    let mut helper = HelperProcess::spawn()?;
    helper.send(payload)?;
    helper.close_input();
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) || started.elapsed() > timeout {
            return Err(stopped.into());
        }
        let Some(line) = helper.recv_timeout(Duration::from_millis(25)) else {
            continue;
        };
        let response: Response =
            serde_json::from_value(line?).map_err(|_| "Invalid Apple AI response")?;
        if response.protocol != PROTOCOL_VERSION
            || response.request_id.as_deref() != Some(request_id)
        {
            return Err("Apple AI helper version or request mismatch.".into());
        }
        if !response.available {
            return Err(response.error.map_or_else(
                || "Enable Apple Intelligence and allow its model to download.".into(),
                |error| friendly_error(response.error_code.as_deref(), &error),
            ));
        }
        if let Some(error) = &response.error {
            return Err(friendly_error(response.error_code.as_deref(), error));
        }
        return Ok(response);
    }
}

/// Detect the framework used by the bundled local model helper without starting
/// an inference request. This runs on a worker because model availability may
/// involve checking downloaded Apple Intelligence assets.
pub fn framework_status() -> FrameworkStatus {
    if let Err(detail) = helper_path() {
        return FrameworkStatus::Missing {
            detail: if detail == MISSING_HELPER {
                "build or install the release bundle".into()
            } else {
                detail
            },
        };
    }
    let payload = serde_json::json!({
        "protocol": PROTOCOL_VERSION,
        "request_id": "availability",
        "operation": "capabilities",
    });
    let never = AtomicBool::new(false);
    match request_once(
        &payload,
        "availability",
        Duration::from_secs(5),
        &never,
        "availability check timed out",
    ) {
        Ok(response) => {
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
        }
        Err(detail) => FrameworkStatus::Unavailable {
            detail: display_text(&detail),
        },
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
    if !insight.next_checks.is_empty() {
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
pub fn triage(request: &Request, cancelled: &Arc<AtomicBool>) -> Result<Triage, String> {
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
    let response = request_once(
        &payload,
        &request_id,
        Duration::from_secs(20),
        cancelled,
        "Local triage stopped. Measured findings remain available.",
    )?;
    let triage = response.triage.ok_or("No triage returned")?;
    validate_triage(request, &triage)?;
    Ok(triage)
}

/// Summarize already-completed outcomes. The request contains no executable targets.
pub fn summarize(request: &Request, cancelled: &Arc<AtomicBool>) -> Result<Insight, String> {
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
    });
    let response = request_once(
        &payload,
        &request_id,
        Duration::from_secs(20),
        cancelled,
        "Local result summary stopped. Measurements remain available.",
    )?;
    let insight = response.insight.ok_or("No result summary returned")?;
    validate(request, &insight)?;
    Ok(Insight {
        summary: display_text(&insight.summary),
        evidence_ids: insight.evidence_ids,
        action_ids: vec![],
        next_checks: vec![],
    })
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Write an executable shell script that stands in for the Swift helper.
    pub(crate) fn fake_helper(dir: &std::path::Path, body: &str) -> PathBuf {
        let path = dir.join("fake-helper");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn one_subject_request() -> Request {
        Request {
            revision: 7,
            subjects: vec![Subject {
                id: "item1".into(),
                title: "cache".into(),
                observation: "measured".into(),
                consequence: "rebuild".into(),
                action_ids: vec![],
                quick_win: true,
                priority: 1,
                disruption: "routine".into(),
            }],
        }
    }

    #[test]
    fn helper_transport_returns_a_correlated_response() {
        let dir = tempfile::tempdir().unwrap();
        set_test_helper(Some(fake_helper(
            dir.path(),
            r#"read line
echo '{"protocol":3,"request_id":"triage:7","available":true,"triage":{"key_area_ids":[],"quick_win_ids":["item1"],"reasons":[]}}'"#,
        )));
        let triage = triage(&one_subject_request(), &Arc::new(AtomicBool::new(false)));
        set_test_helper(None);
        assert_eq!(triage.unwrap().quick_win_ids, vec!["item1"]);
    }

    #[test]
    fn helper_transport_rejects_mismatch_timeout_oversize_and_maps_errors() {
        let dir = tempfile::tempdir().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        set_test_helper(Some(fake_helper(
            dir.path(),
            r#"read line
echo '{"protocol":3,"request_id":"triage:8","available":true}'"#,
        )));
        assert!(
            triage(&one_subject_request(), &cancelled)
                .unwrap_err()
                .contains("mismatch")
        );
        set_test_helper(Some(fake_helper(
            dir.path(),
            r#"read line
head -c 40000 /dev/zero | tr '\0' a
echo"#,
        )));
        assert!(
            triage(&one_subject_request(), &cancelled)
                .unwrap_err()
                .contains("too large")
        );
        set_test_helper(Some(fake_helper(
            dir.path(),
            r#"read line
echo '{"protocol":3,"request_id":"triage:7","available":false,"error_code":"model_not_ready","error":"unavailable(FoundationModels.SystemLanguageModel.Availability.UnavailableReason.modelNotReady)"}'"#,
        )));
        let error = triage(&one_subject_request(), &cancelled).unwrap_err();
        assert!(error.contains("preparing its model"), "{error}");
        assert!(!error.contains("FoundationModels"));
        set_test_helper(Some(fake_helper(dir.path(), "read line\nexec sleep 30")));
        cancelled.store(true, Ordering::Relaxed);
        let started = Instant::now();
        assert!(
            triage(&one_subject_request(), &cancelled)
                .unwrap_err()
                .contains("stopped")
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cancellation must kill the helper promptly"
        );
        set_test_helper(None);
    }

    #[test]
    fn helper_process_streams_lines_both_ways_and_drop_kills_the_child() {
        let dir = tempfile::tempdir().unwrap();
        set_test_helper(Some(fake_helper(
            dir.path(),
            r#"while read line; do echo "{\"echo\":$line}"; done"#,
        )));
        let helper = HelperProcess::spawn().unwrap();
        set_test_helper(None);
        helper.send(&serde_json::json!({"n": 1})).unwrap();
        helper.send(&serde_json::json!({"n": 2})).unwrap();
        let mut seen = Vec::new();
        let started = Instant::now();
        while seen.len() < 2 && started.elapsed() < Duration::from_secs(5) {
            if let Some(line) = helper.recv_timeout(Duration::from_millis(50)) {
                seen.push(line.unwrap()["echo"]["n"].as_u64().unwrap());
            }
        }
        assert_eq!(seen, vec![1, 2]);
        assert!(helper.try_recv().is_none());
        drop(helper);
    }

    #[test]
    fn friendly_errors_hide_framework_internals() {
        for code in [
            "not_eligible",
            "not_enabled",
            "model_not_ready",
            "context",
            "guardrail",
            "rate_limited",
        ] {
            let text = friendly_error(Some(code), "FoundationModels.Internal");
            assert!(!text.contains("FoundationModels"), "{code}");
        }
        assert_eq!(friendly_error(None, "plain\u{1b}text"), "plaintext");
    }
    #[test]
    fn rejects_invented_actions_and_investigations() {
        let request = Request {
            revision: 1,
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
        insight.next_checks = vec!["fs_usage".into()];
        assert!(
            validate(&request, &insight).is_err(),
            "one-shot summaries never request checks"
        );
    }
    #[test]
    fn triage_cannot_change_policy_or_refer_to_unknown_subjects() {
        let request = Request {
            revision: 1,
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
