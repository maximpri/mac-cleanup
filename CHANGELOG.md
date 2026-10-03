# Changelog

All notable changes to this project will be documented in this file. The
format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `diskray license` prints the full GPLv3 license and project notice without
  scanning, checking the host environment, or migrating settings. Terminal help
  also exposes copyright, redistribution, and warranty information.
- Dependency-license policy and CI checks for both supported Mac architectures,
  plus attribution and source-distribution guidance for release maintainers.
- The README includes a color GIF and still screenshots captured from the
  running Ratatui interface, with a reproducible read-only demo tape.
- `diskray installers` (and the `old_installers` MCP tool) reports `.dmg`,
  `.pkg`, `.xip`, and `.iso` files in Downloads and Desktop with their age,
  whether the matching app is already installed, and whether one is mounted.
  Report only.
- `~/.codex/sessions` is identified as Codex conversation transcripts
  (review only). The guidance points to `codex delete`, which removes each
  transcript together with its history records.
- Storage rows show a proportional size bar and each item's share of used
  space, and long paths keep the item's own name (`~/…/target/debug/app`)
  instead of cutting through a folder name.

### Changed

- Local AI no longer claims to require English (United States). Diskray shows
  a language blocker only when the Mac language is unsupported by Apple's
  model or differs from Siri's.
- Investigations tell the model how many tool calls remain, answer a repeated
  identical call with its earlier evidence ID instead of re-running it, and
  retry once when the on-device model is busy.
- On terminals under 24 rows the Ask bar takes one line until you type.
- The storage footer shows `/ ask` instead of a second "browse" command.

### Fixed

- Terminal state is restored after partial setup failures and panics through
  one session guard; panic diagnostics appear after the terminal is restored.
- The demo fixture builder refuses nonempty or symlinked roots instead of
  recursively deleting a caller-supplied directory.
- Public source, issue, and security links point to the actual `mac-cleanup`
  repository; source installation replaces the unavailable Homebrew tap in
  the README. The formula remains an explicitly labeled release template.
- Cleanup goals handle fractional amounts and distinguish requested space from
  space already free. Repeated checks use the latest eligible total, and APFS
  aliases no longer inflate estimates or hide the selected folder. Overviews
  retain the largest measured child even when the AI omits its separate citation.
- Build-output reports reject symlinked configured parents, count overlapping
  roots once, and include recent source activity in unrelated folders whose
  names happen to match build output.
- Failed relocation rollback names recovery paths without falsely claiming
  that both copies still exist.
- Ask cleanup questions report the total eligible cache estimate and any
  shortfall against a requested amount, and expose large measured folders for
  further inspection. The open Explore folder is included as context. Review
  folders never count toward eligible cleanup or acquire cleanup actions.
- Ask growth questions compare saved assessments for the requested period and
  explain missing history. Exhausted tool budgets switch to a report-only
  session, and stale helper-exit events no longer cancel the recovery report.
- On-device tool investigations work with Apple models that reject regex
  generation guides; Rust still validates every handle and evidence reference.
- General Ask questions cannot suggest stopping processes without a targeted
  process investigation. Cleanup questions expose the open-file check within
  the existing six-tool budget.

- The startup volume's capacity came from the sealed system volume, so the
  Data volume was reported as "other volumes" and the capacity figures did not
  add up. Capacity and before/after free space now come from the Data volume.
- In-use caches name the processes that block them.
- Diskray and the terminal running it are no longer listed as findings.
- Storage-only plans no longer spend about 25 seconds sampling CPU activity.

### Safety

- Whitelisted files inside a cleanup target are kept, and native cleanup
  commands are skipped when they would remove one.
- Age-based cleanup checks a folder's whole contents, not just its top level,
  and skips anything held open.
- Recycle bins on network and Windows-formatted volumes are review-only.
- Relocation re-checks the source immediately before replacing it and never
  deletes the only copy during a rollback.

### Renamed and relicensed

- **Mac Cleanup is now Diskray.** The binary is `diskray` and the AI helper is
  `diskray-ai`. On first run, existing data moves automatically from
  `~/Library/Application Support/mac-cleanup`, `~/Library/Logs/mac-cleanup`,
  and `~/.config/mac-cleanup` to the matching `diskray` locations. A location is
  moved only when the new one does not exist yet.
- **License changed from MIT to GPL-3.0-or-later.** Every source file carries an
  SPDX identifier.

### Added

- Selected-folder storage reallocation with detected external drives, free-space
  estimates, keyboard drive selection, and bounded Apple on-device AI advice.
  The reviewed move rechecks capacity and volume identity, preserves Mac file
  metadata, and refuses to overwrite an existing destination. Measured selection
  remains available without Apple Intelligence.

