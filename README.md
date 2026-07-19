# Safe macOS cache cleanup

`mac-cleanup.sh` reports the size and safety status of known regenerable caches,
then clears only exact allowlisted directories when explicitly requested. Its
default mode is read-only.

Requirements: macOS and Bash 3.2 or newer. Do not run the script with `sudo`.

## Quick start

```bash
chmod +x mac-cleanup.sh

# Read-only analysis
./mac-cleanup.sh

# Interactive cleanup of routine caches
./mac-cleanup.sh --clean
```

Close Telegram, Chrome and other Google apps, TradingView, ZCode, package
managers, and development tools before cleanup. If a related process is still
running, that candidate is skipped automatically.

## Cleanup levels

Routine cleanup includes:

- pip, node-gyp, Homebrew, Python, npm package, and OpenCode caches
- TradingView and ZCode updater downloads
- Telegram cached media and thumbnails; messages are not removed
- Google application and browser caches; profiles are not removed

The following items are reported but not cleaned by default because using their
tools later can require a large download:

- Playwright and Playwright Go browser binaries
- temporary `npx` package installations
- Chrome DevTools MCP downloads
- Codex runtimes

Include those items with:

```bash
./mac-cleanup.sh --clean --include-reinstallable
```

The script normally asks before each deletion. For unattended use, `--yes`
accepts every eligible candidate, while process, path, and symlink checks remain
active:

```bash
./mac-cleanup.sh --clean --yes
./mac-cleanup.sh --clean --include-reinstallable --yes
```

Because `--yes` causes permanent deletion without individual prompts, run the
read-only report immediately beforehand and review its `Eligible in this run`
total.

## Understanding the report

| Status | Meaning |
| --- | --- |
| `ready` | The directory exists and all current safeguards passed. |
| `opt-in` | Regenerable, but requires `--include-reinstallable`. |
| `SKIP app/process running` | A related application or package manager appears active. |
| `SKIP symlink` | The cache path redirects elsewhere and will never be cleared. |
| `SKIP not a directory` | The allowlisted path is an unexpected file type. |
| `not present` | There is nothing to clean at that path. |

Use `--verbose` to display every exact allowlisted path, including missing
ones:

```bash
./mac-cleanup.sh --analyze --verbose
```

## Safety model

- Analysis is the default and does not modify files.
- Cleanup refuses to run as root or through `sudo`.
- Only exact paths coded into the script's allowlist can be cleared.
- Cache-directory symlinks are refused so deletion cannot be redirected.
- Related running applications and package managers cause an item to be
  skipped. The process check is repeated immediately before deletion.
- The cache directory itself is retained; only its contents are removed.
- Large runtimes and browser binaries require `--include-reinstallable`.
- Documents, projects, messages, browser profiles, Application Support,
  containers, and macOS system files are outside the allowlist.

Deletion is permanent rather than moving data to Trash because files in Trash
continue occupying disk space. Cleared caches and downloaded tools can be
recreated by their applications, but deleted cache contents cannot be restored
by this script.

## Options

```text
--analyze                 Read-only report (default)
--clean                   Clean eligible caches after confirmation
--include-reinstallable   Include browser binaries and tool runtimes
--yes                     Confirm all eligible candidates
--verbose                 Show skipped and missing candidates and their paths
-h, --help                Show command help
```

The final summary distinguishes the measured cache contents removed from the
change in filesystem free space. These values can differ slightly because apps
may create files during cleanup and APFS updates free-space accounting
asynchronously.
