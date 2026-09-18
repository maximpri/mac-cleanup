# Mac Cleanup

[![CI](https://github.com/maximpri/mac-cleanup/actions/workflows/ci.yml/badge.svg)](https://github.com/maximpri/mac-cleanup/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey.svg)

Mac Cleanup brings storage and running-process evidence into one visual care
workspace. Find quick wins, inspect large folders and active workloads, review
exact actions, and keep the measured results. On compatible Macs, Apple
Intelligence adds local explanations and bounded investigations without chat,
a cloud account, or an API key.

Assessment is read-only. Changes require an explicit reviewed plan; `--analyze`
locks the session read-only. Never run the app with `sudo`. AI cannot introduce
delete targets or execute actions. Existing path, ownership, process identity,
whitelist, and in-use safeguards remain authoritative.

The [implementation plan](docs/IMPLEMENTATION_PLAN.md) records the product scope
and outstanding release validation. This implementation is under development;
performance targets and the full model-evaluation matrix are not yet certified.

> [!IMPORTANT]
> The `mac-cleanup` package currently published on crates.io is an unrelated
> project. Install this project only from this repository or its GitHub
> Releases. Registry publication is disabled in this repository to prevent
> accidental name confusion.

## Install and run

Requirements: Rust 1.88 or newer and a normal macOS user account. Local AI requires
Apple silicon, macOS 26+, an SDK containing FoundationModels, and Apple Intelligence
enabled with its model downloaded. If unavailable, measured findings remain usable.

Full Disk Access is optional. If macOS reports `SCAN ERROR` for a protected
user cache you intentionally want to inspect, grant it to the terminal in
System Settings → Privacy & Security → Full Disk Access, then rescan. The app
does not treat an unreadable directory as empty or eligible for cleanup.

```bash
git clone https://github.com/maximpri/mac-cleanup.git
cd mac-cleanup
sh scripts/build-release.sh
./target/release/mac-cleanup
```

Keep `mac-cleanup` and `mac-cleanup-ai` together when installing the release
bundle. A Rust-only build (`cargo build --release` or `cargo install --path .`)
works without generated insights; the interface explains a missing helper.
There is no remote provider fallback.

## Visual workflow

The top menu is **Findings · Explore · History**, with **Review plan** on the
right and one status line below. `Tab` switches between menu and content;
arrow keys choose a menu item and `Enter` returns to content. Menu selection is
one line, with no numbered labels.

**Findings** combines cache measurements, pressure, flagged or busy processes,
and large storage areas. It initially shows up to three Quick wins and five
Key areas; `f` exposes all findings. Critical memory pressure remains visible.
Quick wins use an explicit deterministic cache policy, never a model judgment.
Rows stay in place as new evidence arrives. Storage sizes overlap when a
parent and its descendants are shown; they are not added into a reclaimable total.

The assessment first samples resource activity, then progressively measures
known cleanup targets, temporary retention, incomplete downloads, and the full
storage inventory. A large scan takes time; an unfinished or unreadable scan is
not a complete measurement. `v` shows coverage and unexplained accounting.

**Explore** browses folders largest first. `Enter` opens children; `Left` or
`Backspace` returns. The right pane shows the selected folder's nested storage
map: rectangle area represents allocated space, and colors distinguish branches,
not cleanup safety. Tiny items remain available in the list. Click a rectangle
to explore its target. `i` measures the selected folder again; `o` reveals it
in Finder. No separate terminal commands are required.

**Local AI** first ranks measured key areas and eligible quick wins, then explains
supplied evidence and tradeoffs. Triage can only reorder Rust-approved findings;
it cannot create cleanup targets or change eligibility. `i` on a finding can
request up to three permitted read-only checks: children, current process
readings, open handles, or history comparison. Unsupported references are
rejected, and explanations expire when supporting evidence changes. Inference
has a timeout and pauses under critical memory pressure. A cyan/violet sweep
runs only during actual inference or investigation; completion briefly accents
the result. `M`, `REDUCE_MOTION`, and `NO_COLOR` provide static presentation.
Animation stops when the terminal reports focus loss.

**Review plan** lists exact cleanup paths, process identities and signals, and
relocation destinations with tradeoffs. `Space` adds/removes a finding's action;
`p` reviews. Type `CLEAN` for ordinary cleanup, `APPLY` for signals or relocation,
or `DELETE` for a separate single-item protected-data plan. No changes happen on
selection. Signals run before data actions; if a signal fails, subsequent data
actions are conservatively skipped. Every engine revalidates its targets.
`Esc` returns from review, and `Delete` clears the plan. Mixed plans also require
`s` to acknowledge process-signal consequences and `m` to acknowledge relocation
and symlink consequences before `APPLY` is accepted.

**History** keeps assessment records and per-action outcomes on this Mac in
`~/Library/Application Support/mac-cleanup/sessions`, with private directory/file
permissions, a 30-day retention window, and a 50 MiB cap. It records before/after
CPU, swap-rate, per-device activity, pressure, and free-space observations, plus
regrowth and newly observed matching processes. These do not prove a
causal performance improvement. Results stay open. `r` starts another assessment
once the pending plan is empty. In writable mode, Delete in History opens a
separate `CLEAR` confirmation; the deletion audit log is retained.
When Apple Intelligence is available, a bounded summary of the completed
observations appears beside the raw results; it cannot add actions or targets.

## TUI controls

The interface supports terminals from 60 columns × 16 rows. Narrow windows
stack evidence below the list. Hidden confirmations are blocked below the
minimum size. Colors supplement text labels; `--no-color` is supported.

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Switch menu/content focus. |
| Arrow keys | Choose menu item or list row. |
| `Enter` | Open selected folder or inspect a finding. |
| `Space` | Add/remove selected finding's action. |
| `p` | Review the exact action plan. |
| `i` | Investigate finding with local AI; in Explore, measure selected folder. |
| `e` / `d` | Explore selected finding's contents / expand evidence. |
| `P` | Inspect current-account processes. |
| `m` | Plan relocation to an external volume. |
| `f` / `K` | All/fewer findings; keep selected finding out of this session's list. |
| `v` | Show/hide scan coverage. |
| `o` | Reveal selected path in Finder. |
| `r` | Recheck storage and activity. |
| `PgUp` / `PgDn` | Scroll evidence or review details. |
| `M` / `A` | Toggle motion / open System Settings for Apple Intelligence. |
| `?` | Show the keyboard guide in the evidence pane. |
| `Esc` | Back/cancel; during execution, request stop after current action. |
| `q` | Quit when no action is running. |

## Process review

Press `P` to inspect processes. Resource sampling begins with the unified assessment.
The screen lists current-account processes and high-memory system daemons as
separate read-only observations. A visibly hung app can still be found and
terminated even when macOS reports an ordinary run state. Explicit
abnormal states are sorted to the top and highlighted:

- `STUCK WAIT` is blocked in an uninterruptible kernel wait (`U` or `D`);
- `STOPPED` is suspended by job control, a debugger, or another signal (`T`);
- `DEAD/ZOMBIE` has exited but has not yet been reaped by its parent (`Z`).

`RUNNING` means no explicit abnormal state was reported; it does not prove the
application UI is responsive. Review its command, CPU use, and elapsed time
before acting. The scanner does not infer a hang solely from high CPU use. A
zombie is already dead, so sending it another signal cannot remove it; its
parent must reap it, or the parent/session must be restarted.

For a signalable entry, press `Space` or `d` to add `SIGTERM` to the plan,
or `x` to add an explicit `SIGKILL`. Review the plan and type `APPLY`. No
escalation is automatic. Immediately before signalling, Mac Cleanup checks the
PID, current-account ownership, parent PID, and start time again to guard
against PID reuse. A previously flagged process that recovered is also
protected. Mac Cleanup never signals itself or an ancestor process.
`--analyze` disables process actions, and `--clean --yes` never kills processes
unattended. For `fseventsd`, `i` can request a bounded `sudo -n fs_usage -w -f
filesys fseventsd` sample when an existing authorization is available; the app
never signals the daemon. The investigation can continue with mounted-volume
context and a fixed Apple source catalog. Set `MAC_CLEANUP_RESEARCH=1` to allow
those fixed URLs to be fetched; paths, usernames, and raw logs are never sent
as research queries.

## Scan another volume

Pass `--volume` to select a mounted volume for either the TUI or
line-oriented output and automation. NAS/server-managed recycle directories
are separate from the Trash shown by Finder, so an empty Finder Trash does not
mean a share's `#recycle` or `@Recycle` directory is empty:

```bash
./target/release/mac-cleanup --volume "/Volumes/Work Drive"
./target/release/mac-cleanup --analyze --no-tui --verbose --volume "/Volumes/Work Drive"
```

The path must be an accessible real directory, not a symlink. For example, if a
NAS share uses `/Volumes/DATA/#recycle`, selecting `/Volumes/DATA` reports the
size of that recycle bin. A mounted macOS system volume also checks the matching
current-account home under `Users`, when present. Select a user-home directory
directly with `--volume` when that is the intended layout. Plain output also
reports the detected storage type.

## `/private/tmp` retention

When the startup volume (`/`) is scanned, Mac Cleanup applies a separate
retention policy to direct children of `/private/tmp`. The default threshold is
seven days, based on each entry's modification time; change it with
`--tmp-retention-days <DAYS>`. An entry is eligible only when it is owned by the
current account, its complete directory tree is also owned by that account, and
`lsof` finds no process with the entry open. Top-level symlinks, recent entries,
other users' entries, and anything that cannot be checked are left alone.

Eligible files and directories appear with their exact `/private/tmp/...` paths
in the report and as ordinary `READY` cleanup findings. Analysis is read-only.
`--clean --yes` removes only those individually allowlisted exact paths; it
never removes `/private/tmp` itself. If the open-file check is unavailable, the
policy fails closed and reports zero eligible entries. Rescanning after a
cleanup refreshes the policy.

The same retention threshold drives two more age-based cleanup sources:

- **Incomplete downloads.** `*.crdownload`, `*.part`, and `*.download` entries
  in the account's `Downloads` folder that are owned by the current account,
  untouched past the retention age, and not open by any process become exact
  `READY` findings labeled "Incomplete download". If the open-file check for
  the Downloads folder cannot run, none are offered.
- **Aged directories.** `~/Library/Logs` (7 days), `~/Library/Saved Application
  State` (30 days), and `~/Library/DiagnosticReports` (30 days) keep their
  recent contents and release only direct entries untouched past their
  retention window. Symlinks and newer entries always stay, and the directory
  itself is retained.

## Cache statuses

| Status | Meaning |
| --- | --- |
| `READY` | The allowlisted target exists and all current safeguards passed. Ordinary cache directories are emptied; aged directories release only stale entries; retention candidates are removed as exact paths. |
| `OPTIONAL` | Regenerable, but needs explicit opt-in. Add the highlighted item with `Space`; review its download tradeoff. |
| `REVIEW` | Large app-managed or personal data. Excluded from ordinary and unattended cleanup; clean mode offers guarded single-item deletion. |
| `PROTECTED` | Matches the user whitelist (`~/.config/mac-cleanup/whitelist`); every cleanup path refuses it. |
| `IN USE` | A related application or package manager appears active. |
| `SCAN ERROR` | The directory or a required safety check could not be read completely, so cleanup is disabled. Protected user data may require Full Disk Access. |
| `SYMLINK` | The path or one of its parent components redirects elsewhere. |
| `INVALID` | The allowlisted path is an unexpected file type. |
| `MISSING` | There is nothing to clean at that path (plain `--verbose` output only). |

Close Telegram, Chrome and other Google apps, TradingView, ZCode, package
managers, and development tools before cleanup. A candidate whose related
process is running is automatically unavailable.

## User whitelist

Create `~/.config/mac-cleanup/whitelist` to protect locations from every
cleanup path, including unattended cleanup and confirmed review deletion. Each
non-empty, non-comment line is one path pattern: relative patterns resolve
against the account home directory, a leading `~/` also resolves there, `*`
matches any characters within one path component (`?` matches one character),
matching is case-insensitive, and a matching directory also protects everything
inside it. Both spellings of a symlinked location are honored (for example
`/tmp` and `/private/tmp`).

```text
# never touch these
Library/Caches/Adobe
~/Library/Developer
/private/tmp/my-important-dir
```

## Cleanup levels

Routine cleanup includes:

- user Trash and volume-level Trash, recycle-bin, and temporary folders
- pip, node-gyp, Homebrew, Python, npm package, and OpenCode caches
- Go, uv, Yarn, Xcode derived-data, and Simulator caches
- Safari, Firefox, Google, and Adobe caches; browser profiles and browsing data remain
- TradingView and ZCode updater downloads
- Telegram cached media and thumbnails; messages are not removed
- old log entries, old window-restoration data, and old crash reports (see
  the aged directories above), plus abandoned partial downloads

The `/private/tmp` retention policy is reported separately because it is based
on age and open-file checks rather than a fixed cache location. Its eligible
entries are included in the safe-cleanup total.

The following regenerable downloads are visible but unavailable by default:

- Playwright and Playwright Go browser binaries
- temporary `npx` package installations
- Codex runtimes
- Gradle, SwiftPM, and Hugging Face caches
- CocoaPods downloads and Cypress application binaries

Press `Space` to add the highlighted item to the plan, or start with
`--include-reinstallable`. Using
their associated tools later may trigger a large download.

Large app-managed areas are also shown as `REVIEW` findings when present:

- Chrome DevTools MCP persistent browser profile (sessions, cookies, and site data)
- Xcode archives, iOS device-support files, and Simulator devices
- Cursor user and workspace data
- OrbStack containers, images, machines, and volumes
- Telegram local account and media data

These locations can contain valuable state. They are never included in normal
selection, “select all,” or unattended cleanup and should preferably be reduced
using the controls in Xcode, Cursor, OrbStack, or Telegram. Clean mode also
offers advanced deletion for one highlighted `REVIEW` item at a time: highlight
it and press `Space`, open the plan with `p`, inspect the exact path and impact,
then type `DELETE`. Protected review data requires its own single-item plan. This
permanently clears everything inside that app-managed directory and can remove
settings, history, containers, simulator apps, or local account data. The
related app must be closed, and the same path, symlink, type, and allowlist
checks are repeated immediately before deletion.

### Native-command cleanup

When a vendor provides a non-interactive command that can be confined to the
current account's matching cache, Mac Cleanup runs it before touching files
directly:

| Finding | Native command used first |
| --- | --- |
| pip | `python3 -m pip cache purge` |
| npm packages / npx packages | `npm cache clean --force` / `npm cache npx rm` |
| Go / uv / Yarn | `go clean -cache -testcache -fuzzcache` / `uv cache clean` / `yarn cache clean` |
| SwiftPM | `swift package purge-cache` |
| Playwright | `playwright uninstall --all` |
| CocoaPods / Cypress | `pod cache clean --all` / `cypress cache clear` |
| Hugging Face | `hf cache prune --yes` |
| Simulator devices | `xcrun simctl delete all` |
| OrbStack data | `orbctl reset --yes` |

Cache-directory flags or documented environment variables pin commands to the
allowlisted path where the tool supports them. Commands have a 30-minute
timeout and run in the background so the TUI remains responsive. If a command
is unavailable, fails, or leaves files behind, the existing symlink-safe
exact-path cleanup removes only those remaining contents. The details screen
shows the planned strategy, and the TUI and line-oriented output report what
actually ran after cleanup.

Candidates without a safe, exact-scope command use only the guarded path
cleanup. For example, `brew cleanup --prune=all` can affect installed formulae
and locations beyond the displayed Homebrew cache, `xcodebuild clean` requires
a specific project, and Gradle performs retention cleanup during builds rather
than offering a global cache-clear command. Mac Cleanup does not broaden the
deletion scope merely to use a command.

## Automation and plain output

When input or output is redirected, the app automatically emits line-oriented
plain text. `--no-tui` forces that behavior. The default plain-output scope is
the local startup volume; pass `--volume` to inspect a specific home or mounted
volume. Findings are ordered largest-first, and analysis remains read-only:

```bash
./target/release/mac-cleanup --analyze --no-tui --verbose
```

For scripts and inventory tools, `--json` emits a versioned report and implies
non-interactive output. Sizes are allocated filesystem kilobytes, paths are
exact, and cleanup outcomes are included when cleanup was requested:

```bash
./target/release/mac-cleanup --json | jq '.storage_inventory, .summary, (.items[] | select(.size_kb > 0))'
./target/release/mac-cleanup --clean --yes --json > cleanup-report.json
```

The top-level `schema_version` is currently `5`. Runtime scan and validation
errors remain machine-readable after argument parsing. `--json` cannot be
combined with `--verbose`. Reports include the scan location and storage type,
a `storage_inventory` with filesystem capacity/usage, APFS container free space
when `diskutil` reports it, the dates of local Time Machine snapshots
(`local_snapshots`, report-only), scan coverage, top-level
usage, and largest review-only paths; aggregate cleanable/opt-in/review/protected
totals; every allowlisted item; a `temp_retention` report with the threshold, exact
eligible paths, skipped-entry counts, and safety-check status; a
`process_review` inventory; and—when applicable—the result of each cleanup
attempt. A positive
`storage_inventory.unaccounted_kb` means filesystem usage was not visible in
the directory walk; on APFS this can include snapshots or protected/system-
managed data, and a partial scan can also contribute.

For unattended cleanup, `--yes` clears every currently eligible item without
opening the TUI. With `--relocate`, it confirms the explicitly requested move
instead. Path, symlink, type, allowlist, and process checks still apply:

```bash
./target/release/mac-cleanup --clean --yes
./target/release/mac-cleanup --clean --include-reinstallable --yes
```

Because `--yes` permanently deletes contents without individual selection,
review a fresh analysis immediately beforehand.

## Options

```text
--analyze                 Read-only report (default)
--clean                   Select and clear eligible removable-data contents
--relocate <PATH>         Preview or relocate one large user-owned directory
--relocate-to <PATH>      Existing destination directory on an external volume
--include-reinstallable   Include items that may need a large download
--tmp-retention-days <DAYS>
                          Treat user-owned /private/tmp entries as stale after this many days (default: 7)
--volume <PATH>           Scan known waste and inventory this volume or home directory
--yes                     Confirm cleanup or relocation without the TUI
--verbose                 Show missing items and exact paths in plain output
--json                    Emit a versioned machine-readable report
--no-color                Disable colored output
--no-tui                  Force line-oriented output
-h, --help                Show command help
-V, --version             Show the application version
```

## Relocating large directories

For a large directory that is useful but does not need startup-disk
performance, use the explicit relocation command. It requires an existing
directory on a mounted volume under `/Volumes`; the destination must be on a
different filesystem and network volumes are rejected. The source must be a
real directory inside the current account's home or a child of
`/private/tmp`, with no symlinks, special files, other-user-owned entries, open
file handles, or running owning application.

The command first copies the directory to `<destination>/<source-name>`,
compares the copied tree with the source, then renames the source to a private
temporary backup and creates an absolute symlink at the original path. The
backup is removed only after the link is verified. A failed copy or validation
leaves the original directory untouched. Without `--yes`, the command only
prints the plan:

```bash
./target/release/mac-cleanup \
  --relocate "$HOME/Library/Developer/Xcode/iOS DeviceSupport" \
  --relocate-to "/Volumes/EXT_DISK/MacCleanup"
```

After reviewing the source, destination, size, and file count, perform it with:

```bash
./target/release/mac-cleanup \
  --relocate "$HOME/Library/Developer/Xcode/iOS DeviceSupport" \
  --relocate-to "/Volumes/EXT_DISK/MacCleanup" \
  --yes
```

The original path remains usable through the symlink, while new data is stored
on the external volume. Keep that volume mounted before starting the owning
application. Relocation is never part of `--clean --yes` and is never inferred
from the largest-consumer inventory.

## Safety model

- Analysis is the default and cannot modify files.
- `REVIEW` findings cannot enter ordinary selection or unattended cleanup.
  Advanced TUI deletion is single-item only and requires a typed confirmation.
- Cleanup refuses to run as root or through `sudo`.
- Only exact known waste paths constructed for the selected location can be
  cleared.
- Volume roots are canonicalized, and symlink roots are rejected.
- The app rejects a candidate if it or any path component is a symlink.
- Related running applications and package managers block cleanup. The process
  check is repeated immediately before deletion.
- Ordinary cache directories are retained while only their contents are removed;
  `/private/tmp` retention candidates are exact paths and are removed only after
  ownership and open-file checks are repeated immediately before deletion.
- Every cleanup target's filesystem identity (device and inode numbers of the
  path and its parent) is captured during the scan and re-verified at the
  deletion step itself. A path that was deleted and recreated, renamed, or
  swapped after the scan is refused, and a target whose identity could not be
  captured is never cleaned.
- Whitelisted paths are refused by every cleanup path; see the user whitelist
  section above.
- Symlinks inside an eligible cache are unlinked without following them.
- Every cleanup attempt (cleared, skipped, or failed) is appended to
  `~/Library/Logs/mac-cleanup/deletions.log` with the timestamp, reclaimed
  size, and exact path, so permanent deletions remain auditable.
- Large runtimes and browser binaries require explicit opt-in.
- The full-volume inventory is read-only and review-only; it never turns
  personal files, application data, or large files into cleanup candidates.
- Ordinary cleanup excludes documents, projects, messages, browser profiles,
  Application Support, containers, and macOS system files. A few explicitly
  listed `REVIEW` locations can be cleared only through the separate typed
  confirmation described above.
- `/private/tmp` retention is limited to old, current-account-owned, unopened
  direct children; an inability to prove eligibility leaves the entry untouched.
- Relocation is a separate explicit operation: it copies and verifies one
  directory before replacing its original path with a symlink; it is never
  included in automatic cleanup.

Deletion is permanent rather than moving data to Trash because files in Trash
continue occupying disk space. The final summary distinguishes measured cache
contents removed from the filesystem free-space change; APFS accounting and
concurrent app activity can make those values differ slightly.

## Interface preview

![Redesigned storage workspace](docs/images/storage.png)

The preview uses synthetic data. [Unified care workspace](docs/images/care.svg).

## Architecture

See the [performance and cleanup journey review](docs/PRODUCT_REVIEW.md) for
current product findings and the proposed workflow, and
[architecture](docs/ARCHITECTURE.md) for module boundaries and safety invariants.

## Contributing and security

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) before
changing cleanup paths or deletion behavior; those changes have additional
safety and test requirements. User-visible changes are tracked in
[CHANGELOG.md](CHANGELOG.md).

Do not report path escapes, unintended deletion, confirmation bypasses, or
other vulnerabilities in a public issue. Follow [SECURITY.md](SECURITY.md) to
submit them privately. Community participation is governed by
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## License

Mac Cleanup is available under the [MIT License](LICENSE).
