use super::*;
use crate::storage::{StorageCategory, StorageItemKind, StorageRoot, VolumeStats};

#[test]
fn storage_review_shows_volume_usage_and_largest_consumer() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: true,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.inventory = Some(StorageInventory {
        children: Default::default(),
        volume: Some(VolumeStats {
            accounting_path: temp.path().to_path_buf(),
            filesystem: "/dev/test".into(),
            capacity_kb: 250 * 1_048_576,
            used_kb: 247 * 1_048_576,
            free_kb: 3 * 1_048_576,
            container_free_kb: None,
            device: 1,
        }),
        volume_error: None,
        roots: vec![StorageRoot {
            path: temp.path().to_path_buf(),
            size_kb: 247 * 1_048_576,
            device: 1,
            scan_errors: 0,
        }],
        scanned_kb: 245 * 1_048_576,
        scanned_on_volume_kb: 245 * 1_048_576,
        unaccounted_kb: 2 * 1_048_576,
        inventory_overage_kb: 0,
        local_snapshots: Vec::new(),
        scanned_items: 42,
        scan_errors: 0,
        scan_error_paths: Vec::new(),
        complete: true,
        top_level: Vec::new(),
        largest: vec![
            StorageItem {
                path: PathBuf::from("/Users/maximp"),
                size_kb: 80 * 1_048_576,
                kind: StorageItemKind::Directory,
                category: StorageCategory::PersonalData,
            },
            StorageItem {
                path: PathBuf::from("/Users/maximp/Library"),
                size_kb: 70 * 1_048_576,
                kind: StorageItemKind::Directory,
                category: StorageCategory::ApplicationData,
            },
            StorageItem {
                path: PathBuf::from("/System/Library"),
                size_kb: 60 * 1_048_576,
                kind: StorageItemKind::Directory,
                category: StorageCategory::SystemData,
            },
            StorageItem {
                path: PathBuf::from("/private/tmp"),
                size_kb: 50 * 1_048_576,
                kind: StorageItemKind::Directory,
                category: StorageCategory::TemporaryData,
            },
        ],
    });
    // Supply a non-overlapping root list and a real child relationship.
    let inventory = app.inventory.as_mut().unwrap();
    inventory.top_level = [0, 2, 3]
        .into_iter()
        .map(|i| inventory.largest[i].clone())
        .collect();
    inventory.children.insert(
        PathBuf::from("/Users/maximp"),
        vec![inventory.largest[1].clone()],
    );
    inventory
        .children
        .insert(temp.path().to_path_buf(), inventory.top_level.clone());
    app.phase = Phase::Review;
    app.sidebar_focus = false;

    let backend = ratatui::backend::TestBackend::new(200, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &app)).unwrap();
    let rendered =
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .fold(String::new(), |mut text, cell| {
                text.push_str(cell.symbol());
                text
            });

    assert!(rendered.contains("Explore folders"));
    assert!(rendered.contains("247.0 GiB"));
    assert!(rendered.contains("80.0"));
    assert!(rendered.contains("/Users/maximp"));
    assert!(rendered.contains("/private/tmp"));
    assert!(rendered.contains("FOLDER / FILE"));
    assert!(rendered.contains("SELECTED ITEM"));
    assert!(rendered.contains("Library"));
    assert!(!rendered.contains("/Users/maximp/Library"));

    // A populated cleanup list must not hide the rest of the disk.
    app.entries.push(CacheEntry {
        spec: app.specs[0].clone(),
        status: CacheStatus::Ready,
        size_kb: 1024,
        outcome: None,
        identity: None,
    });
    let volume = app.inventory.as_mut().unwrap().volume.as_mut().unwrap();
    volume.used_kb = 180 * 1_048_576;
    volume.container_free_kb = Some(3 * 1_048_576);
    terminal.draw(|frame| render(frame, &app)).unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("Explore folders"));
    assert!(rendered.contains("Cleanup decisions"));
    app.handle_review_key(KeyCode::Enter);
    assert_eq!(
        app.explorer_path.as_deref(),
        Some(Path::new("/Users/maximp"))
    );
    assert_eq!(
        app.explorer_items()[0].path,
        PathBuf::from("/Users/maximp/Library")
    );
    assert!(rendered.contains("247.0 GiB"));
    app.handle_review_key(KeyCode::Char('v'));
    let rendered = buffer_text(&draw_fixture(&mut app, 200, 40));
    assert!(rendered.contains("67.0 GiB in other APFS"));
}

#[test]
fn relocation_ui_selects_an_inventory_directory_and_opens_destination_input() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Library/Developer/BuildData");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("artifact.bin"), [1_u8; 8_192]).unwrap();
    let canonical_source = source.canonicalize().unwrap();
    let cli = Cli {
        analyze: false,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: Some(temp.path().to_path_buf()),
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.inventory = Some(StorageInventory {
        children: Default::default(),
        volume: None,
        volume_error: None,
        roots: Vec::new(),
        scanned_kb: 8,
        scanned_on_volume_kb: 8,
        unaccounted_kb: 0,
        inventory_overage_kb: 0,
        local_snapshots: Vec::new(),
        scanned_items: 3,
        scan_errors: 0,
        scan_error_paths: Vec::new(),
        complete: true,
        top_level: Vec::new(),
        largest: vec![StorageItem {
            path: source.clone(),
            size_kb: 8,
            kind: StorageItemKind::Directory,
            category: StorageCategory::DeveloperData,
        }],
    });
    app.phase = Phase::Review;
    app.sidebar_focus = false;

    app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
    assert_eq!(app.phase, Phase::RelocationSources);
    assert_eq!(app.relocation_sources[0].path, canonical_source);

    let backend = ratatui::backend::TestBackend::new(200, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &app)).unwrap();
    let rendered =
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .fold(String::new(), |mut text, cell| {
                text.push_str(cell.symbol());
                text
            });
    assert!(rendered.contains("RELOCATE LARGE DATA"));
    assert!(rendered.contains("BuildData"));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.phase, Phase::RelocationDestination);
    assert_eq!(
        app.relocation_source.as_ref().unwrap().path,
        canonical_source
    );

    app.relocation_destination = "/Volumes/EXT_DISK/MacCleanup".into();
    terminal.draw(|frame| render(frame, &app)).unwrap();
    let rendered =
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .fold(String::new(), |mut text, cell| {
                text.push_str(cell.symbol());
                text
            });
    assert!(rendered.contains("EXTERNAL DESTINATION"));
    assert!(rendered.contains("/Volumes/EXT_DISK/MacCleanup"));
}

