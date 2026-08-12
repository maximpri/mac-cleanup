# Mac Cleanup

Mac Cleanup is a conservative macOS cache cleaner with a full-screen
[Ratatui](https://ratatui.rs/) interface. It scans a fixed allowlist of known,
regenerable caches and makes no changes unless cleanup mode is explicitly
requested and the user confirms a selection.

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
the startup volume, or any mounted volume listed under `/Volumes`. The selected
location becomes the root for the same fixed relative cache allowlist; the app
does not recursively classify unrelated files as caches.

## TUI controls

| Key | Action |
| --- | --- |
| `↑` / `↓` or `j` / `k` | Move through cache entries. |
| `Space` | Select or deselect the highlighted eligible cache. |
| `a` | Select or deselect all eligible caches. |
| `i` | Toggle large reinstallable items and rescan. |
| `v` | Return to the home/volume picker. |
| `r` | Rescan all allowlisted locations. |
| `Enter` | Review the selected items for cleanup. |
| `y` | Confirm the permanent deletion in the confirmation dialog. |
| `n` or `Esc` | Cancel the confirmation dialog. |
| `q` | Quit, or stop after the current item while cleaning. |

Analysis mode supports navigation, details, reinstallable-item toggling, and
rescanning, but does not expose selection or cleanup actions.

## Scan another volume

Use the location picker in the TUI, or pass the mounted volume path explicitly
for line-oriented output and automation:

```bash
./mac-cleanup.sh --volume "/Volumes/Work Drive"
./mac-cleanup.sh --analyze --no-tui --verbose --volume "/Volumes/Work Drive"
```

The path must be an accessible real directory, not a symlink. A volume selection
changes only the root used to construct known cache paths. For example, the pip
candidate on `/Volumes/Work Drive` is
`/Volumes/Work Drive/Library/Caches/pip`. Select a user-home directory directly
with `--volume` when that is the intended layout.

## Cache statuses

| Status | Meaning |
| --- | --- |
| `READY` | The directory exists and all current safeguards passed. |
| `OPTIONAL` | Regenerable, but needs reinstallable-item opt-in. |
| `IN USE` | A related application or package manager appears active. |
| `SYMLINK` | The path or one of its parent components redirects elsewhere. |
| `INVALID` | The allowlisted path is an unexpected file type. |
| `MISSING` | There is nothing to clean at that path. |

Close Telegram, Chrome and other Google apps, TradingView, ZCode, package
managers, and development tools before cleanup. A candidate whose related
process is running is automatically unavailable.

## Cleanup levels

Routine cleanup includes:

- pip, node-gyp, Homebrew, Python, npm package, and OpenCode caches
- TradingView and ZCode updater downloads
- Telegram cached media and thumbnails; messages are not removed
- Google application and browser caches; profiles are not removed

The following regenerable downloads are visible but unavailable by default:

- Playwright and Playwright Go browser binaries
- temporary `npx` package installations
- Chrome DevTools MCP downloads
- Codex runtimes

Press `i` in the TUI or start with `--include-reinstallable` to make those items
eligible. Using their associated tools later may trigger a large download.

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
--clean                   Select and clear eligible cache contents
--include-reinstallable   Include browser binaries and tool runtimes
--volume <PATH>           Scan known cache paths relative to this volume
--yes                     Clear all eligible items without the TUI
--verbose                 Show missing items and exact paths in plain output
--no-color                Disable colored output
--no-tui                  Force line-oriented output
-h, --help                Show command help
-V, --version             Show the application version
```

## Safety model

- Analysis is the default and cannot modify files.
- Cleanup refuses to run as root or through `sudo`.
- Only exact paths constructed from the selected root and compiled allowlist can
  be cleared.
- Volume roots are canonicalized, and symlink roots are rejected.
- The app rejects a cache if it or any path component is a symlink.
- Related running applications and package managers block cleanup. The process
  check is repeated immediately before deletion.
- The cache directory itself is retained; only its contents are removed.
- Symlinks inside an eligible cache are unlinked without following them.
- Large runtimes and browser binaries require explicit opt-in.
- Documents, projects, messages, browser profiles, Application Support,
  containers, and macOS system files are outside the allowlist.

Deletion is permanent rather than moving data to Trash because files in Trash
continue occupying disk space. The final summary distinguishes measured cache
contents removed from the filesystem free-space change; APFS accounting and
concurrent app activity can make those values differ slightly.
