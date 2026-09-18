//! Conservative review and signalling of current-account processes.
//!
//! Explicit abnormal states are highlighted, while ordinary processes remain
//! visible for manual review of UI hangs that `ps` cannot prove. Signals are
//! restricted to the current account and every action revalidates the PID,
//! owner, parent, and start time first.

use std::{
    collections::{HashMap, HashSet},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;

const PS_COMMAND: &str = "/bin/ps";
const ID_COMMAND: &str = "/usr/bin/id";
const KILL_COMMAND: &str = "/bin/kill";
const SIGNAL_WAIT: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessHealth {
    /// No explicit abnormal state is reported. It remains visible for manual review.
    Running,
    /// The process exited but its parent has not collected its status.
    Zombie,
    /// The process is suspended by a job-control or debugging signal.
    Stopped,
    /// The process is blocked in an uninterruptible kernel wait.
    Uninterruptible,
}

impl ProcessHealth {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::Zombie => "DEAD/ZOMBIE",
            Self::Stopped => "STOPPED",
            Self::Uninterruptible => "STUCK WAIT",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Self::Running => {
                "No explicit abnormal state is reported. Review CPU use and the command if an app still appears hung."
            }
            Self::Zombie => {
                "Already dead; only its parent can reap it. Signals cannot remove a zombie."
            }
            Self::Stopped => {
                "macOS reports the process as suspended. It may be paused intentionally by a debugger or job control."
            }
            Self::Uninterruptible => {
                "macOS reports an uninterruptible kernel wait. Even SIGKILL may not finish until that wait returns."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessOutcome {
    Exited { signal: ProcessSignal },
    SignalSent { signal: ProcessSignal },
    SafetySkipped(String),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub uid: u32,
    pub state: String,
    pub elapsed: String,
    pub cpu_percent: String,
    pub command: String,
    pub health: ProcessHealth,
    pub signalable: bool,
    pub signal_block_reason: Option<String>,
    pub outcome: Option<ProcessOutcome>,
    pub(crate) start_time: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessSignal {
    Terminate,
    Kill,
}

impl ProcessSignal {
    pub fn label(self) -> &'static str {
        match self {
            Self::Terminate => "SIGTERM",
            Self::Kill => "SIGKILL",
        }
    }

    fn argument(self) -> &'static str {
        match self {
            Self::Terminate => "-TERM",
            Self::Kill => "-KILL",
        }
    }
}

#[derive(Debug, Clone)]
struct ProcessRow {
    pid: u32,
    parent_pid: u32,
    uid: u32,
    state: String,
    elapsed: String,
    cpu_percent: String,
    command: String,
    start_time: String,
}

/// Return current-account processes with explicit abnormal states sorted first.
pub fn review_processes() -> Result<Vec<ProcessEntry>, String> {
    let uid = current_uid()?;
    let rows = read_process_rows()?;
    let by_pid: HashMap<u32, u32> = rows.iter().map(|row| (row.pid, row.parent_pid)).collect();
    let protected = protected_processes(&by_pid);
    let current_pid = std::process::id();

    let mut entries = Vec::new();
    for row in rows {
        if row.uid != uid {
            continue;
        }

        let health = health_from_state(&row.state).unwrap_or(ProcessHealth::Running);
        let start_time = row.start_time.clone();
        let signal_block_reason = if health == ProcessHealth::Zombie {
            Some("zombies are already dead and must be reaped by their parent".into())
        } else if row.pid <= 1 || row.pid == current_pid || protected.contains(&row.pid) {
            Some("Mac Cleanup never signals itself or one of its ancestor processes".into())
        } else if start_time.is_empty() {
            Some("the process identity could not be captured safely".into())
        } else {
            None
        };
        entries.push(ProcessEntry {
            pid: row.pid,
            parent_pid: row.parent_pid,
            uid: row.uid,
            state: row.state,
            elapsed: row.elapsed,
            cpu_percent: row.cpu_percent,
            command: row.command,
            health,
            signalable: signal_block_reason.is_none(),
            signal_block_reason,
            outcome: None,
            start_time,
        });
    }
    entries.sort_by(|left, right| {
        health_order(left.health)
            .cmp(&health_order(right.health))
            .then_with(|| {
                if left.health == ProcessHealth::Running && right.health == ProcessHealth::Running {
                    let left_cpu = left.cpu_percent.parse::<f32>().unwrap_or_default();
                    let right_cpu = right.cpu_percent.parse::<f32>().unwrap_or_default();
                    right_cpu.total_cmp(&left_cpu)
                } else {
                    left.pid.cmp(&right.pid)
                }
            })
    });
    Ok(entries)
}

/// Return only the explicitly abnormal subset for reports and automation.
pub fn review_unhealthy_processes() -> Result<Vec<ProcessEntry>, String> {
    let mut entries = review_processes()?;
    entries.retain(|entry| entry.health != ProcessHealth::Running);
    Ok(entries)
}

/// Send one signal after revalidating the exact process identity and owner.
/// No escalation is automatic: SIGKILL requires its own explicit user action.
pub fn signal_process(entry: &mut ProcessEntry, signal: ProcessSignal) -> ProcessOutcome {
    let outcome = signal_process_inner(entry, signal);
    entry.outcome = Some(outcome.clone());
    outcome
}

fn signal_process_inner(entry: &ProcessEntry, signal: ProcessSignal) -> ProcessOutcome {
    if !entry.signalable {
        return ProcessOutcome::SafetySkipped(
            entry
                .signal_block_reason
                .clone()
                .unwrap_or_else(|| "this process is protected from signalling".into()),
        );
    }

    let uid = match current_uid() {
        Ok(uid) => uid,
        Err(error) => return ProcessOutcome::SafetySkipped(error),
    };
    if uid != entry.uid {
        return ProcessOutcome::SafetySkipped("the process owner changed".into());
    }

    let rows = match read_process_rows_for(entry.pid) {
        Ok(rows) => rows,
        Err(error) => return ProcessOutcome::SafetySkipped(error),
    };
    let Some(current) = rows.into_iter().find(|row| row.pid == entry.pid) else {
        return ProcessOutcome::SafetySkipped("the process already exited".into());
    };
    if current.uid != entry.uid || current.parent_pid != entry.parent_pid {
        return ProcessOutcome::SafetySkipped("the PID now belongs to a different process".into());
    }
    let current_health = health_from_state(&current.state).unwrap_or(ProcessHealth::Running);
    if entry.health != ProcessHealth::Running && current_health == ProcessHealth::Running {
        return ProcessOutcome::SafetySkipped(
            "the process is no longer in an unhealthy state; refresh before acting".into(),
        );
    }
    if current.start_time != entry.start_time {
        return ProcessOutcome::SafetySkipped("the PID was reused by a different process".into());
    }

    let status = Command::new(KILL_COMMAND)
        .arg(signal.argument())
        .arg(entry.pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            return ProcessOutcome::Failed(format!(
                "{} exited with status {status}",
                signal.label()
            ));
        }
        Err(error) => return ProcessOutcome::Failed(error.to_string()),
    }

    let deadline = Instant::now() + SIGNAL_WAIT;
    while Instant::now() < deadline {
        if !process_exists(entry.pid) {
            return ProcessOutcome::Exited { signal };
        }
        thread::sleep(Duration::from_millis(50));
    }
    ProcessOutcome::SignalSent { signal }
}

fn current_uid() -> Result<u32, String> {
    let output = Command::new(ID_COMMAND)
        .arg("-u")
        .output()
        .map_err(|error| format!("could not determine the current account: {error}"))?;
    if !output.status.success() {
        return Err("could not determine the current account".into());
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|_| "could not parse the current account id".into())
}

fn read_process_rows() -> Result<Vec<ProcessRow>, String> {
    read_process_rows_with_args(&["-axo", "pid=,ppid=,uid=,state=,etime=,%cpu=,lstart=,comm="])
}

fn read_process_rows_for(pid: u32) -> Result<Vec<ProcessRow>, String> {
    read_process_rows_with_args(&[
        "-p",
        &pid.to_string(),
        "-o",
        "pid=,ppid=,uid=,state=,etime=,%cpu=,lstart=,comm=",
    ])
}

fn read_process_rows_with_args(args: &[&str]) -> Result<Vec<ProcessRow>, String> {
    let output = Command::new(PS_COMMAND)
        .args(args)
        .output()
        .map_err(|error| format!("could not inspect processes: {error}"))?;
    if !output.status.success() {
        return Err("macOS process inspection failed".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_process_row)
        .collect())
}

fn parse_process_row(line: &str) -> Option<ProcessRow> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 12 {
        return None;
    }
    Some(ProcessRow {
        pid: fields[0].parse().ok()?,
        parent_pid: fields[1].parse().ok()?,
        uid: fields[2].parse().ok()?,
        state: fields[3].into(),
        elapsed: fields[4].into(),
        cpu_percent: fields[5].into(),
        start_time: fields[6..11].join(" "),
        command: fields[11..].join(" "),
    })
}