#[test]
fn selection_only_includes_ready_nonempty_entries() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: true,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.entries = vec![CacheEntry {
        spec: app.specs[0].clone(),
        status: CacheStatus::Ready,
        size_kb: 10,
        outcome: None,
        identity: None,
    }];
    assert!(app.is_selectable(0));
    app.entries[0].size_kb = 0;
    assert!(!app.is_selectable(0));
    app.entries[0].size_kb = 10;
    app.entries[0].status = CacheStatus::InUse;
    assert!(!app.is_selectable(0));
    app.entries[0].status = CacheStatus::Review;
    assert!(!app.is_selectable(0));
}

#[test]
fn location_picker_defaults_to_the_local_startup_volume() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };

    let app = App::new(&cli, temp.path()).unwrap();
    let selected = &app.locations[app.location_cursor];

    assert_eq!(selected.path, Path::new("/"));
    assert_eq!(selected.kind, crate::cache::ScanLocationKind::Local);
    assert_eq!(app.scan_root, Path::new("/"));
}

#[test]
fn default_tui_starts_storage_audit_and_keeps_process_scan_explicit() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: true,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    assert_eq!(app.phase, Phase::Scanning);
    assert!(!app.sidebar_focus);
    assert!(app.retention_worker.is_some());
    assert!(!app.temp_retention.complete);

    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &app)).unwrap();
    let rendered =
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .fold(String::new(), |mut text, cell| {
                text.push_str(cell.symbol());
                text
            });
    assert!(rendered.contains("SCANNING"));
    assert!(rendered.contains("Process health"));
    assert!(rendered.contains("Storage audit"));
    assert!(!rendered.contains("Overview"));

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.phase, Phase::Location);

    app.handle_key(KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.phase, Phase::Processes);
    assert!(app.process_scan_completed);
}

#[test]
fn navigate_menu_is_the_single_cross_section_navigation_model() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: true,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.phase = Phase::Location;

    app.handle_key(KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE));
    assert_eq!(app.menu_open, Some(MenuId::File));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.menu_open, Some(MenuId::View));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.menu_cursor, 1);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.phase, Phase::Processes);
    assert!(app.process_scan_completed);

    app.handle_key(KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.phase, Phase::Location);
    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.show_help);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.show_help);
}

#[test]
fn mouse_operates_menus_and_home_task_choices() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: true,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.phase = Phase::Location;
    app.terminal_width = 120;
    app.terminal_height = 30;
    let left_click = |column, row| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };

    app.handle_mouse(left_click(30, 0));
    assert_eq!(app.menu_open, None);
    app.handle_key(KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE));
    assert_eq!(app.menu_open, Some(MenuId::File));
    app.handle_mouse(left_click(70, 10));
    assert_eq!(app.menu_open, None);

    app.handle_mouse(left_click(40, 0));
    assert_eq!(app.phase, Phase::Processes);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.handle_mouse(left_click(20, 0));
    assert_eq!(app.phase, Phase::Location);
}

#[test]
fn storage_navigation_marks_the_focused_window() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: true,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.phase = Phase::Location;
    app.sidebar_focus = true;
    app.sidebar_cursor = 1;

    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &app)).unwrap();
    let rendered =
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .fold(String::new(), |mut text, cell| {
                text.push_str(cell.symbol());
                text
            });

    assert!(rendered.contains("› Process health"));
    assert!(!rendered.contains("› Storage audit"));
}

#[test]
fn storage_audit_prioritizes_disk_usage_and_adapts_to_terminal_width() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: true,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.inventory = Some(StorageInventory {
        children: Default::default(),
        volume: Some(VolumeStats {
            accounting_path: temp.path().to_path_buf(),
            filesystem: "/dev/test".into(),
            capacity_kb: 250 * 1_048_576,
            used_kb: 247 * 1_048_576,
            free_kb: 3 * 1_048_576,
            container_free_kb: None,
            device: 1,
        }),
        volume_error: None,
        roots: Vec::new(),
        scanned_kb: 247 * 1_048_576,
        scanned_on_volume_kb: 247 * 1_048_576,
        unaccounted_kb: 0,
        inventory_overage_kb: 0,
        local_snapshots: Vec::new(),
        scanned_items: 42,
        scan_errors: 0,
        scan_error_paths: Vec::new(),
        complete: true,
        top_level: Vec::new(),
        largest: vec![StorageItem {
            path: temp.path().join("Library/Developer/BuildData"),
            size_kb: 80 * 1_048_576,
            kind: StorageItemKind::Directory,
            category: StorageCategory::DeveloperData,
        }],
    });
    app.phase = Phase::Review;

    for (width, height) in [(120, 30), (80, 24), (60, 16)] {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered =
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .fold(String::new(), |mut text, cell| {
                    text.push_str(cell.symbol());
                    text
                });

        assert!(rendered.contains("Storage"));
        assert!(rendered.contains("Storage"));
        if height >= 20 {
            assert!(rendered.contains("Where is the space?"));
            assert!(rendered.contains("USED · 99%"));
        }
    }
}

#[test]
fn process_review_is_reachable_and_rendered_from_storage_review() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: true,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.phase = Phase::Review;
    app.sidebar_focus = false;

    app.handle_key(KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.phase, Phase::Processes);
    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &app)).unwrap();
    let rendered =
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .fold(String::new(), |mut text, cell| {
                text.push_str(cell.symbol());
                text
            });
    assert!(rendered.contains("LIVE PROCESSES"));
    assert!(rendered.contains("READ ONLY"));
}

#[test]
fn completed_scan_hides_missing_candidates() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: true,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.choose_location();
    assert!(app.retention_worker.is_some());

    while !app.scan_work_complete {
        app.scan_next();
    }
    app.scan_progress_ratio = 0.995;
    app.advance_scan_progress();

    assert_eq!(app.phase, Phase::Review);
    assert_eq!(app.scan_progress_ratio, 1.0);
    assert!(app.entries.is_empty());
}

#[test]
fn completed_summary_closes_after_the_grace_period() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: true,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.phase = Phase::Summary;
    app.summary_started_at = Some(Instant::now());

    assert!(!app.summary_has_timed_out());

    app.summary_started_at = Some(
        Instant::now()
            .checked_sub(SUMMARY_AUTO_CLOSE_AFTER + Duration::from_millis(1))
            .unwrap(),
    );
    assert!(app.summary_has_timed_out());
}

#[test]
fn completed_summary_can_be_closed_immediately() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: true,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.phase = Phase::Summary;
    app.summary_started_at = Some(Instant::now());

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.quit);
}

