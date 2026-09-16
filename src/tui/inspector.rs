use super::*;

pub(super) fn render_details(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if matches!(app.phase, Phase::Processes | Phase::ProcessConfirm) {
        render_process_details(frame, area, app);
        return;
    }
    let lines = if app.phase == Phase::Scanning {
        if app.retention_worker.is_some() {
            vec![
                Line::from(vec![
                    Span::styled("Current  ", Style::default().fg(app.color(MUTED))),
                    Span::styled(
                        "Temporary-data safety check",
                        Style::default()
                            .fg(app.color(BLUE))
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Scope    ", Style::default().fg(app.color(MUTED))),
                    Span::raw("user-owned stale entries under /private/tmp"),
                ]),
                Line::from(vec![
                    Span::styled("Safety   ", Style::default().fg(app.color(MUTED))),
                    Span::raw("checking age, ownership, open handles, and allocated size"),
                ]),
                Line::from(Span::styled(
                    "The interface remains responsive while this check runs in the background.",
                    Style::default().fg(app.color(MINT)),
                )),
            ]
        } else if let Some(task) = &app.scan_task {
            let access = if task.errors == 0 {
                "Reading allocated filesystem blocks".to_string()
            } else {
                format!("Skipped {} unreadable item(s)", task.errors)
            };
            vec![
                Line::from(vec![
                    Span::styled("Current  ", Style::default().fg(app.color(MUTED))),
                    Span::styled(
                        task.spec.label,
                        Style::default()
                            .fg(app.color(BLUE))
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Path     ", Style::default().fg(app.color(MUTED))),
                    Span::raw(task.spec.path.display().to_string()),
                ]),
                Line::from(vec![
                    Span::styled("Progress ", Style::default().fg(app.color(MUTED))),
                    Span::raw(format!(
                        "{} items inspected • {} found • {} elapsed",
                        task.inspected_items,
                        format_kb(task.size_kb()),
                        format_elapsed(task.started_at.elapsed()),
                    )),
                ]),
                Line::from(vec![
                    Span::styled("Access   ", Style::default().fg(app.color(MUTED))),
                    Span::raw(access),
                ]),
            ]
        } else if app.inventory_worker.is_some() {
            let elapsed = app
                .inventory_started_at
                .map_or(Duration::ZERO, |started_at| started_at.elapsed());
            vec![
                Line::from(vec![
                    Span::styled("Current  ", Style::default().fg(app.color(MUTED))),
                    Span::styled(
                        "Full-volume storage inventory",
                        Style::default()
                            .fg(app.color(BLUE))
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Path     ", Style::default().fg(app.color(MUTED))),
                    Span::raw(app.scan_root.display().to_string()),
                ]),
                Line::from(vec![
                    Span::styled("Progress ", Style::default().fg(app.color(MUTED))),
                    Span::raw(format!(
                        "walking directories and allocated blocks • {} elapsed",
                        format_elapsed(elapsed)
                    )),
                ]),
                Line::from(Span::styled(
                    "This accounts for personal and app data for review; it does not make them cleanup candidates.",
                    Style::default().fg(app.color(AMBER)),
                )),
            ]
        } else {
            let next = app
                .specs
                .get(app.scan_index)
                .map(|spec| format!("Preparing {}…", spec.label))
                .unwrap_or_else(|| "Finishing scan…".into());
            vec![Line::from(next)]
        }
    } else if let Some(entry) = app.entries.get(app.cursor) {
        let outcome = match &entry.outcome {
            Some(CleanupOutcome::Cleared { removed_kb, method }) => {
                format!(
                    "Cleared; reclaimed about {} via {}.",
                    format_kb(*removed_kb),
                    method.explanation()
                )
            }
            Some(CleanupOutcome::SafetySkipped(reason)) => format!("Skipped: {reason}."),
            Some(CleanupOutcome::Failed { error, removed_kb }) if *removed_kb > 0 => format!(
                "Failed after removing about {}: {error}",
                format_kb(*removed_kb)
            ),
            Some(CleanupOutcome::Failed { error, .. }) => format!("Failed: {error}"),
            None => entry.status.explanation().to_string(),
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled("Path  ", Style::default().fg(app.color(MUTED))),
                Span::raw(entry.spec.path.display().to_string()),
            ]),
            Line::from(vec![
                Span::styled("Effect  ", Style::default().fg(app.color(MUTED))),
                Span::raw(entry.spec.note),
            ]),
            Line::from(vec![
                Span::styled("Safety  ", Style::default().fg(app.color(MUTED))),
                Span::raw(outcome),
            ]),
        ];
        if let Some(message) = &app.status_message {
            lines.push(Line::from(vec![
                Span::styled("Finder  ", Style::default().fg(app.color(MUTED))),
                Span::raw(message),
            ]));
        }
        lines
    } else {
        vec![Line::from(
            "No allowlisted cleanup findings were found. Review DISK USAGE for actual consumers.",
        )]
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(app, " DETAILS "))
            .wrap(Wrap { trim: true }),
        area,
    );
}

