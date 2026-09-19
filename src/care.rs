//! Shared assessment, policy, and outcome records for the visual care workflow.
use crate::{
    ai,
    cache::{self, CacheEntry, CacheStatus, CacheTier},
    investigation::{CheckObservation, EvidenceKind, EvidenceStatus},
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
pub const SETTINGS_SUBPATH: &str = "Library/Application Support/mac-cleanup/settings.json";

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

/// Capture a short, read-only filesystem-activity sample for fseventsd. The
/// caller must obtain explicit in-app approval first. macOS owns the
/// administrator prompt; this process never reads or stores a password.
pub fn observe_fseventsd(cancel_requested: &AtomicBool) -> CheckObservation {
    const SCRIPT: &str = "do shell script \"/usr/bin/fs_usage -w -f filesys -t 8 fseventsd\" with administrator privileges";
    let child = Command::new("/usr/bin/osascript")
        .args(["-e", SCRIPT])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            return CheckObservation::unavailable(
                EvidenceKind::FilesystemActivity,
                EvidenceStatus::Unsupported,
                format!("Filesystem activity probe unavailable: {error}"),
            );
        }
    };
    let Some(stdout) = child.stdout.take() else {
        return CheckObservation::unavailable(
            EvidenceKind::FilesystemActivity,
            EvidenceStatus::Failed,
            "Filesystem activity probe returned no output.",
        );
    };
    let Some(stderr) = child.stderr.take() else {
        return CheckObservation::unavailable(
            EvidenceKind::FilesystemActivity,
            EvidenceStatus::Failed,
            "Filesystem activity probe returned no diagnostics.",
        );
    };
    let output_reader = thread::spawn(move || {
        let mut output = String::new();
        stdout
            .take(1_000_001)
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
            return CheckObservation::unavailable(
                EvidenceKind::FilesystemActivity,
                EvidenceStatus::Cancelled,
                "Filesystem activity probe cancelled.",
            );
        }
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) if started.elapsed() < Duration::from_secs(10) => {
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
                return CheckObservation::unavailable(
                    EvidenceKind::FilesystemActivity,
                    EvidenceStatus::Failed,
                    format!("Filesystem activity probe failed: {error}"),
                );
            }
        }
    };
    let output = match output_reader
        .join()
        .map_err(|_| "filesystem activity reader stopped".to_string())
        .and_then(|result| result.map_err(|error| error.to_string()))
    {
        Ok(output) => output,
        Err(error) => {
            return CheckObservation::unavailable(
                EvidenceKind::FilesystemActivity,
                EvidenceStatus::Failed,
                error,
            );
        }
    };
    let error = match error_reader
        .join()
        .map_err(|_| "filesystem activity diagnostics stopped".to_string())
        .and_then(|result| result.map_err(|error| error.to_string()))
    {
        Ok(error) => error,
        Err(error) => error,
    };
    if !output.lines().all(|line| line.trim().is_empty()) {
        return summarize_fseventsd_sample(&output, timed_out);
    }
    let diagnostic = ai::display_text(error.trim());
    let lowered = diagnostic.to_ascii_lowercase();
    let cancelled = lowered.contains("user canceled") || lowered.contains("-128");
    let permission = lowered.contains("not authorized")
        || lowered.contains("administrator")
        || lowered.contains("privilege");
    CheckObservation::unavailable(
        EvidenceKind::FilesystemActivity,
        if cancelled {
            EvidenceStatus::Cancelled
        } else if permission {
            EvidenceStatus::PermissionRequired
        } else if timed_out {
            EvidenceStatus::TimedOut
        } else {
            EvidenceStatus::Failed
        },
        if diagnostic.is_empty() {
            "No filesystem events were captured.".into()
        } else {
            format!("No filesystem events captured: {diagnostic}")
        },
    )
}

fn summarize_fseventsd_sample(output: &str, timed_out: bool) -> CheckObservation {
    let mut total = 0_usize;
    let mut page_ins = 0_usize;
    let mut page_outs = 0_usize;
    let mut reads = 0_usize;
    let mut writes = 0_usize;
    let mut metadata = 0_usize;
    let mut swap_paths = 0_usize;
    let mut external_paths = 0_usize;
    for raw in output.lines().filter(|line| !line.trim().is_empty()) {
        let line = raw.to_ascii_lowercase();
        total += 1;
        page_ins += usize::from(line.contains("pgin"));
        page_outs += usize::from(line.contains("pgout"));
        reads += usize::from(line.contains(" read "));
        writes += usize::from(line.contains(" write "));
        metadata += usize::from(
            line.contains(" getattrlist ")
                || line.contains(" stat ")
                || line.contains(" open ")
                || line.contains(" lstat "),
        );
        swap_paths += usize::from(line.contains("/system/volumes/vm/swapfile"));
        external_paths +=
            usize::from(line.contains("/volumes/") && !line.contains("/system/volumes/"));
    }
    let ordinary_ops = reads + writes + metadata;
    let mut observation = CheckObservation::complete(
        EvidenceKind::FilesystemActivity,
        format!(
            "Eight-second fseventsd trace: {total} events · page-ins {page_ins} · page-outs {page_outs} · reads {reads} · writes {writes} · metadata {metadata} · VM swap paths {swap_paths} · external-volume paths {external_paths}. {} Thread-number suffixes in fs_usage output are not process IDs. Paging shows observed VM filesystem work; it does not prove an event storm or that fseventsd caused memory pressure.",
            if timed_out {
                "The outer safety deadline ended the sample."
            } else {
                "The requested sample completed."
            }
        ),
    );
    if timed_out || output.len() > 1_000_000 {
        observation.status = EvidenceStatus::Partial;
    }
    if ordinary_ops > 0 {
        observation.supports.push("filesystem_activity".into());
    }
    if external_paths > 0 {
        observation.supports.push("volume_specific".into());
    }
    observation
}