#[test]
fn directory_scan_advances_in_bounded_visible_slices() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    fs::create_dir(&cache).unwrap();
    for index in 0..300 {
        fs::write(cache.join(format!("item-{index}")), [1_u8; 4_096]).unwrap();
    }
    let spec = CacheSpec {
        home: temp.path().to_path_buf(),
        path: cache,
        label: "Test cache",
        tier: crate::cache::CacheTier::Routine,
        process_pattern: "",
        note: "test data",
        target: crate::cache::CacheTarget::DirectoryContents,
    };
    let mut scan = DirectoryScan::new(spec, CacheStatus::Ready);

    let finished = scan.advance();

    assert!(!finished);
    assert!((1..=256).contains(&scan.inspected_items));
    assert!(scan.size_kb() > 0);
    while !scan.advance() {}
    assert_eq!(scan.inspected_items, 300);
}

#[test]
fn incomplete_directory_scan_is_not_marked_ready() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let unreadable = cache.join("unreadable");
    fs::create_dir(&cache).unwrap();
    fs::create_dir(&unreadable).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
    let spec = CacheSpec {
        home: temp.path().to_path_buf(),
        path: cache,
        label: "Test cache",
        tier: crate::cache::CacheTier::Routine,
        process_pattern: "",
        note: "test data",
        target: crate::cache::CacheTarget::DirectoryContents,
    };
    let mut scan = DirectoryScan::new(spec, CacheStatus::Ready);

    while !scan.advance() {}
    let entry = scan.into_entry();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(entry.status, CacheStatus::ScanError);
}

#[test]
fn active_scan_progress_moves_smoothly_without_claiming_completion() {
    let initial = smooth_scan_ratio(0, 7, Some(Duration::ZERO));
    let completed_boundary = smooth_scan_ratio(1, 7, None);
    let just_started = smooth_scan_ratio(1, 7, Some(Duration::ZERO));
    let after_ten_seconds = smooth_scan_ratio(1, 7, Some(Duration::from_secs(10)));
    let after_two_minutes = smooth_scan_ratio(1, 7, Some(Duration::from_secs(120)));
    let next_boundary = 2.0 / 7.0;

    assert_eq!(initial, 0.0);
    assert_eq!(completed_boundary, 1.0 / 7.0);
    assert_eq!(just_started, completed_boundary);
    assert!(after_ten_seconds > just_started);
    assert!(after_two_minutes > after_ten_seconds);
    assert!(after_two_minutes < next_boundary);
    assert_eq!(smooth_scan_ratio(7, 7, None), 1.0);
}

#[test]
fn rendered_scan_progress_eases_forward_without_jumping() {
    let frame = Duration::from_millis(16);
    let first = animate_scan_ratio(0.0, 0.5, frame);
    let second = animate_scan_ratio(first, 0.5, frame);

    assert_eq!(animate_scan_ratio(0.0, 0.5, Duration::ZERO), 0.0);
    assert!(first > 0.0);
    assert!(first <= 0.8 * frame.as_secs_f64());
    assert!(second > first);
    assert!(second < 0.5);
    assert_eq!(animate_scan_ratio(0.5, 0.25, frame), 0.5);
}

#[test]
fn scan_activity_spinner_advances_independently_of_category_completion() {
    assert_ne!(
        scan_spinner(Duration::ZERO),
        scan_spinner(Duration::from_millis(80))
    );
}

#[test]
fn escape_cancels_scan_and_returns_to_location_picker() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: true,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: Some(temp.path().to_path_buf()),
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.entries.push(CacheEntry {
        spec: app.specs[0].clone(),
        status: CacheStatus::Ready,
        size_kb: 10,
        outcome: None,
        identity: None,
    });

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert_eq!(app.phase, Phase::Location);
    assert!(!app.quit);
    assert!(app.entries.is_empty());
    assert!(app.scan_task.is_none());
}

#[test]
fn enter_opens_and_closes_details_without_quitting_analyze_mode() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: true,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.entries = vec![CacheEntry {
        spec: app.specs[0].clone(),
        status: CacheStatus::Ready,
        size_kb: 10,
        outcome: None,
        identity: None,
    }];
    app.phase = Phase::Review;
    app.sidebar_focus = false;

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.phase, Phase::Details);
    assert!(!app.quit);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.phase, Phase::Review);
    assert!(!app.quit);
}

#[test]
fn enter_still_opens_confirmation_when_cleanup_items_are_selected() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: true,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.entries = vec![CacheEntry {
        spec: app.specs[0].clone(),
        status: CacheStatus::Ready,
        size_kb: 10,
        outcome: None,
        identity: None,
    }];
    app.selected.insert(0);
    app.phase = Phase::Review;
    app.sidebar_focus = false;

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.phase, Phase::Confirm);
    assert!(!app.quit);
}

#[test]
fn default_tui_can_prepare_all_safe_items_without_a_restart() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.entries = vec![
        CacheEntry {
            spec: app.specs[0].clone(),
            status: CacheStatus::Ready,
            size_kb: 10,
            outcome: None,
            identity: None,
        },
        CacheEntry {
            spec: app.specs[1].clone(),
            status: CacheStatus::InUse,
            size_kb: 20,
            outcome: None,
            identity: None,
        },
        CacheEntry {
            spec: app.specs[2].clone(),
            status: CacheStatus::Review,
            size_kb: 30,
            outcome: None,
            identity: None,
        },
    ];
    app.phase = Phase::Details;

    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));

    assert_eq!(app.mode, Mode::Clean);
    assert_eq!(app.phase, Phase::Confirm);
    assert_eq!(app.selected, BTreeSet::from([0]));
}

#[test]
fn default_tui_can_prepare_the_highlighted_safe_item_from_review() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.entries = vec![
        CacheEntry {
            spec: app.specs[0].clone(),
            status: CacheStatus::Ready,
            size_kb: 10,
            outcome: None,
            identity: None,
        },
        CacheEntry {
            spec: app.specs[1].clone(),
            status: CacheStatus::Ready,
            size_kb: 20,
            outcome: None,
            identity: None,
        },
    ];
    app.cursor = 1;
    app.selected.insert(0);
    app.phase = Phase::Review;
    app.sidebar_focus = false;

    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert_eq!(app.mode, Mode::Clean);
    assert_eq!(app.phase, Phase::Confirm);
    assert_eq!(app.selected, BTreeSet::from([1]));
}

#[test]
fn direct_deletion_can_opt_in_one_optional_item() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.entries = vec![CacheEntry {
        spec: app.specs[0].clone(),
        status: CacheStatus::Optional,
        size_kb: 10,
        outcome: None,
        identity: None,
    }];
    app.phase = Phase::Details;

    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert_eq!(app.mode, Mode::Clean);
    assert_eq!(app.phase, Phase::Confirm);
    assert_eq!(app.selected, BTreeSet::from([0]));

    let backend = ratatui::backend::TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &app)).unwrap();
    let rendered =
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .fold(String::new(), |mut text, cell| {
                text.push_str(cell.symbol());
                text
            });
    assert!(rendered.contains("DELETE REINSTALLABLE CACHE"));
    assert!(rendered.contains("opts in only the highlighted item"));
}

