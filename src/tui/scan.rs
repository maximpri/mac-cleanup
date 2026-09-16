use super::*;

/// Background result combining `/private/tmp` retention discovery with any
/// incomplete-download candidates for the account home.
pub(super) struct ScanExtras {
    pub retention: TempRetentionScan,
    pub download_specs: Vec<CacheSpec>,
}

impl DirectoryScan {
    pub(super) fn new(spec: CacheSpec, status: CacheStatus) -> Self {
        let mut seen = HashSet::new();
        let mut allocated_blocks = 0;
        let mut errors = 0;
        let mut is_directory = false;
        let mut identity = None;
        match fs::symlink_metadata(&spec.path) {
            Ok(metadata) => {
                seen.insert((metadata.dev(), metadata.ino()));
                allocated_blocks = metadata.blocks();
                is_directory = metadata.is_dir();
                identity = PathIdentity::capture(&spec.path);
                if identity.is_none() {
                    errors += 1;
                }
            }
            Err(_) => errors += 1,
        }
        let readers = if spec.target == CacheTarget::ExactPath && !is_directory {
            Vec::new()
        } else {
            match fs::read_dir(&spec.path) {
                Ok(reader) => vec![reader],
                Err(_) => {
                    errors += 1;
                    Vec::new()
                }
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
            identity,
            started_at: Instant::now(),
        }
    }

    /// Process a bounded slice of filesystem work so drawing and keyboard
    /// handling get a chance to run between slices.
    pub(super) fn advance(&mut self) -> bool {
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
            // Aged targets only release entries old enough to be cleaned, so
            // the reported size describes what a cleanup would remove. The
            // depth-1 reader stack is processing the target's own children.
            if self.readers.len() == 1
                && let CacheTarget::AgedContents { min_age_days } = self.spec.target
            {
                let stale = metadata
                    .modified()
                    .is_ok_and(|modified| modified <= age_cutoff(min_age_days));
                if !stale {
                    continue;
                }
            }
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

    pub(super) fn size_kb(&self) -> u64 {
        self.allocated_blocks.saturating_add(1) / 2
    }

    pub(super) fn into_entry(self) -> CacheEntry {
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
            identity: self.identity,
        }
    }
}

pub(super) fn spawn_retention_worker(
    scan_root: PathBuf,
    account_home: PathBuf,
    retention_days: u64,
) -> RetentionWorker {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let retention =
            TempRetentionScan::discover_for_scan(&scan_root, &account_home, retention_days);
        // Incomplete downloads belong to the account home, so they are only
        // offered for scans that actually include it.
        let download_specs = if scan_root == Path::new("/") || scan_root == account_home {
            crate::downloads::discover_specs(&account_home, retention_days)
        } else {
            Vec::new()
        };
        let _ = sender.send(ScanExtras {
            retention,
            download_specs,
        });
    });
    RetentionWorker { receiver }
}

pub(super) fn spawn_cleanup_worker(
    entries: Vec<(usize, CacheEntry)>,
    allowlist: Vec<PathBuf>,
    scan_root: PathBuf,
    account_home: PathBuf,
    whitelist: Whitelist,
    cleanup_kind: CleanupKind,
    include_reinstallable: bool,
) -> CleanupWorker {
    let (sender, receiver) = mpsc::channel();
    let stop_requested = Arc::new(AtomicBool::new(false));
    let worker_stop_requested = Arc::clone(&stop_requested);

    thread::spawn(move || {
        let free_before_kb = free_kb(&scan_root);
        for (position, (entry_index, mut entry)) in entries.into_iter().enumerate() {
            if position > 0 && worker_stop_requested.load(Ordering::Relaxed) {
                break;
            }
            let outcome = match cleanup_kind {
                CleanupKind::Cache => {
                    clean_cache(&mut entry, &allowlist, include_reinstallable, &whitelist)
                }
                CleanupKind::ReviewData => clean_review_data(&mut entry, &allowlist, &whitelist),
            };
            crate::history::record(&account_home, &entry, &outcome);
            if sender
                .send(CleanupMessage::ItemFinished {
                    entry_index,
                    entry: Box::new(entry),
                    outcome,
                })
                .is_err()
            {
                return;
            }
            if worker_stop_requested.load(Ordering::Relaxed) {
                break;
            }
        }
        let _ = sender.send(CleanupMessage::Finished {
            free_before_kb,
            free_after_kb: free_kb(&scan_root),
        });
    });

    CleanupWorker {
        receiver,
        stop_requested,
    }
}
