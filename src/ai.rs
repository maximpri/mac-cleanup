//! Bounded local inference. Structured suggestions never execute actions.
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
pub const PROMPT_VERSION: &str = "care-triage-v2";

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
    available: bool,
    error: Option<String>,
    insight: Option<Insight>,
    triage: Option<Triage>,
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
    if triage.key_area_ids.len() > 5
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
    let payload = serde_json::json!({"protocol":1,"operation":"triage","prompt":prompt});
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
                if response.protocol != 1 {
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
    let payload = serde_json::json!({"protocol":1,"operation":"result","prompt":prompt});
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
                if response.protocol != 1 {
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
    let payload = serde_json::json!({"protocol":1,"operation":"explain","prompt":prompt});
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
                if response.protocol != 1 {
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
