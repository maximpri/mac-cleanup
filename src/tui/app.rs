use super::*;

impl App {
    pub(super) fn new(cli: &Cli, home: &Path) -> Result<Self, String> {
        let requested_root = cli.volume.as_deref().map(validate_scan_root).transpose()?;
        let mut locations = scan_locations(home);
        if let Some(path) = &requested_root
            && !locations.iter().any(|location| location.path == *path)
        {
            locations.push(ScanLocation {
                label: path
                    .file_name()
                    .filter(|name| !name.is_empty())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Selected volume".into()),
                path: path.clone(),
                kind: scan_location_kind(path),
            });
        }
        let location_cursor = requested_root
            .as_ref()
            .and_then(|requested| {
                locations
                    .iter()
                    .position(|location| location.path == *requested)
            })
            .or_else(|| {
                locations
                    .iter()
                    .position(|location| location.path == Path::new("/"))
            })
            .unwrap_or(0);
        let scan_root = requested_root.clone().unwrap_or_else(|| {
            locations
                .get(location_cursor)
                .map(|location| location.path.clone())
                .unwrap_or_else(|| home.to_path_buf())
        });
        // Retention discovery can invoke lsof and measure large stale trees,
        // so it runs off-thread while the audit screen stays responsive.
        let temp_retention = TempRetentionScan::pending(cli.tmp_retention_days);
        let retention_worker = Some(spawn_retention_worker(
            scan_root.clone(),
            home.to_path_buf(),
            cli.tmp_retention_days,
        ));
        let specs = scan_specs(&scan_root, home);
        let now = Instant::now();
        Ok(Self {
            mode: cli.mode(),
            analysis_only: cli.analyze,
            account_home: home.to_path_buf(),
            scan_root: scan_root.clone(),
            locations,
            location_cursor,
            specs,
            entries: Vec::new(),
            whitelist: Whitelist::load(home),
            // Start with the useful work. The audit runs in the background
            // and opens the storage review when its inventory is ready.
            phase: Phase::Scanning,
            include_reinstallable: cli.include_reinstallable,
            tmp_retention_days: cli.tmp_retention_days,
            temp_retention,
            retention_worker,
            no_color: cli.no_color || std::env::var_os("NO_COLOR").is_some(),
            scan_index: 0,
            scan_task: None,
            inventory: None,
            storage_tab: StorageTab::Explore,
            explorer_path: None,
            explorer_cursor: 0,
            explorer_history: Vec::new(),
            coverage_scroll: 0,
            coverage_max_scroll: std::cell::Cell::new(0),
            explorer_details: false,
            inventory_worker: None,
            inventory_started_at: None,
            relocation_sources: Vec::new(),
            relocation_source_cursor: 0,
            relocation_source: None,
            relocation_destination: String::new(),
            relocation_destination_error: None,
            relocation_plan: None,
            relocation_plan_worker: None,
            relocation_worker: None,
            relocation_started_at: None,
            relocation_report: None,
            scan_started_at: now,
            scan_progress_ratio: 0.0,
            scan_progress_updated_at: now,
            scan_work_complete: false,
            cursor: 0,
            selected: BTreeSet::new(),
            cleanup_queue: Vec::new(),
            cleanup_index: 0,
            cleanup_kind: CleanupKind::Cache,
            cleanup_worker: None,
            cleanup_started_at: None,
            review_target: None,
            review_confirmation: String::new(),
            review_confirmation_error: false,
            processes: Vec::new(),
            process_cursor: 0,
            process_scan_error: None,
            process_scan_completed: false,
            process_scan_duration: None,
            process_target: None,
            stats: CleanupStats::default(),
            stopped_early: false,
            summary_started_at: None,
            status_message: None,
            menu_open: None,
            menu_cursor: 0,
            sidebar_cursor: 0,
            sidebar_focus: false,
            terminal_width: 80,
            terminal_height: 24,
            show_help: false,
            dialog_scroll: 0,
            dialog_max_scroll: std::cell::Cell::new(0),
            hit_regions: std::cell::RefCell::new(Vec::new()),
            quit: false,
        })
    }