- `diskray why`: a one-screen, screenshot-friendly explanation of what fills
  the disk, including hidden space, quick wins, and growth since an earlier
  assessment (`--json`, `--since DAYS`, `--quick`).
- `diskray ask "<question>"`: the on-device investigation without the
  interface. Exit code 3 means local AI is unavailable (measured facts are still
  printed); 4 means it timed out.
- `diskray mcp`: a read-only Model Context Protocol server for coding agents,
  speaking both the 2025-11-25 and 2026-07-28 protocol versions. Agents can only
  save a cleanup proposal, which `diskray review` re-checks and confirms.
- `diskray artifacts`: a report of build output in projects untouched for 60+
  days, with how to rebuild each. Also on the `why` card and as an MCP tool.
- Cleanup rules are data: bundled TOML rule packs, optional app packs that can
  be disabled, user packs in `~/.config/diskray/rules`, and
  `diskray rules list|check`.
- Redesigned interface after a supervised usability run. The **Overview**
  home screen lists where the space went, largest first, one line per item with
  a plain verdict (*Safe to clear*, *In use by npx*, *App data · review*, *Your
  files*); folders are opened until their names mean something; `a` adds every
  quick win; three tabs (Overview · Explore · History) switch with `1`–`3` or
  `Tab`; one status line replaces the progress band; `:` opens a command
  palette; explanations lead with a plain sentence and what the checks found;
  messages clear on the next key; Explore opens at the home folder.
- `scripts/eval_agent.py`: fixture-based checks of `diskray ask` answers
  (citations exist, suggestions eligible, file contents and symlink targets
  unchanged, prompt injection ignored). Safety checks run during measured
  fallback, with standalone harness regression tests in CI.
- Release engineering: `build-release.sh` skips the AI helper on unsupported
  Macs, a tag-driven release workflow publishes a source tarball and updates
  the Homebrew tap, and `packaging/homebrew/diskray.rb` is the formula.
- Growth tracking: assessments record their volume, and History, the `why`
  card, and a "What grew" finding show significant growth since the previous
  complete assessment. Weekly baselines are kept for 12 weeks.

- On-device tool calling: Apple Foundation Models now calls the app's own
  read-only tools during an investigation. It can list folder children, profile
  file ages, find open files, compare history, check cleanup rules, guess the
  owning app, inspect and sample processes, read memory and disk accounting,
  list mounted volumes, and read bundled Apple notes. Results become numbered
  evidence the report must cite, and Rust validates every report.
- `/` Ask box: one question answered with read-only tools on an answer page.
- **AI SUGGESTS** badges on actions Rust already allows. The user still adds
  and confirms every action.
- One automatic investigation of the top key area after a scan, with a
  three-call budget, memory-pressure pause, and stop on review.
- Tool timelines, questions, and suggestions are kept in private History.
- Unified Findings, Explore, History, and exact-action review plans across
  storage cleanup, process signals, and relocation.
- On-device Apple FoundationModels explanations and bounded investigations,
  with real-work activity visuals and static accessibility modes.
- Progressive assessment, sampled CPU/pressure evidence, private local outcome
  history, persistent results, and before/after observations.
- Swift helper protocol checks and unified-workflow terminal smoke coverage.
- Apple Foundation Models protocol v2 with correlated requests, capability and
  context reporting, token preflight, and dynamically constrained evidence,
  action, hypothesis, and diagnostic IDs.
- Adaptive bounded investigation cases with competing hypotheses and typed
  evidence outcomes across storage, process, filesystem, capacity, and developer
  data; cases and conclusions are retained in private local History.
- Explicit approval for the fixed administrator-assisted `fseventsd` trace and
  saved consent for fixed-catalog Apple documentation research.

### Changed

- Helper protocol v3: one long-lived helper per investigation with concurrent,
  call-ID-routed tool calls. This replaces the one-process-per-step `decide` loop.
  One shared transport replaces five duplicated helper call paths.
- Helper failures are reported in plain language (for example "still preparing
  its model") instead of framework enum names.
- Without a model, investigations run the same tools as a fixed measured
  sequence and label the conclusion as having no AI.
- The automatic investigation no longer starts under elevated pressure and then
  silently never retries.

- Replaced fixed post-insight diagnostic sequencing with one-step Apple FM
  decisions. Rust still owns every collector, timeout, evidence relationship,
  action policy, and final reference validation. Failed or denied checks cannot
  support a cause.
- Added a visible Observe → Choose → Check → Verify → Decide path and live case
  status to the AI pane.

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
