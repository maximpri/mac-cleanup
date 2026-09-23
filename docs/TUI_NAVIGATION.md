# Diskray navigation

There are three screens: **Overview · Explore · History**, with free space and
**Review plan (N)** at the right. `1`–`3` or `Tab` / `Shift+Tab` switch
screens; there is no separate menu focus. One status line below says what is
happening: "Checking cleanup targets 11/45", "Measuring folders · 1.3M files ·
106 GB · 65 s", or "✓ Scanned 3.3M files in 2 min · 789 unreadable (v)".
Scans report measured work, never an invented percentage. Read-only mode, AI
availability, and elevated memory pressure appear quietly at the right.
Messages appear under the screen and disappear on the next key.

**Overview** is the landing screen and answers "where did my space go?". A
capacity bar splits the disk into four parts that always add up: folders the
scan measured (█), used space it could not see (▓), macOS and other APFS
volumes (▒), and free space (░). Glyphs, not colors, carry the meaning. For
`/`, capacity comes from the APFS Data volume, the same filesystem the folder
walk measures.

Below it, one line per item, largest first: cleanup targets and folders chosen
so each name means something (`~/Library/Developer/CoreSimulator/Devices`
rather than `/Users`; `src/storage.rs` `breakdown`), plus one "macOS system
files" row. Each row has a plain verdict: *Safe to clear*, *Safe · downloads
again*, *In use by npx*, *App data · review*, *Protected by you*, *Can't read*,
*Your files*, *Apps*, *Developer data*, *macOS*. By default the list shows the
15 largest items and leaves out unreadable targets, ordinary running processes,
items under 100 MB (or 1% of a small volume), and the low-space alert, which
the header already shows; `f` lists everything. Summary lines at the bottom
give the quick-win total (`a` adds them all), what grew since the last scan,
and how many locations could not be read (`A` opens Full Disk Access settings,
`v` lists them).

The selection stays on the top row while the scan re-ranks the list, until you
move it; from then on the selected item stays selected (pinned) until `f`
changes the list. Wide terminals (110+ columns) show the explanation beside the
list; narrower ones open it with Enter and return to the same row with Esc.

`:` opens the command palette, a searchable list of every action and its key.
Typed letters filter it (they never run single-letter commands); Enter runs the
selected command and Esc closes it.

Clickable controls and rows register the same rectangles used to render them.
Read-only mode blocks all plan additions and execution. Terminals below
60 × 16 block hidden confirmations while allowing cancellation and exit.
Modal states (review, Ask, palette, approval, help) capture every key except
Ctrl-C.

The Overview combines storage and, when relevant, process evidence. Quick wins use deterministic
eligibility; local AI ranks eligible quick wins and key areas without changing
policy. `i` starts a read-only tool investigation of the selected finding. An
action the model suggests shows an **AI SUGGESTS** badge, but only `Space` adds it.
New evidence preserves
selected identity. `f` exposes all findings; `K` keeps an item out of the current
session list. Enter or `d` opens details at every terminal width; `e` explores
the selected finding. An explanation leads with one plain sentence about what was measured, then what the checks found (or the on-device model's interpretation, in its own accent color), what you can do, and a one-line summary of how it was checked. Expanded details (Enter) list every tool call with ✓ usable, ! unusable, ✗ rejected. Unrelated
AI explanations are not shown for a different selection. Wide terminals show a
split view; terminals narrower than 100 columns use a full-width list and a
separate detail page. `v` exposes accounting and coverage without allowing hidden
plan additions. `Esc` closes details before navigating to a parent folder.

Explore opens at the home folder when the startup volume was scanned and lists immediate children by allocated size. Enter opens children;
Left/Backspace returns. A nested rectangle map represents the selected folder.
Colors identify branches, not deletion safety. A map click opens its exact
measured path. Relative bars compare sibling sizes; colors do not imply safety.
`i` measures a selected folder, `d` opens details, and `o` reveals it in Finder.
Enter on a file opens its details instead of doing nothing.

`P` opens current-account process inspection. Space queues SIGTERM; `x` queues
an explicitly chosen SIGKILL. `m` opens relocation planning. Both feed the same
review plan as cache actions. The review groups actions by type (clear
rebuildable contents, permanently delete reviewed data, signal processes, move
to another volume), shows the total that could be reclaimed, and marks items
the model suggested. Protected review data requires a separate,
single-item DELETE plan. Confirmation is fixed below the scrollable targets;
Esc cancels and Delete clears the plan. Mixed plans require `s` to acknowledge
signals and `m` to acknowledge relocation before `APPLY`. Execution cannot
start from menu clicks, AI output, or row selection.

An empty plan explains how to add an action instead of asking for a confirmation
phrase. Execution distinguishes baseline sampling, applying reviewed actions,
and verification, including the actual sample count and actions reported.
The action-count bar is not an overall completion estimate. A stop request is
acknowledged visibly; already completed actions are not undone.

Local inference and read-only investigation have distinct labels. The progress
band names the tool running now, for example `Local AI · Python cache · 2/6 tools
→ folder_age(~/Library/Caches/pip)`, or `Measured checks` when no model is used.
Details list each tool call with its evidence ID, status, and duration; each
result; the explanations considered; and the report with its cited evidence.
Rejected calls (unknown handles, paths, tools outside the toolset) appear in the
timeline and never run. Reports expire when the evidence they explain changes.
`Esc` in the Overview stops a running investigation and keeps its evidence.

`/` opens the Ask box, a modal one-line input capped at 200 characters. Every
letter, including `q`, is typed rather than acting as a command. Enter asks and
opens the answer page with the question, answer, and tool timeline; `Esc` closes
it. When the answer is ready, up to three follow-up questions are listed;
pressing `1`–`3` opens the Ask box prefilled with one so it can be edited first. Ask is available only when Apple Intelligence is ready; otherwise `/`
explains why. After a scan and triage settle, one automatic investigation with a
three-call budget may run for the top key area. It never requests administrator
tracing, pauses under elevated memory pressure, and stops when the review plan
opens.

`M`, `REDUCE_MOTION`, no-color mode, and terminal focus loss suppress scan
animation. `R` saves or revokes consent for the fixed Apple documentation
catalog. When the model requests the administrator-assisted `fseventsd` trace,
a dedicated approval page shows its target and consequences and accepts `a` or
`Esc`. No authorization prompt appears before that choice, and the investigation
deadline pauses while it waits. Measured facts remain usable without AI.

History keeps results open and permits scrolling through exact outcomes.
Enter opens results on compact screens as well as wide ones.
`r` starts a fresh assessment after the plan is empty. It does not claim that
concurrent changes in CPU, pressure, or disk space prove a performance gain.

Contextual footer commands are fitted as whole key/label pairs, never clipped,
and are clickable. `?` opens a scrollable guide rather than a transient status
string. Feedback and errors have a persistent message area; the guide includes
the full last message. In a detail page, arrows and the mouse wheel scroll the
explanation without silently changing the selected subject.

Implementation: `src/tui/care_view.rs` and `src/tui/care_view/presentation.rs`.
Domain safety remains in `cache.rs`,
`processes.rs`, and `relocation.rs`. The retained specialist renderers support
process inspection, folder maps, and relocation entry. CLI output remains
independent and retains JSON schema 5.
