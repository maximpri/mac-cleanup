# Using Diskray

## Commands

| Command | What it does |
| --- | --- |
| `diskray` | Opens the interactive workspace |
| `diskray why` | One-screen explanation: capacity, hidden space, largest folders, quick wins, what grew. `--json` for scripts |
| `diskray ask "why is my disk full?"` | One question answered by the on-device model using read-only tools |
| `diskray artifacts` | Build output (`node_modules`, `target`, `.venv`, …) in projects untouched for 60+ days. Report only |
| `diskray mcp` | Read-only tools for coding agents over MCP ([docs/MCP.md](MCP.md)) |
| `diskray review` | Review a cleanup proposal an agent saved, with the usual confirmation |
| `diskray rules list` / `check FILE` | Show or validate cleanup rule packs ([docs/RULES.md](RULES.md)) |

None of the subcommands delete anything. `diskray artifacts` searches every
top-level folder in your home except media, apps, and sync folders; set
`roots = ["code", "work"]` and `min_age_days = 90` in
`~/.config/diskray/projects.toml` to narrow it.

## The interactive workspace

The workspace has two persistent panels. The left lists storage items, folders,
or saved scans; the right explains the selection and holds investigations and
plan review. Ask AI spans the full width below both panels. `F3` shows the
automatically detected Apple model and any readiness blocker.
`Tab` moves keyboard focus between the panels. `:` opens every command.

**Storage findings** (`g`) answer "where did my space go?": a capacity bar shows
measured folders, unseen space, macOS and other volumes, and free space. Items
appear largest first, with size and status on separate lines. Each has a plain
verdict such as *Safe to clear*,
*In use by npx*, *App data · review*, or *Your files*. `Enter` / `Right` opens folder contents,
`Space` adds it to your plan, `a` adds every quick win, and `p` reviews the
plan. Unreadable locations, small items, and ordinary running processes are
left out until you press `f`.

The assessment samples resource activity independently while progressively measuring
known cleanup targets, temporary retention, incomplete downloads, and the full
storage inventory. A large scan takes time; an unfinished or unreadable scan is
not a complete measurement. `v` shows coverage and unexplained accounting.

**Browse folders** (`b`, or `e` for the selected item) lists children largest first. `Enter` opens children; `Left` or
`Backspace` goes to the containing folder. `Esc` closes details, then returns to
the previous list and selection. The right pane shows the selected folder's nested storage
map: rectangle area represents allocated space, and colors distinguish branches,
not cleanup safety. Tiny items remain available in the list. Click a rectangle
to explore its target. `i` measures the current folder again; `o` reveals it
in Finder. Missing folder measurements start automatically, with progress and
explicit partial totals. `Space` queues cleanup for an exact supported rule.
`t` queues a personal file/folder (or an individual user cache) for **Move to
Trash**. `p` reviews it, then type `TRASH` to confirm. This is a separate plan
from cache cleanup, and **does not free space until Trash is emptied**. Restore
an item by dragging it out of Trash in Finder; Put Back may be unavailable.
Protected/system locations, redirects, and whitelist entries remain blocked.
No separate terminal commands are required.

**Review plan** lists exact cleanup paths, process identities and signals, and
relocation destinations with tradeoffs. `Space` adds/removes a finding's action;
`p` reviews. Type `CLEAN` for ordinary cleanup, `APPLY` for signals or relocation,
or `DELETE` for a separate single-item protected-data plan. No changes happen on
selection. Signals run before data actions; if a signal fails, subsequent data
actions are conservatively skipped. Every engine revalidates its targets.
`Esc` returns from review, and `Delete` clears the plan. Mixed plans also require
`s` to acknowledge process-signal consequences and `m` to acknowledge relocation
and symlink consequences before `APPLY` is accepted.

**History** (`h`) keeps assessment records and per-action outcomes on this Mac in
`~/Library/Application Support/diskray/sessions`, with private directory/file
permissions, a 30-day retention window, and a 50 MiB cap. It records before/after
CPU, swap-rate, per-device activity, pressure, and free-space observations, plus
regrowth and newly observed matching processes. These do not prove a
causal performance improvement. Results stay open. `r` starts another assessment
once the pending plan is empty. In writable mode, Delete in History opens a
separate `CLEAR` confirmation; the deletion audit log is retained.
When Apple Intelligence is available, a bounded summary of the completed
observations appears beside the raw results; it cannot add actions or targets.

## Keys

