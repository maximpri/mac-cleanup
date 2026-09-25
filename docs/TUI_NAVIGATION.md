# Diskray navigation

The workspace has two persistent panels, with no screen tabs. The left panel
lists storage findings, folder children, or saved scans. The right panel shows
the selected item's explanation, an investigation, Ask, help, or plan review.
Both panels remain visible from 60 × 16 upward. The focused panel has a blue
border; the footer names the available keys. Color supplements text labels.

The header shows free space and commands for `/` Ask, `h` History, and `p` Plan.
Clicking DISKRAY or pressing `g` returns to storage findings. A status line below
reports measured progress, such as checks completed, files scanned, unreadable
locations, and elapsed time. It never invents a percentage for unknown work.

## Selection and focus

`↑` / `↓` chooses an item in the left panel; its explanation updates immediately
on the right. `Tab` or `Shift+Tab` focuses details, where arrows scroll
and the full investigation timeline becomes available. `Tab` or `Esc` returns
to the list. `PgUp` / `PgDn` pages the focused list or scrolls the focused details;
`Home` / `End` goes to its beginning or end. Clicking a row selects it;
the mouse wheel targets the panel beneath the pointer.

The selection stays on the top row while a scan re-ranks findings until the
user moves it. After that, the selected identity remains pinned until `f`
changes the list. Unrelated AI reports never appear for another selection.

Review confirmation, help, the command palette, and diagnostic approval own
their input until closed. The left list stays visible for context;
clicking it cannot bypass a confirmation or add an action. Header shortcuts are
inactive during confirmation; they never type letters into the phrase. Coverage
and Ask answers capture their keys until closed with Esc, Left, Backspace, or Tab. Terminals below
60 × 16 show a resize message and block hidden confirmations while allowing
cancellation and exit.

## Storage and folders

Storage findings (`g`) reconcile disk usage. The capacity bar shows measured
files (█), unaccounted usage (▓), other APFS volumes (▒), and free space (░).
For `/`, the walk is reconciled against its own APFS Data-volume capacity
sample. Rows use that walk's sizes, rather than separate cleanup estimates.
Only non-overlapping areas on the accounted device enter the storage list;
macOS-managed files are included in measured files, not other APFS volumes.

SPACE BALANCE adds listed areas, other measured files, unaccounted usage, and
other APFS volumes to total used. Small unknown amounts remain visible.
Filtered, hidden, and smaller areas stay in Other measured files. Listed areas
includes the entire current list, including off-screen rows. If file
measurements exceed reported usage, a negative Measurement excess line
preserves the discrepancy instead of silently shrinking the sizes. Free space
and any capacity difference then reconcile to disk capacity.

Compact terminals show Listed, Rest of used, and Total used. `v` opens the full
balance with exact KiB values, rounding notes, and scan coverage on the right.
For a folder scan, Outside this scan means the rest of the disk; it is not
labelled unexplained. Unreadable locations and unaccounted usage are
observations, never claims of reclaimable space.

Each item has a name and size, followed by a plain status: safe to clear,
in use, app data requiring review, protected, or personal files. The default
list shows the 15 largest items and omits unreadable targets, ordinary
processes, and items under 100 MB (or 1% of a small volume). `f` shows all available areas;
`K` hides the current item until the next scan. Quick-win totals, growth, and
unreadable counts appear below the list when space permits.

`Enter`, `Right`, or `e` opens the selected folder, even from its details.
`b` opens the folder browser directly.
The left panel shows the current path, child count, measured subtotal, and
children sorted by size. Each row shows its share of measured space and whether
it is queued or has a cleanup rule. Partial sizes stay labelled. `Enter` opens
a directory or focuses a file's details. `Left` / `Backspace` goes to the parent;
at the scan root it returns to storage findings. `Esc` first leaves details,
then goes back through visited lists, restoring the selected item by path.
Opening from a finding or heatmap returns to that finding, not an older browser
location. `Left` always follows the actual filesystem parent; `Esc` follows
where you came from. `g` returns directly. `f` on a
folder with an exact cleanup rule selects that finding. Missing child
measurements start automatically when opened, with item/size progress.
`i` remeasures the current folder and `o` reveals it in Finder. A contents heatmap stays below
the explanation in the right panel, in both Storage and folder browsing.
It updates when the selection changes and remains visible on compact terminals.
Rectangle area represents measured size; colors distinguish branches and never
imply cleanup eligibility. Taller maps include a clickable list of the largest
children with exact displayed sizes. Click a folder to browse it or a file to inspect its
details. Small entries are grouped, with `e` opening the full list. The macOS
aggregate maps its real constituent locations. Empty or unreadable contents
are labelled explicitly. Ask remains beneath the heatmap.