/// Capture mounted-volume context without reading file contents or changing mounts.
pub fn observe_volume_context(cancel_requested: &AtomicBool) -> CheckObservation {
    if cancel_requested.load(Ordering::Relaxed) {
        return CheckObservation::unavailable(
            EvidenceKind::VolumeContext,
            EvidenceStatus::Cancelled,
            "Volume context check cancelled.",
        );
    }
    let mounts = match query("/sbin/mount", &[], Duration::from_secs(3)) {
        Ok(mounts) => mounts,
        Err(error) => {
            return CheckObservation::unavailable(
                EvidenceKind::VolumeContext,
                EvidenceStatus::Failed,
                format!("Mounted-volume inventory unavailable: {error}"),
            );
        }
    };
    if cancel_requested.load(Ordering::Relaxed) {
        return CheckObservation::unavailable(
            EvidenceKind::VolumeContext,
            EvidenceStatus::Cancelled,
            "Volume context check cancelled.",
        );
    }
    let disk = query("/bin/df", &["-k", "-P"], Duration::from_secs(3))
        .unwrap_or_else(|_| "disk capacity details unavailable".into());
    let mount_lines = mounts
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(24)
        .map(ai::display_text)
        .collect::<Vec<_>>();
    let disk_lines = disk
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(12)
        .map(ai::display_text)
        .collect::<Vec<_>>();
    let external_or_custom = mount_lines
        .iter()
        .filter(|line| line.contains(" on /Volumes/") || !line.contains("(apfs"))
        .count();
    let mut observation = CheckObservation::complete(
        EvidenceKind::VolumeContext,
        format!(
            "Mounted-volume context (read-only):\n{}\nCapacity context:\n{}",
            if mount_lines.is_empty() {
                "unavailable".into()
            } else {
                mount_lines.join("\n")
            },
            if disk_lines.is_empty() {
                "unavailable".into()
            } else {
                disk_lines.join("\n")
            }
        ),
    );
    if external_or_custom > 0 {
        observation.summary.push_str(&format!(
            "\nObserved {external_or_custom} external or non-APFS mount entries; presence alone does not establish activity."
        ));
    }
    observation
}

/// Research a fixed, no-key source catalog. The caller supplies the saved
/// explicit network-consent state, so an automatic case never enables access.
pub fn research_sources(cancel_requested: &AtomicBool, online: bool) -> Result<String, String> {
    const SOURCES: [(&str, &str); 2] = [
        (
            "Apple File System Events Programming Guide",
            "https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/Introduction/Introduction.html",
        ),
        (
            "Apple fs_usage manual",
            "https://developer.apple.com/library/archive/documentation/Darwin/Reference/ManPages/man1/fs_usage.1.html",
        ),
    ];
    let mut lines = vec![if online {
        "Online research enabled for the fixed Apple source catalog.".into()
    } else {
        "Bundled Apple reference notes; online research is disabled.".into()
    }];
    lines.push("Bundled · FSEvents stores per-volume event logs; clients may receive coarse notifications and must rescan affected hierarchy when events are coalesced.".into());
    lines.push("Bundled · fs_usage reports system calls and page faults; PgIn/PgOut rows are paging observations, while a wide-mode process suffix is a thread identifier.".into());
    for (title, url) in SOURCES {
        if cancel_requested.load(Ordering::Relaxed) {
            return Err("source research cancelled".into());
        }
        if online {
            let fetched = query(
                "/usr/bin/curl",
                &[
                    "--fail",
                    "--location",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "5",
                    url,
                ],
                Duration::from_secs(6),
            )
            .map(|body| {
                let excerpt = document_excerpt(&body, 900);
                if excerpt.is_empty() {
                    format!("{title} · fetched, but no readable excerpt was extracted")
                } else {
                    format!("{title} · excerpt: {excerpt}")
                }
            })
            .unwrap_or_else(|error| {
                format!(
                    "{} · fetch unavailable: {}",
                    title,
                    ai::display_text(&error.to_string())
                )
            });
            lines.push(format!("{} · {}", fetched, url));
        } else {
            lines.push(format!("{} · {}", title, url));
        }
    }
    Ok(lines.join("\n"))
}

