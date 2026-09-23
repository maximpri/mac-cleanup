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

There are three screens: **Overview · Explore · History**. `1`–`3` or `Tab`
switch between them, and `:` opens a searchable list of every command.

**Overview** answers "where did my space go?": a capacity bar (folders the scan
measured, space it could not see, macOS and other volumes, free), then one line
per item, largest first, each with a plain verdict such as *Safe to clear*,
*In use by npx*, *App data · review*, or *Your files*. `Enter` explains an item,
`Space` adds it to your plan, `a` adds every quick win, and `p` reviews the
plan. Unreadable locations, small items, and ordinary running processes are
left out until you press `f`.

The assessment samples resource activity independently while progressively measuring
known cleanup targets, temporary retention, incomplete downloads, and the full
storage inventory. A large scan takes time; an unfinished or unreadable scan is
not a complete measurement. `v` shows coverage and unexplained accounting.

**Explore** browses folders largest first. `Enter` opens children; `Left` or
`Backspace` returns. The right pane shows the selected folder's nested storage
map: rectangle area represents allocated space, and colors distinguish branches,
not cleanup safety. Tiny items remain available in the list. Click a rectangle
to explore its target. `i` measures the selected folder again; `o` reveals it
in Finder. No separate terminal commands are required.

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

The interface supports terminals from 60 columns × 16 rows. Narrow windows use
a full-width list; Enter opens details, and Esc returns. Wide windows show
evidence beside the list. An explanation starts with one plain sentence about what was measured,
then what the checks found, what you can do, and how it was checked. Folder bars and maps show measured size, not waste.

A persistent progress band shows **Checks → Folder map → Review**, with actual
completed checks, scanned items, unreadable entries, and elapsed time. Unknown
work is never presented as a percentage. Execution separately reports baseline
samples, reviewed actions, and verification. Messages remain visible, and `?`
opens a scrollable guide including the full last message.

Hidden confirmations are blocked below the minimum size. Colors supplement
text labels; `--no-color` is supported.

| Key | Action |
| --- | --- |
| `1`–`3`, `Tab` / `Shift+Tab` | Overview · Explore · History. |
| `:` | Command palette: search every action and run it. |
| Arrow keys | Choose a row; scroll an expanded explanation. |
| `Enter` | Open folder children, finding details, or session results. |
| `Space` | Add/remove the selected item's action. |
| `a` | Add every quick win to the plan. |
| `p` | Review the exact action plan. |
| `i` | Investigate finding with local AI and read-only tools; in Explore, measure selected folder. |
| `/` | Ask local AI one question about this Mac (read-only tools). |
| `e` / `d` | Explore selected finding's contents / expand evidence. |
| `P` | Inspect current-account processes. |
| `m` | Plan relocation to an external volume. |
| `f` / `K` | All/fewer items; hide the selected item until the next scan. |
| `v` | Show/hide scan coverage. |
| `o` | Reveal selected path in Finder. |
| `r` | Recheck storage and activity. |
| `PgUp` / `PgDn` | Scroll evidence or review details. |
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
