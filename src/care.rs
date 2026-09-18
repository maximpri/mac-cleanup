//! Shared assessment, policy, and outcome records for the visual care workflow.
use crate::{
    ai,
    cache::{self, CacheEntry, CacheStatus, CacheTier},
    processes::{self, ProcessEntry},
    storage::{self, StorageInventory, VolumeStats},
    whitelist::Whitelist,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const HISTORY_SUBPATH: &str = "Library/Application Support/mac-cleanup/sessions";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Metrics {
    pub pressure: Option<u32>,
    pub swap: String,
    pub cpu: HashMap<u32, f64>,
    pub rss_kb: HashMap<u32, u64>,
    pub sampled_at: u64,
    pub interval_secs: f64,
    pub error: Option<String>,
    pub swap_in_kb_s: Option<f64>,
    pub swap_out_kb_s: Option<f64>,
    pub disk_mb_s: HashMap<String, f64>,
}
impl Metrics {
    pub fn describe(&self) -> String {
        let rate = |value: Option<f64>| {
            value
                .map(|v| format!("{v:.1} KiB/s"))
                .unwrap_or_else(|| "unavailable".into())
        };
        let mut disks: Vec<_> = self.disk_mb_s.iter().collect();
        disks.sort_by(|a, b| a.0.cmp(b.0));
        format!(
            "CPU {} across sampled processes · {:.1}s window\nMemory pressure {} · swap in {} / out {}\nDevice activity: {}\nDevice measurements can overlap; RSS is not reclaimable memory.",
            if self.error.is_some() || self.cpu.is_empty() {
                "unavailable".into()
            } else {
                format!("{:.1}%", self.cpu.values().sum::<f64>())
            },
            self.interval_secs,
            self.pressure_label(),
            rate(self.swap_in_kb_s),
            rate(self.swap_out_kb_s),
            if disks.is_empty() {
                "unavailable".into()
            } else {
                disks
                    .iter()
                    .take(3)
                    .map(|(name, rate)| format!("{name} {rate:.1} MB/s"))
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
        )
    }
    pub fn pressure_label(&self) -> &'static str {
        match self.pressure {
            Some(1) => "normal",
            Some(2) => "warning",
            Some(4) => "critical",
            _ => "unavailable",
        }
    }
}
pub enum Event {
    Capacity(Result<VolumeStats, String>),
    Processes(Result<Vec<ProcessEntry>, String>, Metrics),
    Entry(CacheEntry),
    Inventory(StorageInventory),
    Stage(String),
    Finished,
}
pub struct Assessment {
    pub receiver: Receiver<Event>,
    pub stop: Arc<AtomicBool>,
}
impl Drop for Assessment {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Query with a hard deadline; no pressure-generation or privileged commands.
pub fn query(program: &str, args: &[&str], timeout: Duration) -> io::Result<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing output"))?;
    let reader = thread::spawn(move || {
        use std::io::Read;
        let mut data = String::new();
        stdout
            .take(2_000_001)
            .read_to_string(&mut data)
            .map(|_| data)
    });
    let began = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let data = reader
                    .join()
                    .map_err(|_| io::Error::other("reader stopped"))??;
                return if status.success() && data.len() <= 2_000_000 {
                    Ok(data)
                } else {
                    Err(io::Error::other("query failed or exceeded output limit"))
                };
            }
            Ok(None) if began.elapsed() < timeout => thread::sleep(Duration::from_millis(25)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(result.err().unwrap_or_else(|| {
                    io::Error::new(io::ErrorKind::TimedOut, "query timed out")
                }));
            }
        }
    }
}

