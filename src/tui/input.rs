use super::*;

impl App {
    pub(super) fn menu_is_available(&self) -> bool {
        !matches!(
            self.phase,
            Phase::Scanning
                | Phase::Confirm
                | Phase::Details
                | Phase::Cleaning
                | Phase::RelocationDestination
                | Phase::RelocationPlanning
                | Phase::RelocationConfirm
                | Phase::Relocating
                | Phase::ReviewConfirm
                | Phase::ProcessConfirm
                | Phase::Summary
        )
    }

    pub(super) fn menu_items(&self, menu: MenuId) -> Vec<MenuItem> {
        match menu {
            MenuId::File => vec![
                MenuItem {
                    label: "Choose scan location",
                    shortcut: "↵",
                    enabled: true,
                },
                MenuItem {
                    label: "Rescan current view",
                    shortcut: "↵",
                    enabled: matches!(
                        self.phase,
                        Phase::Review | Phase::Details | Phase::Processes
                    ),
                },
                MenuItem {
                    label: "Quit",
                    shortcut: "↵",
                    enabled: true,
                },
            ],
            MenuId::View => vec![
                MenuItem {
                    label: "Storage audit",
                    shortcut: "↵",
                    enabled: true,
                },
                MenuItem {
                    label: "Process Health",
                    shortcut: "↵",
                    enabled: true,
                },
                MenuItem {
                    label: "Move data",
                    shortcut: "↵",
                    enabled: true,
                },
            ],
            MenuId::Actions => vec![
                MenuItem {
                    label: "Clean safe items",
                    shortcut: "C",
                    enabled: !self.analysis_only
                        && matches!(self.phase, Phase::Review | Phase::Details)
                        && self.ready_kb() > 0,
                },
                MenuItem {
                    label: "Relocate large data",
                    shortcut: "F6",
                    enabled: !self.analysis_only
                        && matches!(self.phase, Phase::Review | Phase::Details),
                },
                MenuItem {
                    label: "Reveal selected in Finder",
                    shortcut: "O",
                    enabled: matches!(self.phase, Phase::Review | Phase::Details)
                        && (self.entries.get(self.cursor).is_some()
                            || !self.explorer_items().is_empty()),
                },
                MenuItem {
                    label: "Signal selected process",
                    shortcut: "D",
                    enabled: !self.analysis_only
                        && self.phase == Phase::Processes
                        && self
                            .processes
                            .get(self.process_cursor)
                            .is_some_and(|process| process.signalable),
                },
            ],
            MenuId::Help => vec![
                MenuItem {
                    label: "Keyboard guide",
                    shortcut: "F1/?",
                    enabled: true,
                },
                MenuItem {
                    label: "Safety & read-only model",
                    shortcut: "F1/?",
                    enabled: true,
                },
            ],
        }
    }

    pub(super) fn open_menu(&mut self, menu: MenuId) {
        if !self.menu_is_available() {
            return;
        }
        let items = self.menu_items(menu);
        self.menu_open = Some(menu);
        self.menu_cursor = items.iter().position(|item| item.enabled).unwrap_or(0);
    }

    pub(super) fn close_menu(&mut self) {
        self.menu_open = None;
    }

    pub(super) fn move_menu_cursor(&mut self, delta: isize) {
        let Some(menu) = self.menu_open else {
            return;
        };
        let items = self.menu_items(menu);
        if items.is_empty() {
            return;
        }
        let length = items.len() as isize;
        let mut cursor = self.menu_cursor.min(items.len() - 1) as isize;
        for _ in 0..items.len() {
            cursor = (cursor + delta).rem_euclid(length);
            if items[cursor as usize].enabled {
                self.menu_cursor = cursor as usize;
                return;
            }
        }
    }

    pub(super) fn switch_menu(&mut self, delta: isize) {
        let Some(menu) = self.menu_open else {
            return;
        };
        let index = (menu.index() as isize + delta).rem_euclid(MenuId::ALL.len() as isize);
        self.open_menu(MenuId::from_index(index as usize));
    }

