# Safety

Diskray's promise is that measuring never changes anything, and nothing is
deleted, moved, or signalled until you have reviewed the exact target and typed
a confirmation. This page describes every safeguard behind that promise.

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
  path and its parent) is captured during the scan and checked again just
  before deletion. A path that was deleted and recreated, renamed, or swapped
  after the scan is refused, and a target whose identity could not be captured
  is never cleaned. The check uses `lstat`, not an open directory handle, so a
  very narrow window remains between the check and the removal; the recursive
  removal itself never follows symlinks.
- Whitelisted paths are never removed: a whitelisted target is skipped, and a
  whitelisted file or folder inside a cleanup target is kept while the rest is
  cleared. See [User whitelist](#user-whitelist).
- Symlinks inside an eligible cache are unlinked without following them.
- Every cleanup attempt (cleared, skipped, or failed) is appended to
  `~/Library/Logs/diskray/deletions.log` with the timestamp, reclaimed
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

## Cache statuses

| Status | Meaning |
| --- | --- |
| `READY` | The allowlisted target exists and all current safeguards passed. Ordinary cache directories are emptied; aged directories release only stale entries; retention candidates are removed as exact paths. |
| `OPTIONAL` | Regenerable, but needs explicit opt-in. Add the highlighted item with `Space`; review its download tradeoff. |
| `REVIEW` | Large app-managed or personal data. Excluded from ordinary and unattended cleanup; clean mode offers guarded single-item deletion. |
| `PROTECTED` | Matches the user whitelist (`~/.config/diskray/whitelist`); every cleanup path refuses it. |
| `IN USE` | A related application or package manager appears active. |
| `SCAN ERROR` | The directory or a required safety check could not be read completely, so cleanup is disabled. Protected user data may require Full Disk Access. |
| `SYMLINK` | The path or one of its parent components redirects elsewhere. |
| `INVALID` | The allowlisted path is an unexpected file type. |
| `MISSING` | There is nothing to clean at that path (plain `--verbose` output only). |

Close Telegram, Chrome and other Google apps, TradingView, ZCode, package
managers, and development tools before cleanup. A candidate whose related
process is running is automatically unavailable.

## User whitelist

Create `~/.config/diskray/whitelist` to protect locations from every
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

- user Trash, and volume-level Trash and temporary folders on local and USB
  volumes (temporary folders only when no file inside is open)
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

When a vendor provides a non-interactive command that can be pinned to the
current account's exact cache directory, Diskray runs it instead of
removing files directly:

| Finding | Native command |
| --- | --- |
| pip cache | `python3 -m pip cache purge` |
| Go build cache | `go clean -cache -testcache -fuzzcache` |
| uv package cache | `uv cache clean` |
| Yarn package cache | `yarn cache clean` |
| Playwright browsers | `playwright uninstall --all` |
| Cypress runtimes | `cypress cache clear` |
| Hugging Face models | `hf cache prune --yes` |

Cache-directory flags or documented environment variables pin each command to
the allowlisted path. Commands have a 30-minute timeout and run in the
background so the TUI remains responsive.

- If the tool is **not installed**, the guarded exact-path cleanup is used instead.
- If the command **fails**, cleanup stops there; nothing else is swept.
- If it **succeeds**, whatever it deliberately kept stays.
- If your whitelist names something **inside** the cache, the command is not
  run at all, and the guarded cleanup keeps the protected items.

npm, npx, SwiftPM, CocoaPods, Simulator devices (`xcrun simctl delete all`),
and OrbStack (`orbctl reset`) have commands whose scope is broader than the
displayed path. The details screen shows them as advice only; Diskray never
runs them. Likewise `brew cleanup --prune=all`, `xcodebuild clean`, and Gradle's
build-time retention are not used, because they reach beyond the exact cache.

## `/private/tmp` retention and age-based cleanup

When the startup volume (`/`) is scanned, Diskray applies a separate
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
  recent contents. A direct entry is released only when it and everything
  inside it are untouched past the retention window and no process has it
  open; if the open-file check cannot run, nothing is removed. Symlinks always
  stay, and the directory itself is retained.

## Other volumes and recycle bins

Pass `--volume` to select a mounted volume for either the TUI or
line-oriented output and automation. NAS/server-managed recycle directories
are separate from the Trash shown by Finder, so an empty Finder Trash does not
mean a share's `#recycle` or `@Recycle` directory is empty:

```bash
./target/release/diskray --volume "/Volumes/Work Drive"
./target/release/diskray --analyze --no-tui --verbose --volume "/Volumes/Work Drive"
```

The path must be an accessible real directory, not a symlink. For example, if a
NAS share uses `/Volumes/DATA/#recycle`, selecting `/Volumes/DATA` reports the
size of that recycle bin. Server- and Windows-managed recycle bins can hold other
people's deleted files, so they are review-only and never part of routine
cleanup; empty them from the NAS or Windows. On a network share, every
volume-level folder (Trash, temporary items) is review-only too. A mounted macOS system volume also checks the matching
current-account home under `Users`, when present. Select a user-home directory
directly with `--volume` when that is the intended layout. Plain output also
reports the detected storage type.

## Process actions

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
escalation is automatic. Immediately before signalling, Diskray checks the
PID, current-account ownership, parent PID, and start time again to guard
against PID reuse. A previously flagged process that recovered is also
protected. Diskray never signals itself or an ancestor process.
`--analyze` disables process actions, and `--clean --yes` never kills processes
unattended. For `fseventsd`, `i` can propose a fixed eight-second
`fs_usage -w -f filesys -t 8 fseventsd` sample. The app waits for `a`; macOS then
owns the administrator authorization prompt. Declining or failing authorization
is recorded as unusable evidence, and the case continues with mounted-volume
context or bundled Apple reference notes. The app never signals the daemon.

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
./target/release/diskray \
  --relocate "$HOME/Library/Developer/Xcode/iOS DeviceSupport" \
  --relocate-to "/Volumes/EXT_DISK/Diskray"
```

After reviewing the source, destination, size, and file count, perform it with:

```bash
./target/release/diskray \
  --relocate "$HOME/Library/Developer/Xcode/iOS DeviceSupport" \
  --relocate-to "/Volumes/EXT_DISK/Diskray" \
  --yes
```

The original path remains usable through the symlink, while new data is stored
on the external volume. Keep that volume mounted before starting the owning
application. Relocation is never part of `--clean --yes` and is never inferred
from the largest-consumer inventory.


## Manually selected Move to Trash

The folder browser's `t` action queues the entire selected item for native macOS
Trash. It requires a separate plan and typed `TRASH` confirmation. It cannot mix
with cleanup actions, including emptying Trash. Moved bytes are not counted as
freed space. Restore items by dragging them out of Trash in Finder; Put Back may
be unavailable. No permanent-delete fallback is attempted.

This action is limited to owned files/folders on the home volume, inside the
home directory, without symlinked paths. Home and standard folder anchors,
Library data other than individual user caches, authentication directories,
Diskray's working-directory ancestors, and whitelist-protected paths (including
protected descendants) are refused. The path policy and file/parent identity
are checked again at execution. Read-only mode blocks selection. AI and MCP
have no access to this action. Selecting an item does not mean Diskray judged
it disposable: the review explicitly warns that apps using it may stop working.