/// Capture a short, read-only filesystem-activity sample for fseventsd.
/// `sudo -n` is deliberate: an interactive password prompt must never appear
/// inside the TUI. A missing cached authorization is reported as unavailable.
pub fn observe_fseventsd(cancel_requested: &AtomicBool) -> Result<String, String> {
    let mut child = Command::new("/usr/bin/sudo")
        .args([
            "-n",
            "/usr/bin/fs_usage",
            "-w",
            "-f",
            "filesys",
            "fseventsd",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("filesystem activity probe unavailable: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "filesystem activity probe returned no output".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "filesystem activity probe returned no diagnostics".to_string())?;
    let output_reader = thread::spawn(move || {
        let mut output = String::new();
        stdout
            .take(64_001)
            .read_to_string(&mut output)
            .map(|_| output)
    });
    let error_reader = thread::spawn(move || {
        let mut output = String::new();
        stderr
            .take(8_193)
            .read_to_string(&mut output)
            .map(|_| output)
    });
    let started = Instant::now();
    let timed_out = loop {
        if cancel_requested.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = output_reader.join();
            let _ = error_reader.join();
            return Err("filesystem activity probe cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) if started.elapsed() < Duration::from_secs(8) => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break true;
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = output_reader.join();
                let _ = error_reader.join();
                return Err(format!("filesystem activity probe failed: {error}"));
            }
        }
    };
    let output = output_reader
        .join()
        .map_err(|_| "filesystem activity reader stopped".to_string())
        .and_then(|result| result.map_err(|error| error.to_string()))?;
    let error = error_reader
        .join()
        .map_err(|_| "filesystem activity diagnostics stopped".to_string())
        .and_then(|result| result.map_err(|error| error.to_string()))?;
    let lines = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(12)
        .map(ai::display_text)
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        let swap_events = lines
            .iter()
            .filter(|line| line.to_ascii_lowercase().contains("swapfile"))
            .count();
        let pattern = if swap_events > 0 {
            format!(
                "Pattern: {swap_events}/{} sampled lines mention VM swapfiles. This indicates fseventsd is observing swap-backed filesystem activity; it does not prove fseventsd caused memory pressure.",
                lines.len()
            )
        } else {
            "Pattern: no VM swapfile path appeared in the sampled lines.".into()
        };
        let timing = if timed_out {
            "The 8-second sample reached its safety limit."
        } else {
            "The bounded sample completed."
        };
        return Ok(format!(
            "{pattern}\n{timing}\nSampled filesystem events from fseventsd:\n{}",
            lines.join("\n")
        ));
    }
    let diagnostic = ai::display_text(error.trim());
    Err(if diagnostic.is_empty() {
        "No filesystem events were captured. Existing sudo authorization may be required.".into()
    } else {
        format!("No filesystem events captured: {diagnostic}")
    })
}
fn cpu_seconds(value: &str) -> Option<f64> {
    let mut total = 0.;
    for part in value.split(':') {
        total = total * 60. + part.parse::<f64>().ok()?;
    }
    Some(total)
}
fn vm_counters(text: &str) -> Option<(u64, u64, u64)> {
    let page_size = text
        .lines()
        .next()?
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let counter = |name: &str| {
        text.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == name)
                .then(|| value.trim().trim_end_matches('.').parse::<u64>().ok())
                .flatten()
        })
    };
    Some((page_size, counter("Swapins")?, counter("Swapouts")?))
}
fn disk_counters(text: &str) -> Option<HashMap<String, f64>> {
    let mut lines = text.lines();
    let devices: Vec<_> = lines.next()?.split_whitespace().collect();
    let _headers = lines.next()?;
    let fields: Vec<_> = lines.next()?.split_whitespace().collect();
    if fields.len() != devices.len() * 3 || devices.is_empty() {
        return None;
    }
    devices
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mb = fields[i * 3 + 2].parse::<f64>().ok()?;
            (mb.is_finite() && mb >= 0.).then_some((name.to_string(), mb))
        })
        .collect()
}
#[derive(Default)]
pub struct Sampler {
    previous: HashMap<u32, (String, f64)>,
    at: Option<Instant>,
    previous_vm: Option<(u64, u64, u64)>,
    previous_disk: HashMap<String, f64>,
}
impl Sampler {
    pub fn sample(&mut self, processes: &[ProcessEntry]) -> Metrics {
        let now = Instant::now();
        let elapsed = self
            .at
            .map(|at| now.duration_since(at).as_secs_f64())
            .unwrap_or_default();
        let mut result = Metrics {
            sampled_at: timestamp(),
            interval_secs: elapsed,
            ..Default::default()
        };
        result.pressure = query(
            "/usr/sbin/sysctl",
            &["-n", "kern.memorystatus_vm_pressure_level"],
            Duration::from_secs(2),
        )
        .ok()
        .and_then(|s| s.trim().parse().ok());
        result.swap = query(
            "/usr/sbin/sysctl",
            &["-n", "vm.swapusage"],
            Duration::from_secs(2),
        )
        .map(|s| ai::display_text(s.trim()))
        .unwrap_or_else(|_| "unavailable".into());
        let vm = query("/usr/bin/vm_stat", &[], Duration::from_secs(2))
            .ok()
            .and_then(|text| vm_counters(&text));
        if let (Some((page, ins, outs)), Some((old_page, old_ins, old_outs))) =
            (vm, self.previous_vm)
            && page == old_page
            && elapsed > 0.
        {
            result.swap_in_kb_s = ins
                .checked_sub(old_ins)
                .map(|delta| delta as f64 * page as f64 / 1024. / elapsed);
            result.swap_out_kb_s = outs
                .checked_sub(old_outs)
                .map(|delta| delta as f64 * page as f64 / 1024. / elapsed);
        }
        self.previous_vm = vm;
        let disks = query(
            "/usr/sbin/iostat",
            &["-Id", "-c", "1"],
            Duration::from_secs(2),
        )
        .ok()
        .and_then(|text| disk_counters(&text))
        .unwrap_or_default();
        if elapsed > 0. {
            for (name, mb) in &disks {
                if let Some(previous) = self.previous_disk.get(name)
                    && mb >= previous
                {
                    result
                        .disk_mb_s
                        .insert(name.clone(), (mb - previous) / elapsed);
                }
            }
        }
        self.previous_disk = disks;
        match query(
            "/bin/ps",
            &["-axo", "pid=,time=,rss="],
            Duration::from_secs(2),
        ) {
            Ok(text) => {
                let mut next = HashMap::new();
                for line in text.lines() {
                    let fields: Vec<_> = line.split_whitespace().collect();
                    if fields.len() != 3 {
                        continue;
                    }
                    let (Ok(pid), Some(cpu), Ok(rss)) = (
                        fields[0].parse::<u32>(),
                        cpu_seconds(fields[1]),
                        fields[2].parse::<u64>(),
                    ) else {
                        continue;
                    };
                    let Some(process) = processes.iter().find(|p| p.pid == pid) else {
                        continue;
                    };
                    if let Some((identity, previous)) = self.previous.get(&pid)
                        && identity == &process.start_time
                        && elapsed > 0.
                        && cpu >= *previous
                    {
                        result.cpu.insert(pid, (cpu - previous) * 100. / elapsed);
                    }
                    result.rss_kb.insert(pid, rss);
                    next.insert(pid, (process.start_time.clone(), cpu));
                }
                self.previous = next;
            }
            Err(error) => result.error = Some(error.to_string()),
        }
        self.at = Some(now);
        result
    }
}
fn wait(stop: &AtomicBool, duration: Duration) -> bool {
    let began = Instant::now();
    while began.elapsed() < duration {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
    !stop.load(Ordering::Relaxed)
}
pub fn assess(
    root: PathBuf,
    home: PathBuf,
    include_optional: bool,
    retention_days: u64,
) -> Assessment {
    let (sender, receiver) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let telemetry_stop = stop.clone();
    let telemetry = sender.clone();
    thread::spawn(move || {
        let mut sampler = Sampler::default();
        while !telemetry_stop.load(Ordering::Relaxed) {
            let processes = processes::review_processes();
            let metrics = sampler.sample(processes.as_deref().unwrap_or_default());
            if telemetry
                .send(Event::Processes(processes, metrics))
                .is_err()
            {
                break;
            }
            if !wait(&telemetry_stop, Duration::from_secs(2)) {
                break;
            }
        }
    });
    let scan_stop = stop.clone();
    thread::spawn(move || {
        let capacity = storage::read_volume_stats(&root).map_err(|e| e.to_string());
        if sender.send(Event::Capacity(capacity)).is_err() {
            return;
        }
        let _ = sender.send(Event::Stage(
            "Measuring resource baseline · 10 seconds".into(),
        ));
        if !wait(&scan_stop, Duration::from_secs(10)) {
            return;
        }
        let whitelist = Whitelist::load(&home);
        for spec in cache::scan_specs(&root, &home) {
            if scan_stop.load(Ordering::Relaxed) {
                return;
            }
            let _ = sender.send(Event::Stage(format!("Measuring {}", spec.label)));
            let mut entry = cache::scan_cache(&spec, include_optional);
            if whitelist.protects(&entry.spec.path) {
                entry.status = CacheStatus::Whitelisted;
            }
            if sender.send(Event::Entry(entry)).is_err() {
                return;
            }
        }
        let _ = sender.send(Event::Stage(
            "Checking temporary retention and downloads".into(),
        ));
        let retention =
            crate::retention::TempRetentionScan::discover_for_scan(&root, &home, retention_days);
        let mut extra_specs = retention.cache_specs();
        if root == Path::new("/") || root == home {
            extra_specs.extend(crate::downloads::discover_specs(&home, retention_days));
        }
        for spec in extra_specs {
            if scan_stop.load(Ordering::Relaxed) {
                return;
            }
            let mut entry = cache::scan_cache(&spec, include_optional);
            if whitelist.protects(&entry.spec.path) {
                entry.status = CacheStatus::Whitelisted;
            }
            if sender.send(Event::Entry(entry)).is_err() {
                return;
            }
        }
        let _ = sender.send(Event::Stage(
            "Exploring volume · folder totals still measuring".into(),
        ));
        let inventory = StorageInventory::scan_with_cancel(&root, &home, &scan_stop);
        if !scan_stop.load(Ordering::Relaxed) {
            let _ = sender.send(Event::Inventory(inventory));
            let _ = sender.send(Event::Finished);
        }
    });
    Assessment { receiver, stop }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Cache(PathBuf),
    Process(u32, String),
    Folder(PathBuf),
    System,
}
#[derive(Debug, Clone)]
pub struct Finding {
    pub id: String,
    pub title: String,
    pub observation: String,
    pub consequence: String,
    pub size_kb: u64,
    pub quick_win: bool,
    pub target: Target,
    pub related_pids: Vec<u32>,
}
pub fn quick_win(entry: &CacheEntry) -> bool {
    entry.status == CacheStatus::Ready
        && entry.spec.tier == CacheTier::Routine
        && entry.identity.is_some()
        && entry.size_kb >= 100 * 1024
        && matches!(
            entry.spec.label,
            "pip cache"
                | "Python cache"
                | "Homebrew cache"
                | "npm package cache"
                | "uv package cache"
                | "Yarn package cache"
                | "OpenCode cache"
                | "node-gyp cache"
        )
}
fn association(label: &str, command: &str) -> bool {
    let command = command.to_lowercase();
    match label {
        "Xcode derived data" | "Xcode archives" | "Xcode device support" => {
            command.contains("/xcode.app/") || command.ends_with("/xcodebuild")
        }
        "OpenCode cache" => command.ends_with("/opencode"),
        "Codex runtimes" => command.contains("/codex.app/"),
        "Playwright browsers" | "Playwright Go browsers" => command.contains("/ms-playwright/"),
        _ => false,
    }
}
pub fn findings(
    entries: &[CacheEntry],
    processes: &[ProcessEntry],
    metrics: &Metrics,
    inventory: Option<&StorageInventory>,
) -> Vec<Finding> {
    let mut results = Vec::new();
    if metrics.pressure.is_some_and(|p| p == 2 || p == 4) {
        results.push(Finding {
            id: "system:memory".into(),
            title: "Memory pressure needs attention".into(),
            observation: format!(
                "System pressure is {}. Inspect active workloads before deciding to stop anything.",
                metrics.pressure_label()
            ),
            consequence: "Used memory alone is not waste. No automatic process termination.".into(),
            size_kb: 0,
            quick_win: false,
            target: Target::System,
            related_pids: vec![],
        });
    }
    for entry in entries {
        if entry.status == CacheStatus::Missing
            || (entry.size_kb == 0 && entry.status != CacheStatus::ScanError)
        {
            continue;
        }
        let pids = processes
            .iter()
            .filter(|p| association(entry.spec.label, &p.command))
            .map(|p| p.pid)
            .collect::<Vec<_>>();
        results.push(Finding {
            id: format!("path:{}", entry.spec.path.display()),
            title: entry.spec.label.into(),
            observation: format!(
                "{} measured · {}. {}",
                cache::format_kb(entry.size_kb),
                entry.status.label(),
                entry.status.explanation()
            ),
            consequence: entry.spec.note.into(),
            size_kb: entry.size_kb,
            quick_win: quick_win(entry),
            target: Target::Cache(entry.spec.path.clone()),
            related_pids: pids,
        });
    }
    for process in processes
        .iter()
        .filter(|p| {
            p.health != processes::ProcessHealth::Running
                || metrics.cpu.get(&p.pid).is_some_and(|cpu| *cpu >= 25.)
                || metrics
                    .rss_kb
                    .get(&p.pid)
                    .or(p.rss_kb.as_ref())
                    .is_some_and(|rss| *rss >= 512 * 1024)
        })
        .take(20)
    {
        if results
            .iter()
            .any(|f| f.related_pids.contains(&process.pid))
        {
            continue;
        }
        let cpu = metrics
            .cpu
            .get(&process.pid)
            .map(|cpu| format!("{cpu:.1}% CPU over {:.1}s", metrics.interval_secs))
            .unwrap_or_else(|| "CPU interval unavailable".into());
        let rss = metrics
            .rss_kb
            .get(&process.pid)
            .copied()
            .or(process.rss_kb)
            .map(cache::format_kb)
            .unwrap_or_else(|| "RAM unavailable".into());
        let system_note = if process.system_owned {
            "System-owned · read-only observation"
        } else {
            "Current-account process"
        };
        let consequence = if process.system_owned
            && Path::new(&process.command)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("fseventsd"))
        {
            "fseventsd is macOS's filesystem-event journal service. High RAM can reflect event backlog or heavy filesystem activity; it is not reclaimable storage. Inspect filesystem activity before taking action; never terminate this system daemon from Mac Cleanup."
        } else if process.system_owned {
            "This system-owned process is read-only here. High RAM is an observation, not reclaimable memory; inspect its activity and memory pressure before deciding what to do."
        } else {
            process.health.explanation()
        };
        let activity_note = if process.system_owned
            && Path::new(&process.command)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("fseventsd"))
        {
            format!(
                " · swap in {} / out {}",
                metrics
                    .swap_in_kb_s
                    .map(|rate| format!("{rate:.1} KiB/s"))
                    .unwrap_or_else(|| "unavailable".into()),
                metrics
                    .swap_out_kb_s
                    .map(|rate| format!("{rate:.1} KiB/s"))
                    .unwrap_or_else(|| "unavailable".into())
            )
        } else {
            String::new()
        };
        results.push(Finding {
            id: format!("process:{}:{}", process.pid, process.start_time),
            title: ai::display_text(
                Path::new(&process.command)
                    .file_name()
                    .and_then(|p| p.to_str())
                    .unwrap_or(&process.command),
            ),
            observation: format!(
                "{} · {} · {} · PID {} · {}",
                process.health.label(),
                rss,
                cpu,
                process.pid,
                system_note
            ) + &activity_note,
            consequence: consequence.into(),
            size_kb: 0,
            quick_win: false,
            target: Target::Process(process.pid, process.start_time.clone()),
            related_pids: vec![process.pid],
        });
    }
    if let Some(inventory) = inventory {
        for item in &inventory.top_level {
            if results
                .iter()
                .any(|f| matches!(&f.target,Target::Cache(path) if path==&item.path))
            {
                continue;
            }
            results.push(Finding {
                id: format!("path:{}", item.path.display()),
                title: ai::display_text(&item.path.display().to_string()),
                observation: format!(
                    "{} {} · size does not establish waste",
                    cache::format_kb(item.size_kb),
                    if inventory.complete {
                        "measured"
                    } else {
                        "observed in a partial scan"
                    }
                ),
                consequence:
                    "Inspect contents; useful data can be moved to a verified external destination."
                        .into(),
                size_kb: item.size_kb,
                quick_win: false,
                target: Target::Folder(item.path.clone()),
                related_pids: vec![],
            });
        }
    }
    results.sort_by(|a, b| {
        b.quick_win
            .cmp(&a.quick_win)
            .then_with(|| b.size_kb.cmp(&a.size_kb))
            .then_with(|| a.id.cmp(&b.id))
    });
    results
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedAction {
    pub target: String,
    pub result: String,
    pub removed_kb: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub schema_version: u32,
    pub id: u64,
    pub updated: u64,
    pub state: String,
    pub actions: Vec<RecordedAction>,
    pub before: Option<Metrics>,
    pub after: Option<Metrics>,
    pub free_before_kb: Option<u64>,
    pub free_after_kb: Option<u64>,
    pub measurements: HashMap<String, u64>,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            schema_version: 1,
            id: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            updated: timestamp(),
            state: "Assessment".into(),
            actions: vec![],
            before: None,
            after: None,
            free_before_kb: None,
            free_after_kb: None,
            measurements: HashMap::new(),
        }
    }
}
fn history_directory(home: &Path) -> io::Result<PathBuf> {
    if !fs::symlink_metadata(home)?.is_dir() {
        return Err(io::Error::other("History home must be a real directory"));
    }
    let mut directory = home.to_path_buf();
    for component in Path::new(HISTORY_SUBPATH).components() {
        directory.push(component);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(io::Error::other(
                    "History directory redirects or is not a directory",
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(&directory)?,
            Err(e) => return Err(e),
        }
    }
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    Ok(directory)
}
pub fn save_session(home: &Path, session: &Session) -> io::Result<()> {
    let directory = history_directory(home)?;
    let target = directory.join(format!("{}.json", session.id));
    let temporary = directory.join(format!(".{}-{}.tmp", session.id, std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let bytes = serde_json::to_vec(session)?;
    if let Err(error) = file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| fs::rename(&temporary, &target))
    {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let mut records = fs::read_dir(&directory)?
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|e| e == "json"))
        .collect::<Vec<_>>();
    records.sort_by_key(|e| e.file_name());
    let mut total = records
        .iter()
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum::<u64>();
    for record in records {
        let metadata = fs::symlink_metadata(record.path())?;
        let old = metadata
            .modified()
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > Duration::from_secs(30 * 86400));
        if metadata.is_file() && (old || total > 50 * 1024 * 1024) && record.path() != target {
            fs::remove_file(record.path())?;
            total = total.saturating_sub(metadata.len());
        }
    }
    Ok(())
}
pub fn sessions(home: &Path) -> Vec<Session> {
    let mut checked = home.to_path_buf();
    if !fs::symlink_metadata(&checked).is_ok_and(|m| m.is_dir()) {
        return vec![];
    }
    for component in Path::new(HISTORY_SUBPATH).components() {
        checked.push(component);
        if !fs::symlink_metadata(&checked).is_ok_and(|m| m.is_dir()) {
            return vec![];
        }
    }
    let directory = home.join(HISTORY_SUBPATH);
    let Ok(reader) = fs::read_dir(directory) else {
        return vec![];
    };
    let mut results = reader
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).ok()?;
            if !metadata.is_file()
                || metadata.len() > 2 * 1024 * 1024
                || path.extension()?.to_str()? != "json"
            {
                return None;
            }
            let mut session: Session = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
            if path.file_name()?.to_str()? != format!("{}.json", session.id) {
                return None;
            }
            if session.state.starts_with("Running ")
                || session.state.starts_with("Measuring pre-action")
                || session.state == "Verifying results"
            {
                session.state =
                    format!("Verification incomplete · {} · not resumed", session.state);
            }
            (session.schema_version == 1).then_some(session)
        })
        .collect::<Vec<_>>();
    results.sort_by_key(|s| std::cmp::Reverse(s.id));
    results
}