#[test]
fn direct_deletion_ignores_items_blocked_by_safety_checks() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.entries = vec![CacheEntry {
        spec: app.specs[0].clone(),
        status: CacheStatus::InUse,
        size_kb: 10,
        outcome: None,
        identity: None,
    }];
    app.phase = Phase::Details;

    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert_eq!(app.mode, Mode::Analyze);
    assert_eq!(app.phase, Phase::Details);
    assert!(app.selected.is_empty());
}

#[test]
fn stop_request_keeps_the_ui_open_until_the_active_worker_finishes() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: true,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    let (_sender, receiver) = mpsc::channel();
    let stop_requested = Arc::new(AtomicBool::new(false));
    app.cleanup_worker = Some(CleanupWorker {
        receiver,
        stop_requested: Arc::clone(&stop_requested),
    });
    app.phase = Phase::Cleaning;

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert_eq!(app.phase, Phase::Cleaning);
    assert!(app.stopped_early);
    assert!(stop_requested.load(Ordering::Relaxed));
}

#[test]
fn explicit_analyze_mode_keeps_safe_cleanup_locked() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: true,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    app.entries = vec![CacheEntry {
        spec: app.specs[0].clone(),
        status: CacheStatus::Ready,
        size_kb: 10,
        outcome: None,
        identity: None,
    }];
    app.phase = Phase::Review;
    app.sidebar_focus = false;

    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));

    assert_eq!(app.mode, Mode::Analyze);
    assert_eq!(app.phase, Phase::Review);
    assert!(app.selected.is_empty());
}

#[test]
fn default_tui_shows_and_accepts_review_deletion_shortcut() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    let review_spec = app
        .specs
        .iter()
        .find(|spec| spec.label == "Cursor user data")
        .unwrap()
        .clone();
    app.entries = vec![CacheEntry {
        spec: review_spec,
        status: CacheStatus::Review,
        size_kb: 10,
        outcome: None,
        identity: None,
    }];
    app.phase = Phase::Details;
    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(frame, &app)).unwrap();
    let rendered =
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .fold(String::new(), |mut text, cell| {
                text.push_str(cell.symbol());
                text
            });
    assert!(rendered.contains("d advanced delete"));
    assert!(rendered.contains("c clean all safe"));
    assert!(rendered.contains("o reveal in Finder"));

    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert_eq!(app.mode, Mode::Clean);
    assert_eq!(app.phase, Phase::ReviewConfirm);
    assert_eq!(app.review_target, Some(0));
}

#[test]
fn review_details_explain_the_orbstack_deletion_decision() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    let review_spec = app
        .specs
        .iter()
        .find(|spec| spec.label == "OrbStack data")
        .unwrap()
        .clone();
    app.entries = vec![CacheEntry {
        spec: review_spec,
        status: CacheStatus::Review,
        size_kb: 12_165_120,
        outcome: None,
        identity: None,
    }];
    app.phase = Phase::Details;
    let backend = ratatui::backend::TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(frame, &app)).unwrap();
    let rendered =
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .fold(String::new(), |mut text, cell| {
                text.push_str(cell.symbol());
                text
            });

    assert!(rendered.contains("CONTAINER AND VM DATA — NOT A CACHE"));
    assert!(rendered.contains("containers, images, Linux machines, and volumes"));
    assert!(rendered.contains("volume data are not automatically recoverable"));
    assert!(rendered.contains("Open OrbStack and remove unused items individually"));
    assert!(rendered.contains("orbctl reset --yes"));
    assert!(rendered.contains("typing DELETE"));
}

#[test]
fn review_deletion_requires_clean_mode_and_typed_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: true,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    let review_spec = app
        .specs
        .iter()
        .find(|spec| spec.label == "Cursor user data")
        .unwrap()
        .clone();
    app.entries = vec![CacheEntry {
        spec: review_spec,
        status: CacheStatus::Review,
        size_kb: 10,
        outcome: None,
        identity: None,
    }];
    app.phase = Phase::Review;
    app.sidebar_focus = false;

    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert_eq!(app.phase, Phase::ReviewConfirm);
    assert_eq!(app.review_target, Some(0));

    for character in "delete".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.phase, Phase::Cleaning);
    assert_eq!(app.cleanup_kind, CleanupKind::ReviewData);
    assert_eq!(app.cleanup_queue, vec![0]);
}

#[test]
fn review_confirmation_prompt_is_visible_in_a_standard_terminal() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: false,
        clean: true,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    let review_spec = app
        .specs
        .iter()
        .find(|spec| spec.label == "Cursor user data")
        .unwrap()
        .clone();
    app.entries = vec![CacheEntry {
        spec: review_spec,
        status: CacheStatus::Review,
        size_kb: 10,
        outcome: None,
        identity: None,
    }];
    app.phase = Phase::Review;
    app.sidebar_focus = false;
    app.start_review_confirmation();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| render(frame, &app)).unwrap();

    let rendered =
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .fold(String::new(), |mut text, cell| {
                text.push_str(cell.symbol());
                text
            });
    assert!(rendered.contains("ADVANCED REVIEW DELETION"));
    assert!(rendered.contains("Type DELETE"));
    assert!(rendered.contains("> _"));
    assert!(rendered.contains("Esc cancels"));
}

#[test]
fn review_deletion_is_unavailable_in_analyze_mode() {
    let temp = tempfile::tempdir().unwrap();
    let cli = Cli {
        analyze: true,
        clean: false,
        include_reinstallable: false,
        tmp_retention_days: 7,
        volume: None,
        relocate: None,
        relocate_to: None,
        yes: false,
        verbose: false,
        json: false,
        no_color: true,
        no_tui: false,
    };
    let mut app = App::new(&cli, temp.path()).unwrap();
    let review_spec = app
        .specs
        .iter()
        .find(|spec| spec.label == "Cursor user data")
        .unwrap()
        .clone();
    app.entries = vec![CacheEntry {
        spec: review_spec,
        status: CacheStatus::Review,
        size_kb: 10,
        outcome: None,
        identity: None,
    }];
    app.phase = Phase::Review;
    app.sidebar_focus = false;

    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert_eq!(app.phase, Phase::Review);
    assert_eq!(app.review_target, None);
}