    pub(super) fn activate_menu_item(&mut self) {
        let Some(menu) = self.menu_open else {
            return;
        };
        let items = self.menu_items(menu);
        let Some(item) = items.get(self.menu_cursor).copied() else {
            self.close_menu();
            return;
        };
        if !item.enabled {
            return;
        }
        self.close_menu();
        self.sidebar_focus = false;
        match (menu, self.menu_cursor) {
            (MenuId::File, 0) => self.phase = Phase::Location,
            (MenuId::File, 1) => match self.phase {
                Phase::Processes => self.refresh_processes(),
                Phase::Review | Phase::Details => self.restart_scan(),
                _ => {}
            },
            (MenuId::File, 2) => self.quit = true,
            (MenuId::View, 0) => self.navigate_to(0),
            (MenuId::View, 1) => self.navigate_to(1),
            (MenuId::View, 2) => self.navigate_to(2),
            (MenuId::Actions, 0) => self.prepare_safe_cleanup(),
            (MenuId::Actions, 1) => {
                self.navigate_to(2);
            }
            (MenuId::Actions, 2) => self.reveal_current(),
            (MenuId::Actions, 3) => self.handle_process_key(KeyCode::Char('d')),
            (MenuId::Help, 0) | (MenuId::Help, 1) => self.show_help = true,
            _ => {}
        }
    }