/// Remove this application's validated session records, leaving other files and audit logs alone.
pub fn clear_sessions(home: &Path) -> io::Result<usize> {
    let directory = history_directory(home)?;
    let mut count = 0;
    for session in sessions(home) {
        let path = directory.join(format!("{}.json", session.id));
        if fs::symlink_metadata(&path)?.is_file() {
            fs::remove_file(path)?;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_duration_is_parsed_as_seconds() {
        assert_eq!(cpu_seconds("01:02.50"), Some(62.5));
        assert_eq!(cpu_seconds("1:02:03"), Some(3723.));
        assert_eq!(cpu_seconds("bad"), None);
    }
    #[test]
    fn native_rate_counters_require_complete_device_and_page_data() {
        assert_eq!(
            vm_counters(
                "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nSwapins: 12.\nSwapouts: 34.\n"
            ),
            Some((16384, 12, 34))
        );
        assert!(vm_counters("Swapins: 12.").is_none());
        let disks =
            disk_counters("disk0 disk4\nKB/t xfrs MB KB/t xfrs MB\n4.0 12 8.5 2.0 24 3.0\n")
                .unwrap();
        assert_eq!(disks.get("disk0"), Some(&8.5));
        assert_eq!(disks.get("disk4"), Some(&3.));
        assert!(disk_counters("disk0 disk4\nheaders\n4 12 8.5").is_none());
    }
    #[test]
    fn large_system_fseventsd_finding_explains_swap_activity_and_stays_read_only() {
        let process = ProcessEntry {
            pid: 74514539,
            parent_pid: 1,
            uid: 0,
            state: "S".into(),
            elapsed: "01:02".into(),
            cpu_percent: "0.0".into(),
            command: "/usr/sbin/fseventsd".into(),
            rss_kb: Some(900 * 1024),
            system_owned: true,
            health: processes::ProcessHealth::Running,
            signalable: false,
            signal_block_reason: Some("system-owned".into()),
            outcome: None,
            start_time: "Sun Aug 23 16:05:46 2026".into(),
        };
        let mut metrics = Metrics {
            swap_in_kb_s: Some(1200.),
            swap_out_kb_s: Some(80.),
            ..Default::default()
        };
        metrics.rss_kb.insert(process.pid, 900 * 1024);
        let finding = findings(&[], &[process], &metrics, None)
            .into_iter()
            .find(|finding| finding.title == "fseventsd")
            .expect("fseventsd finding");
        assert!(finding.observation.contains("swap in 1200.0 KiB/s"));
        assert!(
            finding
                .consequence
                .contains("filesystem-event journal service")
        );
        assert!(!finding.quick_win);
    }
    #[test]
    fn interrupted_sessions_are_not_resumed_and_clear_preserves_other_files() {
        let home = tempfile::tempdir().unwrap();
        let session = Session {
            state: "Running cleanup".into(),
            ..Default::default()
        };
        save_session(home.path(), &session).unwrap();
        let unrelated = home.path().join(HISTORY_SUBPATH).join("notes.json");
        fs::write(&unrelated, "personal note").unwrap();
        assert!(
            sessions(home.path())[0]
                .state
                .starts_with("Verification incomplete")
        );
        assert_eq!(clear_sessions(home.path()).unwrap(), 1);
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "personal note");
    }
    #[test]
    fn history_roundtrip_and_symlink_refusal() {
        let home = tempfile::tempdir().unwrap();
        let session = Session::default();
        save_session(home.path(), &session).unwrap();
        assert_eq!(sessions(home.path()).len(), 1);
        let other = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(other.path(), home.path().join("redirect")).unwrap();
        assert!(save_session(&home.path().join("redirect"), &session).is_err());
        assert!(sessions(&home.path().join("redirect")).is_empty());
    }
}