fn design_fixture() -> App {
    use clap::Parser;
    let cli = Cli::parse_from(["mac-cleanup"]);
    let mut app = App::new(&cli, Path::new("/Users/demo")).unwrap();
    app.no_color = false;
    app.sidebar_focus = false;
    app.scan_root = PathBuf::from("/");
    app.entries = [
        (
            "Xcode derived data",
            "Library/Developer/Xcode/DerivedData",
            CacheStatus::Ready,
            8_388_608,
            "Build products and indexes. Xcode rebuilds them when needed.",
        ),
        (
            "Homebrew",
            "Library/Caches/Homebrew",
            CacheStatus::Ready,
            2_097_152,
            "Downloaded packages. Installed applications remain.",
        ),
        (
            "Playwright browsers",
            "Library/Caches/ms-playwright",
            CacheStatus::Optional,
            1_572_864,
            "Browser binaries are downloaded again when needed.",
        ),
        (
            "npm packages",
            ".npm/_cacache",
            CacheStatus::Ready,
            786_432,
            "Package cache. Projects and installed dependencies remain.",
        ),
        (
            "Cursor user data",
            "Library/Application Support/Cursor",
            CacheStatus::Review,
            655_360,
            "Settings, extensions, local history, and workspace data.",
        ),
        (
            "Chrome cache",
            "Library/Caches/Google/Chrome",
            CacheStatus::InUse,
            262_144,
            "Close Chrome before clearing cached content.",
        ),
    ]
    .into_iter()
    .map(|(name, path, status, size, note)| CacheEntry {
        spec: CacheSpec {
            home: PathBuf::from("/Users/demo"),
            path: Path::new("/Users/demo").join(path),
            label: name,
            tier: if status == CacheStatus::Review {
                CacheTier::ReviewOnly
            } else if status == CacheStatus::Optional {
                CacheTier::Reinstallable
            } else {
                CacheTier::Routine
            },
            process_pattern: "",
            note,
            target: CacheTarget::DirectoryContents,
        },
        status,
        size_kb: size,
        outcome: None,
        identity: None,
    })
    .collect();
    app.inventory = Some(StorageInventory {
        children: Default::default(),
        volume: Some(VolumeStats {
            accounting_path: PathBuf::from("/"),
            filesystem: "/dev/disk3s1".into(),
            capacity_kb: 512 * 1_048_576,
            used_kb: 386 * 1_048_576,
            free_kb: 126 * 1_048_576,
            container_free_kb: None,
            device: 1,
        }),
        volume_error: None,
        roots: Vec::new(),
        scanned_kb: 380 * 1_048_576,
        scanned_on_volume_kb: 380 * 1_048_576,
        unaccounted_kb: 6 * 1_048_576,
        inventory_overage_kb: 0,
        local_snapshots: Vec::new(),
        scanned_items: 148_206,
        scan_errors: 0,
        scan_error_paths: Vec::new(),
        complete: true,
        top_level: Vec::new(),
        largest: [
            ("/Users/demo/Library", 142),
            ("/Users/demo/Projects", 96),
            ("/Applications", 48),
            ("/Users/demo/Pictures", 32),
        ]
        .into_iter()
        .map(|(path, size)| StorageItem {
            path: PathBuf::from(path),
            size_kb: size * 1_048_576,
            kind: StorageItemKind::Directory,
            category: StorageCategory::ApplicationData,
        })
        .collect(),
    });
    let inventory = app.inventory.as_mut().unwrap();
    inventory.roots.push(StorageRoot {
        path: PathBuf::from("/"),
        size_kb: inventory.scanned_kb,
        device: 1,
        scan_errors: 0,
    });
    inventory
        .children
        .insert(PathBuf::from("/"), inventory.largest.clone());
    inventory.children.insert(
        PathBuf::from("/Users/demo/Library"),
        app.entries
            .iter()
            .map(|entry| StorageItem {
                path: entry.spec.path.clone(),
                size_kb: entry.size_kb,
                kind: StorageItemKind::Directory,
                category: StorageCategory::ApplicationData,
            })
            .collect(),
    );
    app.locations = vec![
        ScanLocation {
            label: "Macintosh HD".into(),
            path: PathBuf::from("/"),
            kind: ScanLocationKind::Local,
        },
        ScanLocation {
            label: "Home folder".into(),
            path: PathBuf::from("/Users/demo"),
            kind: ScanLocationKind::Local,
        },
        ScanLocation {
            label: "Studio SSD".into(),
            path: PathBuf::from("/Volumes/Studio SSD"),
            kind: ScanLocationKind::Usb,
        },
    ];
    app
}

fn draw_fixture(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    app.terminal_width = width;
    app.terminal_height = height;
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| render(frame, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
    buffer
        .content()
        .chunks(buffer.area.width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn redesigned_screens_fit_supported_sizes_and_export_optional_previews() {
    let mut app = design_fixture();
    for (name, phase) in [
        ("locations", Phase::Location),
        ("storage", Phase::Review),
        ("details", Phase::Details),
        ("confirm", Phase::Confirm),
        ("advanced", Phase::ReviewConfirm),
        ("scanning", Phase::Scanning),
        ("summary", Phase::Summary),
    ] {
        app.phase = phase;
        app.review_target = Some(4);
        app.selected = BTreeSet::from([0, 1, 3]);
        app.stats.measured_removed_kb = 10_485_760;
        app.stats.cleared = 3;
        for (width, height) in [(60, 16), (80, 24), (120, 30), (160, 40)] {
            let buffer = draw_fixture(&mut app, width, height);
            let text = buffer_text(&buffer);
            if phase == Phase::Confirm {
                assert!(text.contains("y delete permanently"));
                assert!(text.contains("Esc cancel"));
            }
            if phase == Phase::ReviewConfirm {
                assert!(text.contains("Type DELETE, then press Enter:"));
                assert!(text.contains("Esc cancels"));
            }
            if let Some(directory) = std::env::var_os("MAC_CLEANUP_RENDER_DIR") {
                let directory = PathBuf::from(directory);
                fs::create_dir_all(&directory).unwrap();
                fs::write(directory.join(format!("{name}-{width}.txt")), &text).unwrap();
                fs::write(
                    directory.join(format!("{name}-{width}.svg")),
                    buffer_svg(&buffer),
                )
                .unwrap();
            }
        }
    }
}

#[test]
fn confirmations_block_navigation_and_background_clicks() {
    let mut app = design_fixture();
    for phase in [
        Phase::Confirm,
        Phase::ReviewConfirm,
        Phase::ProcessConfirm,
        Phase::RelocationConfirm,
        Phase::Details,
    ] {
        app.phase = phase;
        draw_fixture(&mut app, 120, 30);
        app.handle_key(KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE));
        assert!(app.menu_open.is_none());
        app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 9,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.phase, phase);
    }
    app.phase = Phase::Location;
    app.show_help = true;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 30,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.menu_open.is_none());
}

#[test]
fn tiny_terminal_cannot_confirm_an_invisible_action() {
    let mut app = design_fixture();
    app.phase = Phase::Confirm;
    app.selected.insert(0);
    let buffer = draw_fixture(&mut app, 45, 12);
    assert!(buffer_text(&buffer).contains("Expand the terminal"));
    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert_eq!(app.phase, Phase::Confirm);
    assert!(app.cleanup_worker.is_none());
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.phase, Phase::Review);
}