    pub(super) fn handle_menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::F(9) => self.close_menu(),
            KeyCode::Left => self.switch_menu(-1),
            KeyCode::Right => self.switch_menu(1),
            KeyCode::Up => self.move_menu_cursor(-1),
            KeyCode::Down => self.move_menu_cursor(1),
            KeyCode::PageUp => self.move_menu_cursor(-10),
            KeyCode::PageDown => self.move_menu_cursor(10),
            KeyCode::Home => {
                if let Some(menu) = self.menu_open {
                    self.menu_cursor = self
                        .menu_items(menu)
                        .iter()
                        .position(|item| item.enabled)
                        .unwrap_or(0);
                }
            }
            KeyCode::End => {
                if let Some(menu) = self.menu_open {
                    self.menu_cursor = self
                        .menu_items(menu)
                        .iter()
                        .rposition(|item| item.enabled)
                        .unwrap_or(0);
                }
            }
            KeyCode::Enter => self.activate_menu_item(),
            KeyCode::F(1) => {
                self.close_menu();
                self.dialog_scroll = 0;
                self.show_help = true;
            }
            KeyCode::F(10) => {
                self.close_menu();
                self.quit = true;
            }
            KeyCode::Char(character) => {
                if let Some(menu) = MenuId::ALL
                    .into_iter()
                    .find(|menu| menu.hotkey().eq_ignore_ascii_case(&character))
                {
                    self.open_menu(menu);
                } else if character == 'q' {
                    self.close_menu();
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_function_key(&mut self, code: KeyCode) -> bool {
        if code == KeyCode::F(1) {
            self.dialog_scroll = 0;
            self.show_help = true;
            return true;
        }
        if !self.menu_is_available() {
            return false;
        }
        match code {
            KeyCode::F(5) => match self.phase {
                Phase::Processes => self.refresh_processes(),
                Phase::Review | Phase::Details => self.restart_scan(),
                _ => return false,
            },
            KeyCode::F(6) if !self.analysis_only => {
                self.navigate_to(2);
            }
            KeyCode::F(9) => self.open_menu(MenuId::File),
            KeyCode::F(10) => self.quit = true,
            _ => return false,
        }
        true
    }

    pub(super) fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.is_scrollable_dialog() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.dialog_scroll = self.dialog_scroll.saturating_sub(3)
                }
                MouseEventKind::ScrollDown => {
                    self.dialog_scroll = self
                        .dialog_scroll
                        .saturating_add(3)
                        .min(self.dialog_max_scroll.get())
                }
                _ => {}
            }
            return;
        }
        if !self.menu_is_available() {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if self.menu_open.is_some() {
                    self.move_menu_cursor(-1);
                } else if mouse.column < sidebar_width(self.terminal_width) {
                    if !self.sidebar_focus {
                        self.sidebar_cursor = active_section(self);
                    }
                    self.sidebar_focus = true;
                    self.handle_sidebar_key(KeyCode::Up);
                } else {
                    self.sidebar_focus = false;
                    self.scroll_active_view(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.menu_open.is_some() {
                    self.move_menu_cursor(1);
                } else if mouse.column < sidebar_width(self.terminal_width) {
                    if !self.sidebar_focus {
                        self.sidebar_cursor = active_section(self);
                    }
                    self.sidebar_focus = true;
                    self.handle_sidebar_key(KeyCode::Down);
                } else {
                    self.sidebar_focus = false;
                    self.scroll_active_view(3);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_left_click(mouse.column, mouse.row);
            }
            _ => {}
        }
    }

    pub(super) fn scroll_active_view(&mut self, delta: isize) {
        let move_cursor = |cursor: &mut usize, length: usize| {
            if delta < 0 {
                *cursor = cursor.saturating_sub(delta.unsigned_abs());
            } else {
                *cursor = (*cursor + delta as usize).min(length.saturating_sub(1));
            }
        };
        match self.phase {
            Phase::Review if self.explorer_details => {
                self.handle_storage_navigation(if delta < 0 {
                    KeyCode::Up
                } else {
                    KeyCode::Down
                });
            }
            Phase::Review
                if self.inventory.is_some() && self.storage_tab == StorageTab::Explore =>
            {
                let length = self.explorer_items().len();
                move_cursor(&mut self.explorer_cursor, length);
            }
            Phase::Review
                if self.inventory.is_some() && self.storage_tab == StorageTab::Coverage =>
            {
                self.handle_storage_navigation(if delta < 0 {
                    KeyCode::Up
                } else {
                    KeyCode::Down
                });
            }
            Phase::Review | Phase::Details => move_cursor(&mut self.cursor, self.entries.len()),
            Phase::Processes => move_cursor(&mut self.process_cursor, self.processes.len()),
            Phase::Location => move_cursor(&mut self.location_cursor, self.locations.len()),
            Phase::RelocationSources => move_cursor(
                &mut self.relocation_source_cursor,
                self.relocation_sources.len(),
            ),
            _ => {}
        }
    }

    pub(super) fn handle_left_click(&mut self, column: u16, row: u16) {
        if self.show_help || !self.menu_is_available() {
            return;
        }

        if let Some(menu) = self.menu_open {
            let area = Rect::new(0, 0, self.terminal_width, self.terminal_height);
            let items = self.menu_items(menu);
            if let Some(popup) = menu_popup_rect(area, menu, items.len())
                && rect_contains(popup, column, row)
                && row > popup.y
                && row < popup.y.saturating_add(popup.height).saturating_sub(1)
            {
                let index = row.saturating_sub(popup.y).saturating_sub(1) as usize;
                if items.get(index).is_some_and(|item| item.enabled) {
                    self.menu_cursor = index;
                    self.activate_menu_item();
                }
                return;
            }
            self.close_menu();
        }

        if let Some(section) = navigation_at(column, row, self.terminal_width) {
            self.navigate_to(section);
            return;
        }
        if !self.sidebar_focus && column < sidebar_width(self.terminal_width) {
            self.sidebar_cursor = active_section(self);
        }
        self.sidebar_focus = column < sidebar_width(self.terminal_width);
        if self.sidebar_focus {
            return;
        }
        let hit = self
            .hit_regions
            .borrow()
            .iter()
            .find(|(area, _)| rect_contains(*area, column, row))
            .map(|(_, hit)| *hit);
        if let Some(hit) = hit {
            self.sidebar_focus = false;
            match hit {
                HitTarget::StorageTab(tab) if self.phase == Phase::Review => {
                    self.switch_storage_tab(tab)
                }
                HitTarget::Consumer(index) if self.phase == Phase::Review => {
                    if self.explorer_cursor == index {
                        self.open_consumer();
                    } else {
                        self.explorer_cursor = index;
                    }
                }
                HitTarget::Finding(index) if self.phase == Phase::Review => self.cursor = index,
                HitTarget::Location(index) if self.phase == Phase::Location => {
                    self.location_cursor = index
                }
                HitTarget::Process(index) if self.phase == Phase::Processes => {
                    self.process_cursor = index
                }
                HitTarget::Source(index) if self.phase == Phase::RelocationSources => {
                    self.relocation_source_cursor = index
                }
                _ => {}
            }
        }
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if (self.terminal_width < MIN_WIDTH || self.terminal_height < MIN_HEIGHT)
            && !(matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
                || (key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)))
        {
            return;
        }
        if self.is_scrollable_dialog() {
            match key.code {
                KeyCode::Up | KeyCode::PageUp => {
                    self.dialog_scroll = self.dialog_scroll.saturating_sub(3);
                    return;
                }
                KeyCode::Down | KeyCode::PageDown => {
                    self.dialog_scroll = self
                        .dialog_scroll
                        .saturating_add(3)
                        .min(self.dialog_max_scroll.get());
                    return;
                }
                KeyCode::Home => {
                    self.dialog_scroll = 0;
                    return;
                }
                _ => {}
            }
        }
        if self.menu_open.is_some() {
            self.handle_menu_key(key);
            return;
        }
        if self.show_help {
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::Char('q') => {
                    self.show_help = false;
                }
                _ => {}
            }
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            match self.phase {
                Phase::Cleaning => self.request_cleanup_stop(),
                Phase::RelocationPlanning | Phase::Relocating => {
                    self.status_message = Some(
                        "Relocation is in progress; wait for validation or copy verification to finish."
                            .into(),
                    );
                }
                _ => self.quit = true,
            }
            return;
        }

        if key.modifiers.contains(KeyModifiers::ALT)
            && let KeyCode::Char(character) = key.code
            && let Some(menu) = MenuId::ALL
                .into_iter()
                .find(|menu| menu.hotkey().eq_ignore_ascii_case(&character))
        {
            self.open_menu(menu);
            return;
        }

        if self.menu_is_available()
            && !key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL)
            && let KeyCode::Char(character @ '1'..='3') = key.code
        {
            self.navigate_to(character as usize - '1' as usize);
            return;
        }
        if self.handle_function_key(key.code) {
            return;
        }

        if key.code == KeyCode::Char('?')
            && !matches!(
                self.phase,
                Phase::Scanning
                    | Phase::Cleaning
                    | Phase::RelocationDestination
                    | Phase::RelocationPlanning
                    | Phase::RelocationConfirm
                    | Phase::Relocating
                    | Phase::ReviewConfirm
                    | Phase::ProcessConfirm
            )
        {
            self.dialog_scroll = 0;
            self.show_help = true;
            return;
        }

        if self.menu_is_available()
            && matches!(key.code, KeyCode::Tab | KeyCode::BackTab | KeyCode::F(8))
        {
            self.sidebar_focus = !self.sidebar_focus;
            if self.sidebar_focus {
                self.sidebar_cursor = active_section(self);
            }
            return;
        }
        if self.sidebar_focus && self.menu_is_available() {
            self.handle_sidebar_key(key.code);
            return;
        }
        match self.phase {
            Phase::Location => self.handle_location_key(key.code),
            Phase::Scanning => match key.code {
                KeyCode::Esc => self.cancel_scan(),
                KeyCode::Char('q') => {
                    self.cancel_scan();
                    self.quit = true;
                }
                _ => {}
            },
            Phase::Review => self.handle_review_key(key.code),
            Phase::RelocationSources => self.handle_relocation_sources_key(key.code),
            Phase::RelocationDestination => self.handle_relocation_destination_key(key),
            Phase::RelocationPlanning | Phase::Relocating => {}
            Phase::RelocationConfirm => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.begin_relocation(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.relocation_plan = None;
                    self.phase = Phase::RelocationDestination;
                }
                _ => {}
            },
            Phase::RelocationResult => match key.code {
                KeyCode::Enter | KeyCode::Esc => self.close_relocation_result(),
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            },
            Phase::Details => match key.code {
                KeyCode::Char('d') | KeyCode::Char('D') if !self.analysis_only => {
                    self.prepare_current_cleanup();
                }
                KeyCode::Char('c') | KeyCode::Char('C') if !self.analysis_only => {
                    self.prepare_safe_cleanup();
                }
                KeyCode::Char('m') | KeyCode::Char('M') if !self.analysis_only => {
                    self.open_relocation_sources();
                }
                KeyCode::Char('o') | KeyCode::Char('O') => self.reveal_current(),
                KeyCode::Enter | KeyCode::Esc => self.phase = Phase::Review,
                KeyCode::Char('q') => self.quit = true,
                _ => {}
            },
            Phase::Confirm => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.begin_cleanup(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.phase = Phase::Review;
                }
                _ => {}
            },
            Phase::ReviewConfirm => self.handle_review_confirmation_key(key),
            Phase::Processes => self.handle_process_key(key.code),
            Phase::ProcessConfirm => match key.code {
                KeyCode::Char('t') | KeyCode::Char('T') => {
                    self.begin_process_signal(ProcessSignal::Terminate);
                }
                KeyCode::Char('k') | KeyCode::Char('K') => {
                    self.begin_process_signal(ProcessSignal::Kill);
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.process_target = None;
                    self.phase = Phase::Processes;
                }
                _ => {}
            },
            Phase::Cleaning => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    self.request_cleanup_stop();
                }
            }
            Phase::Summary => {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Enter | KeyCode::Esc) {
                    self.quit = true;
                }
            }
        }
    }

    pub(super) fn handle_sidebar_key(&mut self, code: KeyCode) {
        let last = if self.analysis_only { 1 } else { 2 };
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.sidebar_cursor = self.sidebar_cursor.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.sidebar_cursor = (self.sidebar_cursor + 1).min(last)
            }
            KeyCode::Home => self.sidebar_cursor = 0,
            KeyCode::End => self.sidebar_cursor = last,
            KeyCode::Enter | KeyCode::Right => self.navigate_to(self.sidebar_cursor),
            KeyCode::Esc => {
                self.sidebar_focus = false;
                self.sidebar_cursor = active_section(self);
            }
            KeyCode::Char('q') => self.quit = true,
            _ => {} // Content actions must never fall through while the menu owns focus.
        }
    }

    pub(super) fn handle_location_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.location_cursor = self.location_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.location_cursor =
                    (self.location_cursor + 1).min(self.locations.len().saturating_sub(1));
            }
            KeyCode::Home => self.location_cursor = 0,
            KeyCode::End => self.location_cursor = self.locations.len().saturating_sub(1),
            KeyCode::Enter => self.choose_location(),
            KeyCode::Esc => {
                self.sidebar_focus = true;
                self.sidebar_cursor = 0;
            }
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    pub(super) fn handle_review_key(&mut self, code: KeyCode) {
        if self.handle_storage_navigation(code) {
            return;
        }
        match code {
            KeyCode::PageUp => self.cursor = self.cursor.saturating_sub(10),
            KeyCode::PageDown => {
                self.cursor = (self.cursor + 10).min(self.entries.len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor = self.cursor.saturating_sub(1);
                self.status_message = None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(self.entries.len().saturating_sub(1));
                self.status_message = None;
            }
            KeyCode::Home => {
                self.cursor = 0;
                self.status_message = None;
            }
            KeyCode::End => {
                self.cursor = self.entries.len().saturating_sub(1);
                self.status_message = None;
            }
            KeyCode::Char(' ') if !self.analysis_only => {
                self.mode = Mode::Clean;
                self.toggle_current();
            }
            KeyCode::Char('a') if !self.analysis_only => {
                self.mode = Mode::Clean;
                self.toggle_all();
            }
            KeyCode::Char('d') | KeyCode::Char('D') if !self.analysis_only => {
                self.prepare_current_cleanup();
            }
            KeyCode::Char('c') | KeyCode::Char('C') if !self.analysis_only => {
                self.prepare_safe_cleanup();
            }
            KeyCode::Char('m') | KeyCode::Char('M') if !self.analysis_only => {
                self.open_relocation_sources();
            }
            KeyCode::Char('i') => {
                self.include_reinstallable = !self.include_reinstallable;
                self.restart_scan();
            }
            KeyCode::Char('o') | KeyCode::Char('O') => self.reveal_current(),
            KeyCode::Char('r') => self.restart_scan(),
            KeyCode::Enter if self.mode == Mode::Clean && !self.selected.is_empty() => {
                self.dialog_scroll = 0;
                self.phase = Phase::Confirm;
            }
            KeyCode::Enter if self.entries.get(self.cursor).is_some() => {
                self.dialog_scroll = 0;
                self.phase = Phase::Details;
            }
            KeyCode::Esc => {
                self.sidebar_focus = true;
                self.sidebar_cursor = 0;
            }
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    pub(super) fn handle_relocation_sources_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.relocation_source_cursor = self.relocation_source_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.relocation_source_cursor = (self.relocation_source_cursor + 1)
                    .min(self.relocation_sources.len().saturating_sub(1));
            }
            KeyCode::PageUp => {
                self.relocation_source_cursor = self.relocation_source_cursor.saturating_sub(10);
            }
            KeyCode::PageDown => {
                self.relocation_source_cursor = (self.relocation_source_cursor + 10)
                    .min(self.relocation_sources.len().saturating_sub(1));
            }
            KeyCode::Home => self.relocation_source_cursor = 0,
            KeyCode::End => {
                self.relocation_source_cursor = self.relocation_sources.len().saturating_sub(1)
            }
            KeyCode::Enter => self.choose_relocation_source(),
            KeyCode::Esc => self.phase = Phase::Review,
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    pub(super) fn handle_relocation_destination_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => {
                self.relocation_destination.pop();
                self.relocation_destination_error = None;
            }
            KeyCode::Enter => self.start_relocation_plan(),
            KeyCode::Esc => {
                self.relocation_destination_error = None;
                self.phase = Phase::RelocationSources;
            }
            KeyCode::Char('q') if self.relocation_destination.is_empty() => self.quit = true,
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.relocation_destination.len() < 4096 =>
            {
                self.relocation_destination.push(character);
                self.relocation_destination_error = None;
            }
            _ => {}
        }
    }

    pub(super) fn open_process_review(&mut self) {
        self.sidebar_focus = false;
        self.refresh_processes();
        self.phase = Phase::Processes;
    }

    pub(super) fn refresh_processes(&mut self) {
        let started = Instant::now();
        match review_processes() {
            Ok(processes) => {
                self.processes = processes;
                self.process_scan_error = None;
            }
            Err(error) => {
                self.processes.clear();
                self.process_scan_error = Some(error);
            }
        }
        self.process_scan_completed = true;
        self.process_scan_duration = Some(started.elapsed());
        self.process_cursor = self
            .process_cursor
            .min(self.processes.len().saturating_sub(1));
        self.process_target = None;
    }

    pub(super) fn handle_process_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.process_cursor = self.process_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.process_cursor =
                    (self.process_cursor + 1).min(self.processes.len().saturating_sub(1));
            }
            KeyCode::PageUp => self.process_cursor = self.process_cursor.saturating_sub(10),
            KeyCode::PageDown => {
                self.process_cursor =
                    (self.process_cursor + 10).min(self.processes.len().saturating_sub(1));
            }
            KeyCode::Home => self.process_cursor = 0,
            KeyCode::End => self.process_cursor = self.processes.len().saturating_sub(1),
            KeyCode::Char('r') => self.refresh_processes(),
            KeyCode::Char('d') | KeyCode::Char('D') if !self.analysis_only => {
                if self
                    .processes
                    .get(self.process_cursor)
                    .is_some_and(|process| {
                        process.signalable
                            && !matches!(process.outcome, Some(ProcessOutcome::Exited { .. }))
                    })
                {
                    self.mode = Mode::Clean;
                    self.process_target = Some(self.process_cursor);
                    self.dialog_scroll = 0;
                    self.phase = Phase::ProcessConfirm;
                }
            }
            KeyCode::Esc => self.navigate_to(0),
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    pub(super) fn begin_process_signal(&mut self, signal: ProcessSignal) {
        let Some(index) = self.process_target.take() else {
            self.phase = Phase::Processes;
            return;
        };
        if let Some(process) = self.processes.get_mut(index) {
            signal_process(process, signal);
        }
        self.phase = Phase::Processes;
    }
}

impl App {
    pub(super) fn is_scrollable_dialog(&self) -> bool {
        self.show_help
            || matches!(
                self.phase,
                Phase::Confirm
                    | Phase::ReviewConfirm
                    | Phase::ProcessConfirm
                    | Phase::RelocationConfirm
                    | Phase::Details
            )
    }

    pub(super) fn navigate_to(&mut self, section: usize) {
        if !self.menu_is_available() {
            return;
        }
        if section == 2 && self.analysis_only {
            self.status_message =
                Some("Move data is unavailable in this read-only session.".into());
            return;
        }
        self.status_message = None;
        self.sidebar_focus = false;
        self.sidebar_cursor = section;
        match section {
            0 => {
                self.phase = if self.inventory.is_some() || !self.entries.is_empty() {
                    Phase::Review
                } else {
                    Phase::Location
                }
            }
            1 => self.open_process_review(),
            2 if !self.analysis_only => {
                if self.inventory.is_some() {
                    self.open_relocation_sources();
                } else {
                    self.phase = Phase::Location;
                }
            }
            _ => {}
        }
    }
}