pub(super) fn render_entry_details(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(entry) = app.entries.get(app.cursor) else {
        return;
    };

    let availability = match &entry.outcome {
        Some(CleanupOutcome::Cleared { removed_kb, method }) => {
            format!(
                "Cleared; reclaimed about {} via {}.",
                format_kb(*removed_kb),
                method.explanation()
            )
        }
        Some(CleanupOutcome::SafetySkipped(reason)) => format!("Skipped: {reason}."),
        Some(CleanupOutcome::Failed { error, removed_kb }) if *removed_kb > 0 => format!(
            "Failed after removing about {}: {error}",
            format_kb(*removed_kb)
        ),
        Some(CleanupOutcome::Failed { error, .. }) => format!("Failed: {error}"),
        None => entry.status.explanation().to_string(),
    };
    let guidance = decision_guidance(entry);
    let status_color = match entry.status {
        CacheStatus::Ready => MINT,
        CacheStatus::Optional | CacheStatus::InUse => AMBER,
        CacheStatus::Review => ORCHID,
        CacheStatus::Whitelisted => BLUE,
        CacheStatus::ScanError | CacheStatus::Symlink | CacheStatus::Invalid => CORAL,
        CacheStatus::Missing => MUTED,
    };
    let mut text = vec![
        Line::from(vec![
            Span::styled("Size    ", Style::default().fg(app.color(MUTED))),
            Span::styled(
                format_kb(entry.size_kb),
                Style::default()
                    .fg(app.color(BLUE))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("     Status  "),
            Span::styled(
                entry.status.label(),
                Style::default()
                    .fg(app.color(status_color))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Path    ", Style::default().fg(app.color(MUTED))),
            Span::raw(entry.spec.path.display().to_string()),
        ]),
        Line::from(vec![
            Span::styled("Cleanup ", Style::default().fg(app.color(MUTED))),
            Span::raw(cleanup_plan(&entry.spec)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Type    ", Style::default().fg(app.color(MUTED))),
            Span::styled(
                guidance.classification,
                Style::default()
                    .fg(app.color(guidance.color))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Deletes ", Style::default().fg(app.color(MUTED))),
            Span::raw(guidance.delete_scope),
        ]),
        Line::from(vec![
            Span::styled("Impact  ", Style::default().fg(app.color(MUTED))),
            Span::raw(guidance.impact),
        ]),
        Line::from(vec![
            Span::styled("Recovery ", Style::default().fg(app.color(MUTED))),
            Span::raw(guidance.recovery),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Recommend ", Style::default().fg(app.color(MUTED))),
            Span::styled(
                guidance.recommendation,
                Style::default().fg(app.color(BLUE)),
            ),
        ]),
        Line::from(vec![
            Span::styled("Available ", Style::default().fg(app.color(MUTED))),
            Span::raw(availability),
        ]),
        Line::from(vec![
            Span::styled("Decision  ", Style::default().fg(app.color(MUTED))),
            Span::raw(decision_action(app, entry)),
        ]),
        Line::from(""),
        Line::from(
            if !app.analysis_only && entry.status == CacheStatus::Review {
                "d advanced delete   •   m relocate large data   •   c clean all safe   •   o reveal in Finder   •   Enter/Esc close"
            } else if !app.analysis_only && entry.status == CacheStatus::Optional {
                "d delete this opt-in item   •   m relocate large data   •   i opt in all   •   o reveal in Finder   •   Enter/Esc close"
            } else if !app.analysis_only && app.is_selectable(app.cursor) {
                "d delete this safe item   •   m relocate large data   •   c clean all safe   •   o reveal in Finder   •   Enter/Esc close"
            } else if !app.analysis_only && app.mode == Mode::Analyze {
                "c clean all safe   •   o reveal in Finder   •   Enter/Esc close details   •   q quit"
            } else {
                "o reveal in Finder   •   Enter/Esc close details   •   q quit"
            },
        ),
    ];
    if let Some(message) = &app.status_message {
        text.push(Line::from(Span::styled(
            message,
            Style::default().fg(app.color(BLUE)),
        )));
    }
    render_dialog(
        frame,
        area,
        app,
        &display_entry_label(entry),
        BLUE,
        text,
        vec![label(app, "Enter / Esc close  ·  o reveal in Finder")],
    );
}

pub(super) struct DecisionGuidance {
    classification: &'static str,
    delete_scope: &'static str,
    pub(super) impact: &'static str,
    pub(super) recovery: &'static str,
    pub(super) recommendation: &'static str,
    color: Color,
}

pub(super) fn decision_guidance(entry: &CacheEntry) -> DecisionGuidance {
    if entry.status == CacheStatus::Whitelisted {
        return DecisionGuidance {
            classification: "USER WHITELIST — NEVER CLEANED",
            delete_scope: "Nothing. This path matches ~/.config/mac-cleanup/whitelist and every cleanup path refuses it.",
            impact: "The stored data stays exactly as it is until the whitelist entry is removed.",
            recovery: "Nothing to recover; no cleanup is attempted while it is whitelisted.",
            recommendation: "Remove the line from the whitelist file only if you want this location to become cleanable again.",
            color: BLUE,
        };
    }
    match entry.spec.label {
        "Xcode device support" => DecisionGuidance {
            classification: "DEVELOPER SUPPORT DATA — NOT A CACHE",
            delete_scope: "All installed iOS device-support files in this directory.",
            impact: "Debugging connected devices may pause while Xcode restores compatible support files.",
            recovery: "Usually downloadable again by Xcode, but older device support may be harder to restore.",
            recommendation: "Remove unneeded Developer storage through Xcode or System Settings first.",
            color: ORCHID,
        },
        "Xcode archives" => DecisionGuidance {
            classification: "BUILD ARCHIVES — NOT A CACHE",
            delete_scope: "Every Xcode archive here, including archived builds and their symbols.",
            impact: "You may lose distribution history and dSYMs needed to symbolicate old crash reports.",
            recovery: "Not automatically recoverable; keep archives or symbols required by shipped apps.",
            recommendation: "Delete individual obsolete archives from Xcode Organizer instead.",
            color: ORCHID,
        },
        "Simulator devices" => DecisionGuidance {
            classification: "SIMULATOR DEVICES — NOT A CACHE",
            delete_scope: "All simulator devices here, including installed apps, settings, and local app data.",
            impact: "Simulator-only test data and configured devices will be lost.",
            recovery: "Devices can be recreated, but their apps and local data cannot be reconstructed automatically.",
            recommendation: "Remove only unwanted devices through Xcode's Devices and Simulators window.",
            color: ORCHID,
        },
        "Cursor user data" => DecisionGuidance {
            classification: "EDITOR USER DATA — NOT A CACHE",
            delete_scope: "Cursor settings, keybindings, snippets, workspace state, history, and other user data here.",
            impact: "Editor configuration and local workspace history may be permanently lost.",
            recovery: "Only data already synced or backed up can be restored reliably.",
            recommendation: "Clean up from Cursor or back up this directory before considering full deletion.",
            color: ORCHID,
        },
        "Chrome DevTools MCP profile" => DecisionGuidance {
            classification: "PERSISTENT BROWSER PROFILE — NOT A CACHE",
            delete_scope: "The dedicated Chrome profile, including sessions, cookies, settings, and site data.",
            impact: "MCP browser sessions and authenticated state stored in this profile will be lost.",
            recovery: "The profile can be recreated, but unsynced session and site data cannot be restored automatically.",
            recommendation: "Keep it unless you intentionally want to reset the Chrome DevTools MCP browser profile.",
            color: ORCHID,
        },
        "OrbStack data" => DecisionGuidance {
            classification: "CONTAINER AND VM DATA — NOT A CACHE",
            delete_scope: "All OrbStack containers, images, Linux machines, and volumes stored here.",
            impact: "Local services and persistent volume or machine data can be permanently lost.",
            recovery: "Images may be pulled again; deleted machines and volume data are not automatically recoverable.",
            recommendation: "Open OrbStack and remove unused items individually; use d here only to reset all local OrbStack data.",
            color: ORCHID,
        },
        "Telegram local data" => DecisionGuidance {
            classification: "MESSAGING APP DATA — NOT A CACHE",
            delete_scope: "All Telegram account and media data stored inside this local directory.",
            impact: "The app may require sign-in and downloads again; local-only state may be lost.",
            recovery: "Cloud content may resync, but local-only data is not guaranteed to return.",
            recommendation: "Use Telegram Settings → Data and Storage → Storage Usage first.",
            color: ORCHID,
        },
        "Hugging Face models" => DecisionGuidance {
            classification: "REINSTALLABLE MODEL DOWNLOADS",
            delete_scope: "Cached Hugging Face repository snapshots, model files, and related hub data.",
            impact: "Offline model use will stop and future runs may need a large download.",
            recovery: "Usually downloadable again if the repository, revision, access, and network remain available.",
            recommendation: "Prefer `hf cache rm <repo> --dry-run` or `hf cache prune --dry-run` to preview selective cleanup.",
            color: AMBER,
        },
        _ if entry.spec.tier == CacheTier::ReviewOnly => DecisionGuidance {
            classification: "APP-MANAGED DATA — NOT A CACHE",
            delete_scope: "Every item inside this app-managed directory; only the containing folder remains.",
            impact: entry.spec.note,
            recovery: "Some contents may not be recoverable unless they are synced or backed up elsewhere.",
            recommendation: "Prefer the owning application's storage controls and delete only what you recognize.",
            color: ORCHID,
        },
        _ if entry.spec.tier == CacheTier::Reinstallable => DecisionGuidance {
            classification: "LARGE REINSTALLABLE DOWNLOADS",
            delete_scope: "Every cached download inside this directory; the containing folder remains.",
            impact: entry.spec.note,
            recovery: "Usually downloadable again, but restoring it needs network access, time, and possibly credentials.",
            recommendation: "Delete only if the saved space is worth the later download.",
            color: AMBER,
        },
        _ => DecisionGuidance {
            classification: "REGENERABLE CACHE",
            delete_scope: "Every cache item inside this directory; the containing folder remains.",
            impact: entry.spec.note,
            recovery: "The owning application or tool is expected to regenerate the contents when needed.",
            recommendation: "Reasonable to delete for space after closing the related application or tool.",
            color: MINT,
        },
    }
}

pub(super) fn decision_action(app: &App, entry: &CacheEntry) -> &'static str {
    if app.analysis_only {
        return "Read-only (--analyze); restart without --analyze if you decide to delete.";
    }
    match entry.status {
        CacheStatus::Review => {
            "Prefer the recommendation above. d requires typing DELETE and permanently erases all listed data."
        }
        CacheStatus::Optional => {
            "d opts in only this item and opens a final permanent-deletion confirmation."
        }
        CacheStatus::Ready => "d opens a final permanent-deletion confirmation for this item.",
        _ => {
            "Deletion is disabled until the safety issue shown above is resolved and the item is rescanned."
        }
    }
}

/// Persistent decision context on wide terminals; full details remain available with Enter.
pub(super) fn render_inspector(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if matches!(app.phase, Phase::Processes | Phase::ProcessConfirm) {
        render_process_details(frame, area, app);
        return;
    }
    if app.phase == Phase::Scanning || app.entries.get(app.cursor).is_none() {
        render_details(frame, area, app);
        return;
    }
    let entry = &app.entries[app.cursor];
    if entry.outcome.is_some() {
        render_details(frame, area, app);
        return;
    }
    let guidance = decision_guidance(entry);
    let lines = vec![
        label(app, "SELECTED FINDING"),
        Line::from(""),
        heading(app, display_entry_label(entry)),
        Line::from(vec![
            Span::styled(
                format_kb(entry.size_kb),
                Style::default()
                    .fg(app.color(BLUE))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  ·  {}", entry.status.label())),
        ]),
        Line::from(""),
        label(app, "EXACT PATH"),
        Line::from(entry.spec.path.display().to_string()),
        Line::from(""),
        label(app, "WHAT THIS IS"),
        Line::from(entry.spec.note),
        Line::from(""),
        label(app, "BEFORE YOU ACT"),
        Line::from(guidance.recommendation),
        Line::from(""),
        label(app, "IF REMOVED"),
        Line::from(guidance.impact),
        label(app, "RECOVERY"),
        Line::from(guidance.recovery),
        Line::from(""),
        label(app, "AVAILABILITY"),
        Line::from(entry.status.explanation()),
        Line::from(""),
        Line::from(decision_action(app, entry)),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(panel(app, " UNDERSTAND & DECIDE ")),
        area,
    );
}