#[test]
fn selection_and_pointer_rows_follow_the_visible_workspace() {
    let mut app = design_fixture();
    app.phase = Phase::Review;
    app.sidebar_focus = false;
    app.handle_review_key(KeyCode::Char('f'));
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert_eq!(app.selected, BTreeSet::from([0]));
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert_eq!(app.selected, BTreeSet::from([0, 1, 3]));
    draw_fixture(&mut app, 160, 40);
    let (area, _) = *app
        .hit_regions
        .borrow()
        .iter()
        .find(|(_, target)| matches!(target, HitTarget::Finding(3)))
        .unwrap();
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: area.x + 2,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.cursor, 3);
    app.analysis_only = true;
    app.mode = Mode::Analyze;
    app.selected.clear();
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(app.selected.is_empty());
}

#[test]
fn exact_paths_scroll_in_confirmations_and_no_color_is_respected() {
    let mut app = design_fixture();
    app.phase = Phase::Confirm;
    app.selected = BTreeSet::from([0, 1, 3]);
    app.no_color = true;
    let first = draw_fixture(&mut app, 80, 16);
    let before = buffer_text(&first);
    assert!(before.contains("/Users/demo/Library/Developer/Xcode/DerivedData"));
    assert!(
        first
            .content()
            .iter()
            .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
    );
    for _ in 0..20 {
        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    }
    let last = draw_fixture(&mut app, 80, 16);
    assert!(buffer_text(&last).contains("/Users/demo/.npm/_cacache"));
    assert!(buffer_text(&last).contains("y delete permanently"));
    assert_ne!(before, buffer_text(&last));
}

pub(super) fn buffer_svg(buffer: &ratatui::buffer::Buffer) -> String {
    let rgb = |color: Color| match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Black => "#161817".into(),
        _ => "#e9e8de".into(),
    };
    let width = buffer.area.width as usize * 9 + 32;
    let height = buffer.area.height as usize * 19 + 32;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\"><rect width=\"100%\" height=\"100%\" fill=\"#161817\"/><g font-family=\"Menlo, monospace\" font-size=\"14\">"
    );
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            let left = x as usize * 9 + 16;
            let top = y as usize * 19 + 16;
            svg.push_str(&format!(
                "<rect x=\"{left}\" y=\"{top}\" width=\"9\" height=\"19\" fill=\"{}\"/>",
                rgb(cell.bg)
            ));
            if cell.symbol() != " " {
                let escaped = cell
                    .symbol()
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                svg.push_str(&format!(
                    "<text x=\"{left}\" y=\"{}\" fill=\"{}\" font-weight=\"{}\">{escaped}</text>",
                    top + 14,
                    rgb(cell.fg),
                    if cell.modifier.contains(Modifier::BOLD) {
                        "bold"
                    } else {
                        "normal"
                    }
                ));
            }
        }
    }
    svg.push_str("</g></svg>");
    svg
}

#[test]
fn explorer_drills_to_files_restores_selection_and_never_selects_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("volume");
    fs::create_dir_all(root.join("Large/nested")).unwrap();
    let root = root.canonicalize().unwrap();
    fs::write(root.join("Large/nested/payload.bin"), vec![1; 32768]).unwrap();
    fs::write(root.join("small.txt"), vec![2; 4096]).unwrap();
    let mut app = design_fixture();
    app.inventory = Some(StorageInventory::scan(&root, &root));
    app.phase = Phase::Review;
    app.sidebar_focus = false;
    app.storage_tab = StorageTab::Explore;
    app.selected.clear();
    assert_eq!(app.explorer_items()[0].path, root.join("Large"));
    for key in [' ', 'a', 'd', 'c'] {
        app.handle_review_key(KeyCode::Char(key));
    }
    assert!(app.selected.is_empty());
    assert_eq!(app.phase, Phase::Review);
    app.handle_review_key(KeyCode::Enter);
    assert_eq!(
        app.explorer_path.as_deref(),
        Some(root.join("Large").as_path())
    );
    assert_eq!(app.explorer_items().len(), 1);
    app.handle_review_key(KeyCode::Right);
    assert_eq!(
        app.explorer_items()[0].path,
        root.join("Large/nested/payload.bin")
    );
    app.handle_review_key(KeyCode::Enter);
    assert!(app.explorer_details);
    let text = buffer_text(&draw_fixture(&mut app, 80, 24));
    assert!(text.contains("payload.bin"));
    app.handle_review_key(KeyCode::End);
    let text = buffer_text(&draw_fixture(&mut app, 80, 24));
    assert!(text.contains("Return to parent"));
    app.handle_review_key(KeyCode::Esc);
    assert!(!app.explorer_details);
    app.handle_review_key(KeyCode::Backspace);
    app.handle_review_key(KeyCode::Left);
    assert!(app.explorer_path.is_none());
    assert_eq!(app.explorer_cursor, 0);
    assert!(root.join("Large/nested/payload.bin").exists());
}