fn document_excerpt(body: &str, limit: usize) -> String {
    let mut text = String::with_capacity(limit);
    let mut inside_tag = false;
    let mut last_space = false;
    for character in body.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => {
                inside_tag = false;
                if !last_space && !text.is_empty() {
                    text.push(' ');
                    last_space = true;
                }
            }
            _ if inside_tag => {}
            '&' => {
                if !last_space && !text.is_empty() {
                    text.push(' ');
                    last_space = true;
                }
            }
            value if value.is_whitespace() => {
                if !last_space && !text.is_empty() {
                    text.push(' ');
                    last_space = true;
                }
            }
            value => {
                text.push(value);
                last_space = false;
            }
        }
        if text.len() >= limit {
            break;
        }
    }
    ai::display_text(text.trim())
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
#[serde(default)]
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
    pub investigations: Vec<crate::investigation::InvestigationCase>,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            schema_version: 2,
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
            investigations: Vec::new(),
        }
    }
}
#[derive(Debug, Default, Serialize, Deserialize)]
struct Settings {
    online_research: bool,
}

fn app_data_directory(home: &Path) -> io::Result<PathBuf> {
    if !fs::symlink_metadata(home)?.is_dir() {
        return Err(io::Error::other(
            "Application home must be a real directory",
        ));
    }
    let mut directory = home.to_path_buf();
    for component in Path::new("Library/Application Support/mac-cleanup").components() {
        directory.push(component);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(io::Error::other(
                    "Application data directory redirects or is not a directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&directory)?,
            Err(error) => return Err(error),
        }
    }
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    Ok(directory)
}

pub fn online_research_enabled(home: &Path) -> bool {
    let path = home.join(SETTINGS_SUBPATH);
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return false;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16_384 {
        return false;
    }
    serde_json::from_slice::<Settings>(&fs::read(path).unwrap_or_default())
        .is_ok_and(|settings| settings.online_research)
}

pub fn set_online_research(home: &Path, enabled: bool) -> io::Result<()> {
    let directory = app_data_directory(home)?;
    let target = directory.join("settings.json");
    if fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::other(
            "Settings file redirects through a symlink",
        ));
    }
    let temporary = directory.join(format!(".settings-{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let bytes = serde_json::to_vec(&Settings {
        online_research: enabled,
    })?;
    if let Err(error) = file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| fs::rename(&temporary, &target))
    {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn history_directory(home: &Path) -> io::Result<PathBuf> {
    let mut directory = app_data_directory(home)?;
    directory.push("sessions");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(io::Error::other(
                "History directory redirects or is not a directory",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&directory)?,
        Err(error) => return Err(error),
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
            matches!(session.schema_version, 1 | 2).then_some(session)
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
    fn fseventsd_trace_separates_paging_from_filesystem_operations() {
        let paging = summarize_fseventsd_sample(
            "15:41:40.014840 PgIn[S] D=0x1 B=0x1000 /System/Volumes/VM/swapfile10 0.0005 W fseventsd.74514539\n",
            false,
        );
        assert!(paging.summary.contains("page-ins 1"));
        assert!(paging.summary.contains("not process IDs"));
        assert!(paging.supports.is_empty());

        let activity = summarize_fseventsd_sample(
            "15:41:40.021310 read F=4 B=0x1a1 /Volumes/Work/project 0.28 fseventsd.74514552\n",
            false,
        );
        assert_eq!(
            activity.supports,
            vec!["filesystem_activity", "volume_specific"]
        );
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

    #[test]
    fn online_research_preference_is_private_and_persistent() {
        let home = tempfile::tempdir().unwrap();
        assert!(!online_research_enabled(home.path()));
        set_online_research(home.path(), true).unwrap();
        assert!(online_research_enabled(home.path()));
        let settings = fs::metadata(home.path().join(SETTINGS_SUBPATH)).unwrap();
        assert_eq!(settings.permissions().mode() & 0o777, 0o600);
        set_online_research(home.path(), false).unwrap();
        assert!(!online_research_enabled(home.path()));
    }

    #[test]
    fn document_excerpt_removes_markup_and_stays_bounded() {
        let excerpt = document_excerpt(
            "<html><head><title>Events</title></head><body><p>Per-volume event history.</p></body></html>",
            48,
        );
        assert!(!excerpt.contains('<'));
        assert!(excerpt.contains("Events"));
        assert!(excerpt.len() <= 48);
    }

    #[test]
    fn schema_one_history_remains_readable_with_empty_investigations() {
        let home = tempfile::tempdir().unwrap();
        let directory = history_directory(home.path()).unwrap();
        let session = Session::default();
        let legacy = serde_json::json!({
            "schema_version": 1,
            "id": session.id,
            "updated": session.updated,
            "state": "Assessment",
            "actions": [],
            "before": null,
            "after": null,
            "free_before_kb": null,
            "free_after_kb": null,
            "measurements": {}
        });
        fs::write(
            directory.join(format!("{}.json", session.id)),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let loaded = sessions(home.path());
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].investigations.is_empty());
    }
}