fn process_exists(pid: u32) -> bool {
    Command::new(PS_COMMAND)
        .args(["-p", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn protected_processes(parents: &HashMap<u32, u32>) -> HashSet<u32> {
    let mut protected = HashSet::new();
    let mut pid = std::process::id();
    while protected.insert(pid) {
        let Some(parent) = parents.get(&pid).copied() else {
            break;
        };
        if parent == 0 || parent == pid {
            break;
        }
        pid = parent;
    }
    protected
}

fn health_from_state(state: &str) -> Option<ProcessHealth> {
    match state.chars().next()? {
        'Z' => Some(ProcessHealth::Zombie),
        'T' => Some(ProcessHealth::Stopped),
        'D' | 'U' => Some(ProcessHealth::Uninterruptible),
        _ => None,
    }
}

fn health_order(health: ProcessHealth) -> u8 {
    match health {
        ProcessHealth::Uninterruptible => 0,
        ProcessHealth::Stopped => 1,
        ProcessHealth::Zombie => 2,
        ProcessHealth::Running => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ps_rows_with_commands_containing_spaces() {
        let row = parse_process_row(
            "  123   10  501 T+   01:02:03  7.5 Sun Aug 23 16:05:46 2026 /Applications/Test App.app/Test App",
        )
        .unwrap();
        assert_eq!(row.pid, 123);
        assert_eq!(row.parent_pid, 10);
        assert_eq!(row.command, "/Applications/Test App.app/Test App");
        assert_eq!(row.start_time, "Sun Aug 23 16:05:46 2026");
        assert_eq!(health_from_state(&row.state), Some(ProcessHealth::Stopped));
    }

    #[test]
    fn only_explicit_abnormal_states_are_flagged() {
        assert_eq!(health_from_state("Z+"), Some(ProcessHealth::Zombie));
        assert_eq!(health_from_state("T"), Some(ProcessHealth::Stopped));
        assert_eq!(health_from_state("U"), Some(ProcessHealth::Uninterruptible));
        assert_eq!(health_from_state("S+"), None);
        assert_eq!(health_from_state("R"), None);
    }

    #[test]
    fn reviews_and_revalidates_a_disposable_stopped_process() {
        let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let stopped = Command::new(KILL_COMMAND)
            .args(["-STOP", &pid.to_string()])
            .status()
            .unwrap();
        assert!(stopped.success());

        let test_result = (|| -> Result<(), String> {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut reviewed = loop {
                if let Some(entry) = review_unhealthy_processes()?
                    .into_iter()
                    .find(|entry| entry.pid == pid)
                {
                    break entry;
                }
                if Instant::now() >= deadline {
                    return Err("stopped test process was not reported".into());
                }
                thread::sleep(Duration::from_millis(25));
            };
            if reviewed.health != ProcessHealth::Stopped || !reviewed.signalable {
                return Err("stopped test process was not safely signalable".into());
            }
            let outcome = signal_process(&mut reviewed, ProcessSignal::Kill);
            if !matches!(
                outcome,
                ProcessOutcome::Exited { .. } | ProcessOutcome::SignalSent { .. }
            ) {
                return Err(format!("unexpected signal outcome: {outcome:?}"));
            }
            Ok(())
        })();

        let _ = Command::new(KILL_COMMAND)
            .args(["-KILL", &pid.to_string()])
            .status();
        let _ = child.wait();
        test_result.unwrap();
    }

    #[test]
    fn manually_reviewed_running_process_can_be_explicitly_killed() {
        let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let test_result = (|| -> Result<(), String> {
            let mut reviewed = review_processes()?
                .into_iter()
                .find(|entry| entry.pid == pid)
                .ok_or_else(|| "running test process was not listed".to_string())?;
            if reviewed.health != ProcessHealth::Running || !reviewed.signalable {
                return Err("running test process was not manually signalable".into());
            }
            let outcome = signal_process(&mut reviewed, ProcessSignal::Kill);
            if !matches!(
                outcome,
                ProcessOutcome::Exited { .. } | ProcessOutcome::SignalSent { .. }
            ) {
                return Err(format!("unexpected signal outcome: {outcome:?}"));
            }
            Ok(())
        })();

        let _ = Command::new(KILL_COMMAND)
            .args(["-KILL", &pid.to_string()])
            .status();
        let _ = child.wait();
        test_result.unwrap();
    }
}