`Space` in the browser toggles cleanup for the exact selected rule; it never
silently queues a parent. For personally chosen files/folders, `t` queues a
native Move to Trash action. `p` reviews the complete path and consequences;
type `TRASH` to execute. These moves use a separate plan from cleanup (especially
emptying Trash), release no disk space immediately, and can be restored by
dragging items out of Finder's Trash. Put Back may be unavailable. Home/system
anchors, app-managed Library data outside individual caches, symlinked paths,
other volumes, and whitelist-protected items cannot be trashed here. Read-only
sessions cannot select either kind of action.

## Investigations and actions

`i` on a finding starts a bounded read-only investigation. Details show the tool
timeline, evidence IDs, statuses, and validated report. `Esc` from the list
stops a running investigation and keeps its evidence. Reports expire when their
underlying measurements change. The activity line names the running tool and
whether local AI or the measured fallback chose it.

Ask AI spans the full width below both panels from launch, while the
list initially has keyboard focus. `/` or a click focuses its input, which
accepts up to 200 characters. `Enter` asks; `Esc`, `Tab`, or clicking the list
returns to browsing and keeps the unsent draft. Letters such as `q` are text
while typing. Clicking the composer again does not change its text.
The answer contains the question, selected measured findings, and tool timeline.
`1`–`3` can prefill a suggested follow-up question. Ask requires ready Apple
Intelligence to submit, but drafts can be written while it is unavailable.
Diskray rechecks availability every 30 seconds while unavailable; focusing Ask
or pressing Enter also retries. A draft is never sent automatically when AI
becomes ready. While AI is unavailable, the bar highlights the current
English (US) requirement and the reported reason. `F3` opens local AI status:
the current requirement to use English (United States) for both Mac and Siri,
the auto-selected Apple model, current language support,
Mac and Siri language settings when detected, and recovery guidance. Apple
exposes one general-purpose on-device model through the installed framework;
macOS selects its version, so there is no alternate version picker. `F2` opens
System Settings, including while typing. The composer
steps aside during help, specialist forms, action review, and approvals.
A small automatic investigation may run after scan and triage,
pausing under elevated memory pressure and stopping when plan review opens.

`Space` adds or removes a supported action; `a` adds all quick wins. AI
suggestions remain badges until the user adds them. `P` inspects processes;
Space queues SIGTERM and `x` explicitly queues SIGKILL. `m` plans a relocation.
These actions share the same review plan as cache cleanup.

`p` reviews exact paths, process identities, destinations, and consequences in
the right panel. Targets scroll independently of the visible confirmation.
Type `CLEAN` for ordinary cleanup, `APPLY` for signals or relocation, or `DELETE`
for a separate single-item protected-data plan. Mixed plans require `s` to
acknowledge signals and `m` to acknowledge relocation. `Esc` cancels review;
Delete clears the plan. Every target is revalidated before execution.

Execution reports baseline sampling, reviewed actions, and verification.
`Esc` requests a safe stop; completed actions remain recorded. `h` opens saved
scans on the left and their results on the right. `Esc` returns to storage.
Delete in writable History requires a separate `CLEAR` confirmation; the
cleanup audit log is retained. `r` scans again once the pending plan is empty.

`:` opens a searchable command palette. `?` opens the full guide, including the
last message. `M`, reduced-motion settings, no-color mode, and focus loss
suppress animation. `R` controls consent for fixed-catalog online research.
Administrator-assisted tracing has its own approval and never starts from an
AI suggestion alone.

Implementation: `src/tui/care_view.rs` and `src/tui/care_view/presentation.rs`.
Domain safety remains in `cache.rs`, `processes.rs`, and `relocation.rs`.