#[test]
fn storage_tabs_mouse_and_coverage_work_at_supported_sizes() {
    let mut app = design_fixture();
    app.phase = Phase::Review;
    app.sidebar_focus = false;
    let inventory = app.inventory.as_mut().unwrap();
    inventory.complete = false;
    inventory.scan_errors = 141;
    inventory.scan_error_paths = (0..20)
        .map(|index| {
            PathBuf::from(format!(
                "/Users/demo/Library/Protected area {index}/unreadable data"
            ))
        })
        .collect();
    inventory.local_snapshots = vec!["2026-09-01-101530".into()];
    for (width, height) in [(60, 16), (80, 24), (120, 30), (160, 40)] {
        for (name, tab) in [
            ("explore", StorageTab::Explore),
            ("heatmap", StorageTab::Heatmap),
            ("decisions", StorageTab::Decisions),
            ("coverage", StorageTab::Coverage),
        ] {
            app.switch_storage_tab(tab);
            let buffer = draw_fixture(&mut app, width, height);
            let text = buffer_text(&buffer);
            assert!(text.contains("MAC CLEANUP"));
            assert!(text.contains("e Explore"));
            assert!(text.contains("h Heatmap") || text.contains("h Storage heatmap"));
            assert!(text.contains("f Cleanup"));
            assert!(text.contains("v Coverage") || text.contains("v Scan coverage"));
            if tab == StorageTab::Explore {
                assert!(text.contains("FOLDER / FILE"));
            }
            if tab == StorageTab::Heatmap {
                assert!(text.contains("STORAGE MAP"));
            }
            if tab == StorageTab::Coverage {
                assert!(text.contains("Partial scan"));
                for _ in 0..100 {
                    app.handle_review_key(KeyCode::PageDown);
                }
                let text = buffer_text(&draw_fixture(&mut app, width, height));
                assert!(text.contains("Scanned locations"));
                app.coverage_scroll = 0;
            }
            if let Some(directory) = std::env::var_os("MAC_CLEANUP_RENDER_DIR") {
                let directory = PathBuf::from(directory);
                fs::create_dir_all(&directory).unwrap();
                fs::write(
                    directory.join(format!("{name}-{width}.svg")),
                    buffer_svg(&buffer),
                )
                .unwrap();
                fs::write(directory.join(format!("{name}-{width}.txt")), &text).unwrap();
            }
        }
    }
    app.switch_storage_tab(StorageTab::Explore);
    draw_fixture(&mut app, 160, 40);
    let (area, _) = *app
        .hit_regions
        .borrow()
        .iter()
        .find(|(_, target)| matches!(target, HitTarget::Consumer(1)))
        .unwrap();
    app.handle_left_click(area.x + 2, area.y);
    assert_eq!(app.explorer_cursor, 1);
    app.handle_left_click(area.x + 2, area.y);
    assert_eq!(
        app.explorer_path.as_deref(),
        Some(Path::new("/Users/demo/Projects"))
    );
    app.handle_review_key(KeyCode::Left);
    assert_eq!(app.explorer_cursor, 1);
    app.handle_review_key(KeyCode::Char(']'));
    assert_eq!(app.storage_tab, StorageTab::Heatmap);
    app.handle_review_key(KeyCode::Char(']'));
    assert_eq!(app.storage_tab, StorageTab::Decisions);
    app.handle_review_key(KeyCode::Char('['));
    assert_eq!(app.storage_tab, StorageTab::Heatmap);
    app.handle_review_key(KeyCode::Char('h'));
    assert_eq!(app.storage_tab, StorageTab::Heatmap);
    app.explorer_cursor = 0;
    draw_fixture(&mut app, 160, 40);
    let (area, index) = app
        .hit_regions
        .borrow()
        .iter()
        .find_map(|(area, target)| {
            if let HitTarget::MapNode(index) = target {
                Some((*area, *index))
            } else {
                None
            }
        })
        .unwrap();
    let path = app.map_paths.borrow()[index].clone();
    app.handle_left_click(area.x, area.y);
    assert_eq!(app.explorer_path.as_deref(), Some(path.as_path()));
    assert!(app.selected.is_empty());
    app.handle_review_key(KeyCode::Left);
    assert!(app.explorer_path.is_none());
}

#[test]
fn treemap_geometry_covers_area_without_overlaps_or_inflating_zero_bytes() {
    use super::folder_map::map_rects;
    for (width, height) in [(1, 1), (3, 2), (70, 24)] {
        for weights in [
            vec![40, 35, 25],
            vec![99, 1, 0],
            vec![u64::MAX, u64::MAX, 1],
        ] {
            let area = Rect::new(5, 7, width, height);
            let rects = map_rects(&weights, area);
            let mut cells = std::collections::HashSet::new();
            for (weight, rect) in weights.iter().zip(rects) {
                if *weight == 0 {
                    assert!(rect.is_empty());
                }
                for y in rect.y..rect.bottom() {
                    for x in rect.x..rect.right() {
                        assert!(area.contains((x, y).into()));
                        assert!(cells.insert((x, y)), "overlapping rectangles");
                    }
                }
            }
            assert_eq!(cells.len(), usize::from(width) * usize::from(height));
        }
    }
    assert!(map_rects(&[0], Rect::new(0, 0, 10, 10))[0].is_empty());
    let rects = map_rects(&[75, 25], Rect::new(0, 0, 80, 20));
    assert_eq!(rects[0].area(), 1200);
    assert_eq!(rects[1].area(), 400);
}

#[test]
fn explorer_links_exact_cleanup_rule_without_changing_selection() {
    let mut app = design_fixture();
    app.phase = Phase::Review;
    app.sidebar_focus = false;
    app.open_consumer();
    app.explorer_cursor = 2;
    let path = app.explorer_items()[2].path.clone();
    app.handle_review_key(KeyCode::Char('f'));
    assert_eq!(app.storage_tab, StorageTab::Decisions);
    assert_eq!(app.entries[app.cursor].spec.path, path);
    assert!(app.selected.is_empty());
    assert_eq!(app.phase, Phase::Review);
}

#[test]
fn top_menu_is_persistent_and_keyboard_focus_returns_to_content() {
    let mut app = design_fixture();
    app.inventory = None;
    app.entries.clear();
    app.phase = Phase::Location;
    app.sidebar_focus = true;
    for (width, height) in [(60, 16), (80, 24), (160, 40), (280, 80)] {
        let buffer = draw_fixture(&mut app, width, height);
        let page = content_area(buffer.area);
        assert_eq!(page.x, 0);
        assert_eq!(page.y, TOP_BAR_HEIGHT);
        let text = buffer_text(&buffer);
        assert!(text.contains("AVAILABLE VOLUMES"));
        assert!(!text.contains("YOUR NEXT STEP"));
        assert!(!text.contains("Enter to open"));
        assert!(!text.contains("WORKSPACE"));
        for section in 0..3 {
            let nav = top_menu_item_area(buffer.area, section);
            assert_eq!(navigation_at(nav.x + 1, 0, width), Some(section));
        }
        if let Some(directory) = std::env::var_os("MAC_CLEANUP_RENDER_DIR") {
            let directory = PathBuf::from(directory);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join(format!("welcome-{width}.txt")), &text).unwrap();
            fs::write(
                directory.join(format!("welcome-{width}.svg")),
                buffer_svg(&buffer),
            )
            .unwrap();
        }
    }
    app.phase = Phase::Location;
    app.sidebar_focus = false;
    app.handle_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE));
    assert!(app.sidebar_focus);
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.phase, Phase::Location);
    assert!(!app.sidebar_focus);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.phase, Phase::Processes);
}

#[test]
fn sidebar_focus_is_exclusive_and_tab_preserves_storage_state() {
    let mut app = design_fixture();
    app.phase = Phase::Review;
    app.storage_tab = StorageTab::Decisions;
    app.cursor = 1;
    app.selected.insert(0);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(app.sidebar_focus);
    assert_eq!(app.sidebar_cursor, 0);
    for key in ['c', 'd', 'a', ' ', 'm', 'f', 'e', 'r'] {
        app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        assert_eq!(app.phase, Phase::Review);
        assert_eq!(app.selected, BTreeSet::from([0]));
        assert_eq!(app.cursor, 1);
        assert_eq!(app.storage_tab, StorageTab::Decisions);
        assert!(app.cleanup_worker.is_none());
    }
    app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(app.sidebar_cursor, 0);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.sidebar_cursor, 1);
    app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert!(!app.sidebar_focus);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.cursor, 2);
    app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
    assert_eq!(app.storage_tab, StorageTab::Coverage);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.phase, Phase::Review);
    assert!(!app.sidebar_focus);
    assert_eq!(app.selected, BTreeSet::from([0]));
}