    pub(super) fn advance_work(&mut self) {
        match self.phase {
            Phase::Scanning => {
                self.scan_next();
                self.advance_scan_progress();
            }
            Phase::Cleaning => self.advance_cleanup(),
            Phase::RelocationPlanning => self.poll_relocation_plan(),
            Phase::Relocating => self.poll_relocation_worker(),
            _ => {}
        }
    }

    pub(super) fn scan_next(&mut self) {
        if !self.poll_retention_worker() {
            return;
        }
        self.poll_inventory_worker();
        if self.scan_work_complete {
            return;
        }

        if let Some(task) = self.scan_task.as_mut() {
            if task.advance() {
                let entry = self
                    .scan_task
                    .take()
                    .expect("active scan task")
                    .into_entry();
                self.record_scan_entry(entry);
                self.scan_index += 1;
            }
            return;
        }

        let Some(spec) = self.specs.get(self.scan_index).cloned() else {
            if self.inventory_worker.is_none() {
                self.start_inventory_worker();
            }
            return;
        };

        let status = status_for(&spec, self.include_reinstallable);
        if matches!(
            status,
            CacheStatus::Missing
                | CacheStatus::ScanError
                | CacheStatus::Symlink
                | CacheStatus::Invalid
        ) {
            self.record_scan_entry(CacheEntry {
                spec,
                status,
                size_kb: 0,
                outcome: None,
                identity: None,
            });
            self.scan_index += 1;
        } else {
            self.scan_task = Some(DirectoryScan::new(spec, status));
        }
    }

    pub(super) fn poll_retention_worker(&mut self) -> bool {
        let result = self
            .retention_worker
            .as_ref()
            .map(|worker| worker.receiver.try_recv());
        match result {
            Some(Ok(extras)) => {
                self.temp_retention = extras.retention;
                self.specs.extend(self.temp_retention.cache_specs());
                self.specs.extend(extras.download_specs);
                self.retention_worker = None;
                true
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.retention_worker = None;
                self.status_message = Some(
                    "Temporary retention inspection stopped unexpectedly; no temporary entries were selected."
                        .into(),
                );
                true
            }
            Some(Err(TryRecvError::Empty)) => false,
            None => true,
        }
    }

    pub(super) fn start_inventory_worker(&mut self) {
        let (sender, receiver) = mpsc::channel();
        let stop_requested = Arc::new(AtomicBool::new(false));
        let worker_stop_requested = Arc::clone(&stop_requested);
        let account_home = self.account_home.clone();
        // Unit tests and embedded callers can provide a synthetic home while
        // retaining the startup-volume location in the picker. Do not let
        // that synthetic setup trigger a walk of the host's real `/`.
        let scan_root = if self.scan_root == Path::new("/")
            && std::env::var_os("HOME")
                .is_some_and(|home| home.as_os_str() != self.account_home.as_os_str())
        {
            account_home.clone()
        } else {
            self.scan_root.clone()
        };
        thread::spawn(move || {
            let inventory = StorageInventory::scan_with_cancel(
                &scan_root,
                &account_home,
                &worker_stop_requested,
            );
            let _ = sender.send(inventory);
        });
        self.inventory_started_at = Some(Instant::now());
        self.inventory_worker = Some(InventoryWorker {
            receiver,
            stop_requested,
        });
    }

