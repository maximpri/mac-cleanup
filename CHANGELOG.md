# Changelog

All notable changes to this project will be documented in this file. The
format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Removed the empty Overview destination. The app now starts its read-only
  storage audit automatically and opens the Storage audit when it is ready;
  the persistent menu contains Storage, Process health, and Move data.

- Added a proportional Storage heatmap (`h`) so large folders and cleanable
  areas are visible before an operator makes a cleanup decision.

- Added a compact Space map to the persistent sidebar. It follows the current
  folder level and keeps the largest consumers visible during review.

- Made sidebar menu items and their selection highlight occupy one row, with
  mouse targets aligned to the compact menu.

- Replaced sidebar row painting with a Ratatui stateful list and explicit pane
  focus. Tab/Shift+Tab now switches panes; bracket keys switch storage views.
  Menu focus consumes content shortcuts, mouse scrolling follows its pane,
  disabled Move data is visible in read-only mode, and focus hints reflect
  the actual keyboard target.

- Simplified navigation to a persistent left menu and right content pane at all
  supported sizes; removed duplicate dashboard cards, the top menu bar, and
  oversized empty-state framing. Added F8 sidebar keyboard focus.

- Replaced the storage audit's static consumer list with a folder explorer,
  cleanup decision workspace, and scan coverage view. Added instant directory
  drill-down, per-level size shares, full-screen item information, contextual
  cleanup links, clickable tabs, and parent navigation.
- Made incomplete scans, APFS accounting differences, unreadable paths, and
  local snapshot dates accessible in the app, with a Full Disk Access shortcut.
- Improved contrast, compact terminal layouts, disk fullness visibility,
  cleanup decision labels, and removal/recovery guidance.

- Redesigned the terminal workspace for version 0.3.0: high-contrast slate theme,
  persistent navigation, disk overview, readable findings, wide-screen inspector,
  scan stages, and cleanup results.
- Split the monolithic terminal implementation into focused session, input,
  worker, layout, rendering, and dialog modules.
- Added direct 1–4 section navigation, mouse selection of visible rows, and
  Space/a selection from default interactive mode.
- Cleanup confirmation lists exact paths and per-target removal scope. Long
  dialogs scroll while confirmation/cancellation controls remain pinned.
- Blocked background mouse and menu navigation during modal decisions and hidden
  actions below the minimum 60×16 viewport.
- Added synthetic UI previews and regressions for responsive layouts, modal
  isolation, pointer targeting, scrolling, and read-only selection protection.

### Added

- Deletion-sink identity verification: every cleanup target captures the
  device and inode numbers of its path and parent directory during the scan,
  and the deletion step refuses to act if the filesystem entry changed after
  the scan or the identity could not be captured.
- User whitelist file (`~/.config/mac-cleanup/whitelist`) with per-component
  wildcards, tilde expansion, case-insensitive matching, subtree protection,
  and symlink-spelling awareness; whitelisted locations are reported as
  `PROTECTED` and refused by every cleanup path, including review deletion.
- Append-only deletions audit log at
  `~/Library/Logs/mac-cleanup/deletions.log` recording every cleanup attempt
  with timestamp, outcome, reclaimed size, label, and exact path.
- Age-retention cleanup sources beside `/private/tmp`: stale incomplete
  downloads (`*.crdownload`, `*.part`, `*.download`) in the account Downloads
  folder, plus aged directories `~/Library/Logs` (7 days),
  `~/Library/Saved Application State` (30 days), and
  `~/Library/DiagnosticReports` (30 days) that release only entries untouched
  past their retention window and keep newer entries, symlinks, and the
  directory itself.
- Read-only reporting of local APFS Time Machine snapshot dates and APFS
  container free space in plain, JSON, and dashboard storage output to explain
  usage that a directory walk cannot see.

- Read-only full-volume storage accounting and largest-consumer inventory,
  including macOS startup/data volume coverage, so low free space is explained
  separately from the narrow cleanup allowlist.
- Read-only analysis and guarded cleanup for known macOS caches, Trash,
  mounted-volume recycle bins, and temporary data.
- Full-screen TUI with live, cancelable scanning and local, USB, and network
  location selection.
- Optional reinstallable-cache handling and typed confirmation for
  app-managed `REVIEW` data.
- Native-command-first cleanup for supported package managers and applications,
  followed by exact-path cleanup of any remaining contents.
- Configurable `/private/tmp` retention reporting and cleanup for old,
  current-account-owned entries that are not open by a process.
- Explicit external-volume relocation for one large user-owned directory,
  with verified copy, rollback-safe symlink replacement, and no automatic use.
- TUI relocation flow from the largest-consumer inventory, with destination
  input, asynchronous validation/copy, explicit confirmation, and rescan.
- Versioned JSON output for automation.
- Guarded review of current-account zombie, stopped, and uninterruptible-wait
  processes, with explicit `SIGTERM` and `SIGKILL` actions in the TUI.
- System-care dashboard with equal top-level navigation for Overview, Process
  Health, and Storage Cleanup; Process Health can be run immediately at launch
  and includes ordinary processes for manual review of UI hangs.

### Security

- Exact-path allowlisting, symlink and file-type validation, process checks,
  root/sudo refusal, immediate pre-cleanup safety revalidation, and
  scan-time-to-deletion inode identity verification at the deletion sink.
- `/private/tmp` retention fails closed when ownership, tree contents, or open
  process checks cannot be verified; incomplete-download discovery fails
  closed to no candidates when its open-file check cannot run.
- Whitelist enforcement runs inside the cleanup functions themselves, so
  interactive selection, unattended cleanup, and confirmed review deletion all
  refuse protected paths.
- PID owner, parent, start-time, health, and ancestry revalidation immediately
  before any requested process signal; unattended cleanup never sends signals.

[Unreleased]: https://github.com/maximpri/mac-cleanup/commits/main