#[test]
fn mouse_focus_and_read_only_menu_items_follow_the_visible_pane() {
    let mut app = design_fixture();
    app.phase = Phase::Review;
    app.storage_tab = StorageTab::Decisions;
    app.cursor = 1;
    draw_fixture(&mut app, 120, 30);
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 3,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.sidebar_focus);
    assert_eq!(app.sidebar_cursor, 1);
    assert_eq!(app.cursor, 1);
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 40,
        row: 15,
        modifiers: KeyModifiers::NONE,
    });
    assert!(!app.sidebar_focus);
    assert_eq!(app.cursor, 4);
    app.analysis_only = true;
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    assert_eq!(app.sidebar_cursor, 1);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.sidebar_cursor, 1);
    app.handle_left_click(60, 0);
    assert_eq!(app.phase, Phase::Review);
    assert!(app.status_message.as_ref().unwrap().contains("read-only"));
}

#[test]
fn modal_decisions_keep_focus_and_state_when_tab_is_pressed() {
    let mut app = design_fixture();
    for phase in [
        Phase::Confirm,
        Phase::ReviewConfirm,
        Phase::Details,
        Phase::ProcessConfirm,
        Phase::RelocationConfirm,
    ] {
        app.phase = phase;
        app.sidebar_focus = false;
        for code in [KeyCode::Tab, KeyCode::BackTab, KeyCode::F(8)] {
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
            assert_eq!(app.phase, phase);
            assert!(!app.sidebar_focus);
        }
    }
}

#[test]
fn nested_folder_map_opens_exact_paths_and_preserves_review_selection() {
    let mut app = design_fixture();
    app.phase = Phase::Review;
    app.storage_tab = StorageTab::Explore;
    app.selected.insert(0);
    app.entries.truncate(2);
    for (entry, gib) in app.entries.iter_mut().zip([18, 14]) {
        entry.spec.path = Path::new("/").join(entry.spec.path.strip_prefix("/Users/demo").unwrap());
        entry.size_kb = gib * 1_048_576;
    }
    let inventory = app.inventory.as_mut().unwrap();
    inventory.scanned_kb = 148 * 1_048_576;
    inventory.scanned_on_volume_kb = inventory.scanned_kb;
    inventory.roots[0].size_kb = inventory.scanned_kb;
    let volume = inventory.volume.as_mut().unwrap();
    volume.used_kb = 154 * 1_048_576;
    volume.free_kb = 358 * 1_048_576;
    inventory.children.clear();
    let folder = |path: &str, gib: u64| StorageItem {
        path: PathBuf::from(path),
        size_kb: gib * 1_048_576,
        kind: StorageItemKind::Directory,
        category: StorageCategory::ApplicationData,
    };
    inventory.children.insert(
        PathBuf::from("/"),
        vec![folder("/Library", 100), folder("/Applications", 48)],
    );
    for (parent, children) in [
        (
            "/Library",
            vec![
                ("/Library/Developer", 52),
                ("/Library/Caches", 26),
                ("/Library/Application Support", 18),
                ("/Library/Logs", 4),
            ],
        ),
        (
            "/Library/Developer",
            vec![
                ("/Library/Developer/Xcode", 34),
                ("/Library/Developer/CoreSimulator", 18),
            ],
        ),
        (
            "/Library/Developer/Xcode",
            vec![
                ("/Library/Developer/Xcode/DerivedData", 18),
                ("/Library/Developer/Xcode/DeviceSupport", 16),
            ],
        ),
        (
            "/Library/Caches",
            vec![
                ("/Library/Caches/Homebrew", 14),
                ("/Library/Caches/Browser", 8),
            ],
        ),
    ] {
        inventory.children.insert(
            PathBuf::from(parent),
            children
                .into_iter()
                .map(|(path, size)| folder(path, size))
                .collect(),
        );
    }
    inventory
        .children
        .get_mut(Path::new("/Library/Caches"))
        .unwrap()
        .push(StorageItem {
            path: PathBuf::from("/Library/Caches/archive.bin"),
            size_kb: 4 * 1_048_576,
            kind: StorageItemKind::File,
            category: StorageCategory::ApplicationData,
        });
    for (width, height) in [(120, 30), (160, 48), (220, 60)] {
        let buffer = draw_fixture(&mut app, width, height);
        let text = buffer_text(&buffer);
        assert!(text.contains("SELECTED ITEM"));
        assert!(text.contains("100.0 GiB"));
        assert!(text.contains("Developer"));
        if let Some(directory) = std::env::var_os("MAC_CLEANUP_RENDER_DIR") {
            let directory = PathBuf::from(directory);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join(format!("folder-map-{width}.svg")),
                buffer_svg(&buffer),
            )
            .unwrap();
            fs::write(directory.join(format!("folder-map-{width}.txt")), text).unwrap();
        }
    }
    let click_path = |app: &mut App, path: &str| {
        let index = app
            .map_paths
            .borrow()
            .iter()
            .position(|p| p == Path::new(path))
            .expect("nested path is clickable");
        let area = app
            .hit_regions
            .borrow()
            .iter()
            .find_map(|(area, target)| {
                matches!(target,HitTarget::MapNode(i) if *i==index).then_some(*area)
            })
            .unwrap();
        app.handle_left_click(area.x + 1, area.y + 1);
    };
    click_path(&mut app, "/Library/Developer/Xcode");
    assert_eq!(
        app.explorer_path.as_deref(),
        Some(Path::new("/Library/Developer/Xcode"))
    );
    app.handle_review_key(KeyCode::Left);
    assert!(app.explorer_path.is_none());
    assert_eq!(app.explorer_cursor, 0);
    draw_fixture(&mut app, 220, 60);
    click_path(&mut app, "/Library/Caches/archive.bin");
    assert!(app.explorer_details);
    assert_eq!(
        app.explorer_items()[app.explorer_cursor].path,
        Path::new("/Library/Caches/archive.bin")
    );
    app.handle_review_key(KeyCode::Esc);
    app.handle_review_key(KeyCode::Left);
    assert!(app.explorer_path.is_none());
    assert_eq!(app.selected, BTreeSet::from([0]));
    app.no_color = true;
    let buffer = draw_fixture(&mut app, 160, 48);
    assert!(buffer_text(&buffer).contains("Developer"));
    assert!(
        buffer
            .content()
            .iter()
            .all(|c| !matches!(c.bg, Color::Rgb(..)))
    );
}