    pub(super) fn poll_inventory_worker(&mut self) {
        let result = self
            .inventory_worker
            .as_ref()
            .map(|worker| worker.receiver.try_recv());
        match result {
            Some(Ok(inventory)) => {
                self.inventory = Some(inventory);
                self.inventory_worker = None;
                self.inventory_started_at = None;
                self.scan_work_complete = true;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.inventory = Some(StorageInventory::unavailable(
                    &self.scan_root,
                    "the background storage inventory stopped unexpectedly",
                ));
                self.inventory_worker = None;
                self.inventory_started_at = None;
                self.scan_work_complete = true;
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
    }

    pub(super) fn advance_scan_progress(&mut self) {
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.scan_progress_updated_at);
        self.scan_progress_updated_at = now;
        let target = if self.scan_work_complete {
            1.0
        } else if self.retention_worker.is_some() {
            let elapsed = self.scan_started_at.elapsed().as_secs_f64();
            0.01 + (0.04 * (1.0 - (-elapsed / 8.0).exp()))
        } else if let Some(started_at) = self.inventory_started_at {
            // The full-volume walk has no useful total-item denominator until
            // it finishes. Keep the completed allowlist work visible while
            // making it clear that the expensive disk inventory is active.
            let elapsed = started_at.elapsed().as_secs_f64();
            0.94 + (0.05 * (1.0 - (-elapsed / 30.0).exp()))
        } else {
            smooth_scan_ratio(
                self.scan_index,
                self.specs.len().max(1),
                self.scan_task
                    .as_ref()
                    .map(|task| task.started_at.elapsed()),
            )
            .min(0.94)
        };
        self.scan_progress_ratio = animate_scan_ratio(self.scan_progress_ratio, target, elapsed);

        if self.scan_work_complete && self.scan_progress_ratio >= 0.995 {
            self.scan_progress_ratio = 1.0;
            self.entries.sort_by_key(|entry| Reverse(entry.size_kb));
            self.phase = Phase::Review;
            self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
        }
    }

    pub(super) fn record_scan_entry(&mut self, entry: CacheEntry) {
        if entry.size_kb > 0
            || matches!(
                entry.status,
                CacheStatus::ScanError | CacheStatus::Symlink | CacheStatus::Invalid
            )
        {
            self.entries.push(entry);
        }
    }

    pub(super) fn restart_scan(&mut self) {
        self.sidebar_focus = false;
        self.explorer_details = false;
        self.explorer_path = None;
        self.explorer_cursor = 0;
        self.explorer_history.clear();
        self.coverage_scroll = 0;
        self.refresh_specs();
        let now = Instant::now();
        self.entries.clear();
        self.selected.clear();
        self.scan_index = 0;
        self.scan_task = None;
        if let Some(worker) = &self.inventory_worker {
            worker.stop_requested.store(true, Ordering::Relaxed);
        }
        self.inventory_worker = None;
        self.inventory = None;
        self.inventory_started_at = None;
        self.scan_started_at = now;
        self.scan_progress_ratio = 0.0;
        self.scan_progress_updated_at = now;
        self.scan_work_complete = false;
        self.cursor = 0;
        self.status_message = None;
        self.phase = Phase::Scanning;
    }

    pub(super) fn cancel_scan(&mut self) {
        self.scan_task = None;
        self.retention_worker = None;
        self.entries.clear();
        self.selected.clear();
        self.scan_index = 0;
        if let Some(worker) = &self.inventory_worker {
            worker.stop_requested.store(true, Ordering::Relaxed);
        }
        self.inventory_worker = None;
        self.inventory = None;
        self.inventory_started_at = None;
        self.scan_progress_ratio = 0.0;
        self.scan_work_complete = false;
        self.cursor = 0;
        self.phase = Phase::Location;
    }

    pub(super) fn choose_location(&mut self) {
        let Some(location) = self.locations.get(self.location_cursor) else {
            return;
        };
        self.scan_root = location.path.clone();
        self.restart_scan();
    }

    pub(super) fn refresh_specs(&mut self) {
        self.temp_retention = TempRetentionScan::pending(self.tmp_retention_days);
        self.specs = scan_specs(&self.scan_root, &self.account_home);
        self.retention_worker = Some(spawn_retention_worker(
            self.scan_root.clone(),
            self.account_home.clone(),
            self.tmp_retention_days,
        ));
    }

    pub(super) fn open_relocation_sources(&mut self) {
        self.sidebar_focus = false;
        if self.analysis_only {
            self.status_message = Some(
                "Relocation is disabled in read-only analysis mode; restart without --analyze."
                    .into(),
            );
            return;
        }
        let sources = self
            .inventory
            .as_ref()
            .map(|inventory| relocation_sources(inventory, &self.account_home))
            .unwrap_or_default();
        if sources.is_empty() {
            self.status_message = Some(
                "No relocatable user-owned directories were found in the largest-consumer inventory."
                    .into(),
            );
            return;
        }
        self.relocation_sources = sources;
        self.relocation_source_cursor = 0;
        self.relocation_source = None;
        self.relocation_destination.clear();
        self.relocation_destination_error = None;
        self.relocation_plan = None;
        self.relocation_report = None;
        self.status_message = None;
        self.phase = Phase::RelocationSources;
    }

    pub(super) fn choose_relocation_source(&mut self) {
        let Some(source) = self
            .relocation_sources
            .get(self.relocation_source_cursor)
            .cloned()
        else {
            return;
        };
        self.relocation_source = Some(source);
        self.relocation_destination = self
            .default_relocation_destination()
            .map_or_else(String::new, |path| path.display().to_string());
        self.relocation_destination_error = None;
        self.phase = Phase::RelocationDestination;
    }

    pub(super) fn default_relocation_destination(&self) -> Option<PathBuf> {
        self.locations
            .iter()
            .find(|location| {
                location.path.starts_with(Path::new("/Volumes"))
                    && location.kind == ScanLocationKind::Usb
            })
            .or_else(|| {
                self.locations.iter().find(|location| {
                    location.path.starts_with(Path::new("/Volumes"))
                        && location.kind != ScanLocationKind::Network
                })
            })
            .map(|location| location.path.clone())
    }

    pub(super) fn start_relocation_plan(&mut self) {
        let Some(source) = self
            .relocation_source
            .as_ref()
            .map(|item| item.path.clone())
        else {
            self.phase = Phase::RelocationSources;
            return;
        };
        let destination_text = self.relocation_destination.trim().to_owned();
        if destination_text.is_empty() {
            self.relocation_destination_error =
                Some("Enter an existing directory on the external volume.".into());
            return;
        }
        let destination = expand_user_path(&destination_text, &self.account_home);
        let account_home = self.account_home.clone();
        let process_pattern = self.relocation_process_pattern(&source);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = relocation::plan(&source, &destination, &account_home, process_pattern);
            let _ = sender.send(result);
        });
        self.relocation_destination_error = None;
        self.relocation_plan_worker = Some(RelocationPlanWorker { receiver });
        self.relocation_started_at = Some(Instant::now());
        self.phase = Phase::RelocationPlanning;
    }

    pub(super) fn relocation_process_pattern(&self, source: &Path) -> Option<&'static str> {
        let canonical_source = source.canonicalize().ok()?;
        scan_specs(Path::new("/"), &self.account_home)
            .into_iter()
            .find(|spec| spec.path.canonicalize().ok().as_deref() == Some(&canonical_source))
            .map(|spec| spec.process_pattern)
    }

    pub(super) fn poll_relocation_plan(&mut self) {
        let result = self
            .relocation_plan_worker
            .as_ref()
            .map(|worker| worker.receiver.try_recv());
        match result {
            Some(Ok(Ok(plan))) => {
                self.relocation_plan_worker = None;
                self.relocation_started_at = None;
                self.relocation_plan = Some(plan);
                self.relocation_destination_error = None;
                self.phase = Phase::RelocationConfirm;
            }
            Some(Ok(Err(error))) => self.fail_relocation_plan(error),
            Some(Err(TryRecvError::Disconnected)) => {
                self.fail_relocation_plan(
                    "the relocation validation worker stopped unexpectedly; no files were changed"
                        .into(),
                );
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
    }

    pub(super) fn fail_relocation_plan(&mut self, error: String) {
        self.relocation_plan_worker = None;
        self.relocation_started_at = None;
        self.relocation_plan = None;
        self.relocation_destination_error = Some(error);
        self.phase = Phase::RelocationDestination;
    }

    pub(super) fn begin_relocation(&mut self) {
        if self.analysis_only {
            self.phase = Phase::RelocationDestination;
            return;
        }
        let Some(plan) = self.relocation_plan.take() else {
            self.phase = Phase::RelocationDestination;
            return;
        };
        let template = relocation::preview(&plan);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let report = relocation::execute(&plan);
            let _ = sender.send(report);
        });
        self.relocation_report = None;
        self.relocation_worker = Some(RelocationWorker { receiver, template });
        self.relocation_started_at = Some(Instant::now());
        self.phase = Phase::Relocating;
    }

    pub(super) fn poll_relocation_worker(&mut self) {
        let result = self
            .relocation_worker
            .as_ref()
            .map(|worker| worker.receiver.try_recv());
        match result {
            Some(Ok(report)) => {
                self.relocation_worker = None;
                self.relocation_started_at = None;
                self.relocation_report = Some(report);
                self.phase = Phase::RelocationResult;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                if let Some(worker) = self.relocation_worker.take() {
                    let mut report = worker.template;
                    report.status = RelocationStatus::Failed;
                    report.message = Some(
                        "the relocation worker stopped unexpectedly; inspect the source and destination before retrying"
                            .into(),
                    );
                    self.relocation_report = Some(report);
                }
                self.relocation_started_at = None;
                self.phase = Phase::RelocationResult;
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
    }

    pub(super) fn close_relocation_result(&mut self) {
        self.relocation_report = None;
        self.relocation_plan = None;
        self.relocation_source = None;
        self.relocation_sources.clear();
        self.relocation_destination_error = None;
        self.restart_scan();
    }

    pub(super) fn advance_cleanup(&mut self) {
        if self.cleanup_worker.is_none() {
            self.start_cleanup_worker();
            return;
        }

        let message = self
            .cleanup_worker
            .as_ref()
            .expect("cleanup worker exists")
            .receiver
            .try_recv();
        match message {
            Ok(CleanupMessage::ItemFinished {
                entry_index,
                entry,
                outcome,
            }) => {
                if let Some(current) = self.entries.get_mut(entry_index) {
                    *current = *entry;
                }
                self.record_cleanup_outcome(&outcome);
                self.cleanup_index += 1;
            }
            Ok(CleanupMessage::Finished {
                free_before_kb,
                free_after_kb,
            }) => self.finish_cleanup(free_before_kb, free_after_kb),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.fail_disconnected_cleanup(),
        }
    }

    pub(super) fn start_cleanup_worker(&mut self) {
        let queued_entries = self
            .cleanup_queue
            .iter()
            .filter_map(|&index| self.entries.get(index).cloned().map(|entry| (index, entry)))
            .collect::<Vec<_>>();
        let include_reinstallable = self.include_reinstallable
            || queued_entries
                .iter()
                .any(|(_, entry)| entry.status == CacheStatus::Optional);
        let allowlist = self.specs.iter().map(|spec| spec.path.clone()).collect();
        self.cleanup_worker = Some(spawn_cleanup_worker(
            queued_entries,
            allowlist,
            self.scan_root.clone(),
            self.account_home.clone(),
            self.whitelist.clone(),
            self.cleanup_kind,
            include_reinstallable,
        ));
    }

    pub(super) fn record_cleanup_outcome(&mut self, outcome: &CleanupOutcome) {
        match outcome {
            CleanupOutcome::Cleared { removed_kb, .. } => {
                self.stats.cleared += 1;
                self.stats.measured_removed_kb += *removed_kb;
            }
            CleanupOutcome::SafetySkipped(_) => self.stats.safety_skipped += 1,
            CleanupOutcome::Failed { removed_kb, .. } => {
                self.stats.failed += 1;
                self.stats.measured_removed_kb += *removed_kb;
            }
        }
    }

    pub(super) fn fail_disconnected_cleanup(&mut self) {
        if let Some(&entry_index) = self.cleanup_queue.get(self.cleanup_index) {
            let outcome = CleanupOutcome::Failed {
                error: "the background cleanup worker stopped unexpectedly".into(),
                removed_kb: 0,
            };
            if let Some(entry) = self.entries.get_mut(entry_index) {
                entry.outcome = Some(outcome.clone());
            }
            self.record_cleanup_outcome(&outcome);
        }
        self.finish_cleanup(0, 0);
    }

    pub(super) fn finish_cleanup(&mut self, free_before_kb: u64, free_after_kb: u64) {
        self.cleanup_worker = None;
        self.cleanup_started_at = None;
        self.stats.filesystem_change_kb = free_after_kb.saturating_sub(free_before_kb);
        self.summary_started_at = Some(Instant::now());
        self.phase = Phase::Summary;
    }

    pub(super) fn request_cleanup_stop(&mut self) {
        self.stopped_early = true;
        if let Some(worker) = &self.cleanup_worker {
            worker.stop_requested.store(true, Ordering::Relaxed);
        } else {
            self.cleanup_queue.clear();
        }
    }

    pub(super) fn summary_remaining(&self) -> Option<Duration> {
        self.summary_started_at
            .map(|started_at| SUMMARY_AUTO_CLOSE_AFTER.saturating_sub(started_at.elapsed()))
    }

    pub(super) fn summary_has_timed_out(&self) -> bool {
        self.phase == Phase::Summary
            && self
                .summary_remaining()
                .is_some_and(|remaining| remaining.is_zero())
    }
}

impl App {
    pub(super) fn toggle_current(&mut self) {
        if !self.is_selectable(self.cursor) {
            return;
        }
        if !self.selected.remove(&self.cursor) {
            self.selected.insert(self.cursor);
        }
    }

    pub(super) fn toggle_all(&mut self) {
        let selectable: Vec<usize> = (0..self.entries.len())
            .filter(|&index| self.is_selectable(index))
            .collect();
        if !selectable.is_empty() && selectable.iter().all(|index| self.selected.contains(index)) {
            self.selected.clear();
        } else {
            self.selected.extend(selectable);
        }
    }

    pub(super) fn prepare_safe_cleanup(&mut self) {
        self.dialog_scroll = 0;
        self.mode = Mode::Clean;
        self.selected.clear();
        let selectable: Vec<usize> = (0..self.entries.len())
            .filter(|&index| self.is_selectable(index))
            .collect();
        self.selected.extend(selectable);
        self.phase = if self.selected.is_empty() {
            Phase::Review
        } else {
            Phase::Confirm
        };
    }

    pub(super) fn prepare_current_cleanup(&mut self) {
        if self.current_is_review_data() {
            self.mode = Mode::Clean;
            self.start_review_confirmation();
        } else if self.is_direct_cleanup_candidate(self.cursor) {
            self.mode = Mode::Clean;
            self.selected.clear();
            self.selected.insert(self.cursor);
            self.dialog_scroll = 0;
            self.phase = Phase::Confirm;
        }
    }

    pub(super) fn is_selectable(&self, index: usize) -> bool {
        self.entries.get(index).is_some_and(|entry| {
            entry.status == CacheStatus::Ready && entry.size_kb > 0 && entry.outcome.is_none()
        })
    }

    pub(super) fn is_direct_cleanup_candidate(&self, index: usize) -> bool {
        self.entries.get(index).is_some_and(|entry| {
            matches!(entry.status, CacheStatus::Ready | CacheStatus::Optional)
                && entry.size_kb > 0
                && entry.outcome.is_none()
        })
    }

    pub(super) fn current_is_optional(&self) -> bool {
        self.entries.get(self.cursor).is_some_and(|entry| {
            entry.status == CacheStatus::Optional && entry.size_kb > 0 && entry.outcome.is_none()
        })
    }

    pub(super) fn current_is_review_data(&self) -> bool {
        self.entries.get(self.cursor).is_some_and(|entry| {
            entry.status == CacheStatus::Review && entry.size_kb > 0 && entry.outcome.is_none()
        })
    }

    pub(super) fn start_review_confirmation(&mut self) {
        if !self.current_is_review_data() {
            return;
        }
        self.review_target = Some(self.cursor);
        self.review_confirmation.clear();
        self.review_confirmation_error = false;
        self.dialog_scroll = 0;
        self.phase = Phase::ReviewConfirm;
    }

    pub(super) fn reveal_current(&mut self) {
        let path = if self.phase == Phase::Review
            && self.storage_tab == StorageTab::Explore
            && self.inventory.is_some()
        {
            let Some(item) = self.explorer_items().get(self.explorer_cursor).cloned() else {
                return;
            };
            item.path.clone()
        } else {
            let Some(entry) = self.entries.get(self.cursor) else {
                return;
            };
            entry.spec.path.clone()
        };
        self.status_message = Some(
            match Command::new(OPEN_COMMAND)
                .arg("-R")
                .arg(&path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                Ok(status) if status.success() => {
                    format!("Revealed: {}", path.display())
                }
                Ok(status) => format!("Finder could not reveal the path (open exited {status})."),
                Err(error) => format!("Finder could not be opened: {error}"),
            },
        );
    }

    pub(super) fn handle_review_confirmation_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.review_target = None;
                self.review_confirmation.clear();
                self.review_confirmation_error = false;
                self.phase = Phase::Review;
            }
            KeyCode::Backspace => {
                self.review_confirmation.pop();
                self.review_confirmation_error = false;
            }
            KeyCode::Enter if self.review_confirmation == "DELETE" => {
                self.begin_review_cleanup();
            }
            KeyCode::Enter => self.review_confirmation_error = true,
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.review_confirmation.len() < "DELETE".len() =>
            {
                self.review_confirmation
                    .push(character.to_ascii_uppercase());
                self.review_confirmation_error = false;
            }
            _ => {}
        }
    }

    pub(super) fn begin_cleanup(&mut self) {
        self.cleanup_queue = self.selected.iter().copied().collect();
        self.cleanup_index = 0;
        self.cleanup_kind = CleanupKind::Cache;
        self.cleanup_worker = None;
        self.cleanup_started_at = Some(Instant::now());
        self.stats = CleanupStats::default();
        self.summary_started_at = None;
        self.stopped_early = false;
        self.phase = Phase::Cleaning;
    }

    pub(super) fn begin_review_cleanup(&mut self) {
        let Some(target) = self.review_target.take() else {
            self.phase = Phase::Review;
            return;
        };
        if !self.entries.get(target).is_some_and(|entry| {
            entry.status == CacheStatus::Review && entry.size_kb > 0 && entry.outcome.is_none()
        }) {
            self.phase = Phase::Review;
            return;
        }
        self.cleanup_queue = vec![target];
        self.cleanup_index = 0;
        self.cleanup_kind = CleanupKind::ReviewData;
        self.cleanup_worker = None;
        self.cleanup_started_at = Some(Instant::now());
        self.review_confirmation.clear();
        self.review_confirmation_error = false;
        self.stats = CleanupStats::default();
        self.summary_started_at = None;
        self.stopped_early = false;
        self.phase = Phase::Cleaning;
    }

    pub(super) fn identified_kb(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size_kb).sum()
    }

    pub(super) fn ready_kb(&self) -> u64 {
        self.entries
            .iter()
            .filter(|entry| entry.status == CacheStatus::Ready)
            .map(|entry| entry.size_kb)
            .sum()
    }

    pub(super) fn selected_kb(&self) -> u64 {
        self.selected
            .iter()
            .filter_map(|index| self.entries.get(*index))
            .map(|entry| entry.size_kb)
            .sum()
    }

    pub(super) fn optional_kb(&self) -> u64 {
        self.entries
            .iter()
            .filter(|entry| entry.status == CacheStatus::Optional)
            .map(|entry| entry.size_kb)
            .sum()
    }

    pub(super) fn review_kb(&self) -> u64 {
        self.entries
            .iter()
            .filter(|entry| entry.status == CacheStatus::Review)
            .map(|entry| entry.size_kb)
            .sum()
    }

    pub(super) fn flagged_process_count(&self) -> usize {
        self.processes
            .iter()
            .filter(|process| process.health != ProcessHealth::Running)
            .count()
    }

    pub(super) fn color(&self, color: Color) -> Color {
        if self.no_color { Color::Reset } else { color }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(worker) = &self.inventory_worker {
            worker.stop_requested.store(true, Ordering::Relaxed);
        }
        if let Some(worker) = &self.cleanup_worker {
            worker.stop_requested.store(true, Ordering::Relaxed);
        }
    }
}