The interface supports terminals from 60 columns × 16 rows. Both panels stay
visible at every supported size. `Tab` focuses the explanation;
`Esc` returns focus to the list. The mouse wheel scrolls the panel under it.
`PgUp` / `PgDn` pages the focused list or scrolls details; `Home` / `End` moves
to its beginning or end. Clicking a heatmap preserves the correct return path.
An explanation starts with what was measured, then what the checks found,
what you can do, and how it was checked. Folder bars and maps show measured size.
The right panel also shows a **contents heatmap** for the selected storage item.
Click a folder rectangle to browse it or a file rectangle to inspect its details.
The map follows selection, keeps Ask visible, and uses rectangle area for size.

The storage list has a **SPACE BALANCE**: listed areas + other measured files +
unaccounted usage + other APFS volumes = total used. Rows come from one folder
walk and do not double-count nested cleanup targets. macOS-managed files are
included in measured areas. Any measurement excess appears as a negative
correction; it is not reclaimable space. `v` shows exact KiB totals and scan
coverage. Small terminals show a compact subtotal with the full balance on `v`.

**Ask AI** stays visible below both panels. Press `/` or click it to type,
then `Enter` to ask. `Esc` or `Tab` returns to browsing and preserves your draft.
**For now, local AI requires both Mac and Siri to use English (United States)**
(`en-US`), with Apple Intelligence enabled and its model setup complete.
`F3` shows this requirement alongside the detected language settings and model status.
You can draft a question even while Apple Intelligence is unavailable; Diskray
rechecks every 30 seconds and on Enter. `F2` opens System Settings. Questions
are sent only when you press Enter with AI ready.
If macOS requests a restart after a language change, save your work and restart
before checking again. See [AI setup and live verification](AI.md).

A persistent progress band shows **Checks → Folder map → Review**, with actual
completed checks, scanned items, unreadable entries, and elapsed time. Unknown
work is never presented as a percentage. Execution separately reports baseline
samples, reviewed actions, and verification. Messages remain visible, and `?`
opens a scrollable guide including the full last message.

Hidden confirmations are blocked below the minimum size. Colors supplement
text labels; `--no-color` is supported.

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Switch focus between the list and details. |
| `g` / `b` / `h` | Storage findings / browse folders / saved scans. |
| `:` | Command palette: search every action and run it. |
| Arrow keys | Choose a row; scroll an expanded explanation. |
| `Enter` | Open folder children, finding details, or session results. |
| `Space` | Add/remove the exact selected cleanup action. |
| `t` | Queue/unqueue a personally selected item for Trash review. |
| `a` | Add every quick win to the plan. |
| `p` | Review the exact action plan. |
| `i` | Investigate finding with local AI and read-only tools; while browsing, measure the selected folder. |
| `/` | Focus the persistent Ask AI input (read-only investigation). |
| `F2` | Open System Settings, including while typing in Ask. |
| `e` / `d` | Explore selected finding's contents / expand evidence. |
| `P` | Inspect current-account processes. |
| `m` | Plan relocation to an external volume. |
| `f` / `K` | All/fewer items; hide the selected item until the next scan. |
| `v` | Show/hide exact storage totals and scan coverage. |
| `o` | Reveal selected path in Finder. |
| `r` | Recheck storage and activity. |
| `PgUp` / `PgDn` | Page the focused list or scroll details. |
| `Home` / `End` | Go to the beginning/end of the focused list or details. |
| `M` / `A` | Toggle motion / open System Settings for Apple Intelligence. |
| `a` (approval page) | Approve a pending fixed administrator-assisted diagnostic. |
| `R` | Enable/disable saved fixed-catalog online research consent. |
| `?` | Open/close the scrollable usage and keyboard guide. |
| `Esc` | Back/cancel; stop a running investigation; during execution, request stop after current action. |
| `q` | Quit when no action is running. |

## Automation and plain output

When input or output is redirected, the app automatically emits line-oriented
plain text. `--no-tui` forces that behavior. The default plain-output scope is
the local startup volume; pass `--volume` to inspect a specific home or mounted
volume. Findings are ordered largest-first, and analysis remains read-only:

```bash
./target/release/diskray --analyze --no-tui --verbose
```

For scripts and inventory tools, `--json` emits a versioned report and implies
non-interactive output. Sizes are allocated filesystem kilobytes, paths are
exact, and cleanup outcomes are included when cleanup was requested:

```bash
./target/release/diskray --json | jq '.storage_inventory, .summary, (.items[] | select(.size_kb > 0))'
./target/release/diskray --clean --yes --json > cleanup-report.json
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
./target/release/diskray --clean --yes
./target/release/diskray --clean --include-reinstallable --yes
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
