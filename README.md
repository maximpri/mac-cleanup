# Mac Cleanup

Mac Cleanup finds disk space that can be reclaimed safely: files already in
Trash or a volume recycle bin, temporary data, development artifacts, package
caches, and other regenerable downloads. It also identifies large app-managed
storage that may be worth reviewing without treating it as disposable. Its full-screen
[Ratatui](https://ratatui.rs/) interface makes no changes unless cleanup mode is
explicitly requested and the user confirms a selection.

The default mode is read-only. Do not run the app with `sudo`.

## Run it

Requirements: macOS, Rust 1.88 or newer, and a normal user account.

```bash
# Build once, then run the optimized binary
cargo build --release
./target/release/mac-cleanup

# Or use the source-checkout launcher (Cargo builds automatically)
./mac-cleanup.sh
```

Open the interactive cleanup screen with:

```bash
./mac-cleanup.sh --clean
```

The app scans each allowlisted location, shows its size and safety status, and
lets you inspect every exact path before selecting anything. Cleanup uses a
separate permanent-deletion confirmation dialog and rechecks each selected item
immediately before touching it.

Interactive runs begin with a scan-location picker. Choose the home directory,
the startup volume, or any mounted volume listed under `/Volumes`. Home scans
look for user Trash and regenerable caches. Volume scans look for real
volume-level waste such as `.Trashes`, `#recycle`, `@Recycle`, `$RECYCLE.BIN`,
and `.TemporaryItems`. Personal files are never inferred to be waste.

## TUI controls

| Key | Action |
| --- | --- |
| `↑` / `↓` or `j` / `k` | Move through findings. |
| `Space` | Select or deselect the highlighted eligible item. |
| `a` | Select or deselect all eligible items. |
| `i` | Toggle large reinstallable items and rescan. |
| `v` | Return to the home/volume picker. |
| `r` | Rescan all allowlisted locations. |
| `Enter` | Show details for the highlighted finding, or review selected cleanup items. |
| `y` | Confirm the permanent deletion in the confirmation dialog. |
| `n` or `Esc` | Cancel the confirmation dialog. |
| `q` | Quit, or stop after the current item while cleaning. |

Analysis mode supports navigation, an Enter-opened details dialog,
reinstallable-item toggling, and rescanning, but does not expose selection or
cleanup actions.

## Scan another volume

Use the location picker in the TUI, or pass the mounted volume path explicitly
for line-oriented output and automation:

```bash
./mac-cleanup.sh --volume "/Volumes/Work Drive"
./mac-cleanup.sh --analyze --no-tui --verbose --volume "/Volumes/Work Drive"
```

The path must be an accessible real directory, not a symlink. For example, if a
NAS share uses `/Volumes/DATA/#recycle`, selecting `/Volumes/DATA` reports the
size of that recycle bin. A mounted macOS system volume also checks the matching
current-account home under `Users`, when present. Select a user-home directory
directly with `--volume` when that is the intended layout.

## Cache statuses

| Status | Meaning |
| --- | --- |
| `READY` | The directory exists and all current safeguards passed. |
| `OPTIONAL` | Regenerable, but needs reinstallable-item opt-in. |
| `REVIEW` | Large app-managed or personal data. Inspect it in the named app; Mac Cleanup never deletes it. |
| `IN USE` | A related application or package manager appears active. |
| `SYMLINK` | The path or one of its parent components redirects elsewhere. |
| `INVALID` | The allowlisted path is an unexpected file type. |
| `MISSING` | There is nothing to clean at that path (plain `--verbose` output only). |

Close Telegram, Chrome and other Google apps, TradingView, ZCode, package
managers, and development tools before cleanup. A candidate whose related
process is running is automatically unavailable.

## Cleanup levels

Routine cleanup includes:

- user Trash and volume-level Trash, recycle-bin, and temporary folders
- pip, node-gyp, Homebrew, Python, npm package, and OpenCode caches
- Go, uv, Yarn, Xcode derived-data, and Simulator caches
- TradingView and ZCode updater downloads
- Telegram cached media and thumbnails; messages are not removed
- Google application and browser caches; profiles are not removed

The following regenerable downloads are visible but unavailable by default:

- Playwright and Playwright Go browser binaries
- temporary `npx` package installations
- Chrome DevTools MCP downloads
- Codex runtimes
- Gradle caches and Hugging Face models

Press `i` in the TUI or start with `--include-reinstallable` to make those items
eligible. Using their associated tools later may trigger a large download.

Large app-managed areas are also shown as `REVIEW` findings when present:

- Xcode iOS device-support files and Simulator devices
- Cursor user and workspace data
- OrbStack containers, images, machines, and volumes
- Telegram local account and media data

These locations can contain valuable state. They are never selectable, are
refused by both interactive and unattended cleanup, and should be reduced using
the controls in Xcode, Cursor, OrbStack, or Telegram.

## Automation and plain output

When input or output is redirected, the app automatically emits line-oriented
plain text. `--no-tui` forces that behavior. Analysis remains read-only:

```bash
./mac-cleanup.sh --analyze --no-tui --verbose
```

For unattended cleanup, `--yes` clears every currently eligible item without
opening the TUI. Path, symlink, type, allowlist, and process checks still apply:

```bash
./mac-cleanup.sh --clean --yes
./mac-cleanup.sh --clean --include-reinstallable --yes
```

Because `--yes` permanently deletes contents without individual selection,
review a fresh analysis immediately beforehand.

## Options

```text
--analyze                 Read-only report (default)
--clean                   Select and clear eligible removable-data contents
--include-reinstallable   Include items that may need a large download
--volume <PATH>           Scan known waste on this volume or home directory
--yes                     Clear all eligible items without the TUI
--verbose                 Show missing items and exact paths in plain output
--no-color                Disable colored output
--no-tui                  Force line-oriented output
-h, --help                Show command help
-V, --version             Show the application version
```

## Safety model

- Analysis is the default and cannot modify files.
- `REVIEW` findings are informational and cannot be selected or deleted.
- Cleanup refuses to run as root or through `sudo`.
- Only exact known waste paths constructed for the selected location can be
  cleared.
- Volume roots are canonicalized, and symlink roots are rejected.
- The app rejects a candidate if it or any path component is a symlink.
- Related running applications and package managers block cleanup. The process
  check is repeated immediately before deletion.
- The candidate directory itself is retained; only its contents are removed.
- Symlinks inside an eligible cache are unlinked without following them.
- Large runtimes and browser binaries require explicit opt-in.
- Documents, projects, messages, browser profiles, Application Support,
  containers, and macOS system files are outside the allowlist.

Deletion is permanent rather than moving data to Trash because files in Trash
continue occupying disk space. The final summary distinguishes measured cache
contents removed from the filesystem free-space change; APFS accounting and
concurrent app activity can make those values differ slightly.
