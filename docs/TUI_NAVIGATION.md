# Unified care navigation

The current Ratatui workspace has one navigation line: **Findings · Explore ·
History**, with **Review plan (N)** at the right. One status line reports disk
space, memory pressure, and assessment state. There is no Overview page or
permanent left menu. Selection in the menu occupies one line.

`Tab` / `Shift+Tab` switches menu and content focus. Arrow keys select the menu
item; Enter returns to content. Content actions are ignored while menu focus is
active. Clickable controls and rows register the same rectangles used to render
them. Read-only mode blocks all plan additions and execution. Terminals below
60 × 16 block hidden confirmations while allowing cancellation and exit.

Findings combines storage and process evidence. Quick wins use deterministic
eligibility; local AI ranks eligible quick wins and key areas without changing
policy, and explains the tradeoffs. New evidence preserves
selected identity. `f` exposes all findings; `K` keeps an item out of the current
session list. Enter or `e` inspects a finding; `d` expands its evidence. The right
pane holds local AI context, selected-folder maps or sampled activity, and the
evidence behind a decision. `v` exposes accounting and coverage.

Explore lists immediate children by allocated size. Enter opens children;
Left/Backspace returns. A nested rectangle map represents the selected folder.
Colors identify branches, not deletion safety. A map click opens its exact
measured path. `i` measures a selected folder, and `o` reveals it in Finder.

`P` opens current-account process inspection. Space queues SIGTERM; `x` queues
an explicitly chosen SIGKILL. `m` opens relocation planning. Both feed the same
review plan as cache actions. Protected review data requires a separate,
single-item DELETE plan. Confirmation is fixed below the scrollable targets;
Esc cancels and Delete clears the plan. Mixed plans require `s` to acknowledge
signals and `m` to acknowledge relocation before `APPLY`. Execution cannot
start from menu clicks, AI output, or row selection.

Local inference and read-only investigation have distinct labels. A moving
cyan/violet accent represents real outstanding work. Completion briefly accents
the result. `M`, `REDUCE_MOTION`, no-color mode, and terminal focus loss suppress
motion. Model explanations identify their evidence scope and expire when that
evidence changes. Measured facts remain usable without AI.

History keeps results open and permits scrolling through exact outcomes.
`r` starts a fresh assessment after the plan is empty. It does not claim that
concurrent changes in CPU, pressure, or disk space prove a performance gain.

Implementation: `src/tui/care_view.rs`. Domain safety remains in `cache.rs`,
`processes.rs`, and `relocation.rs`. The retained specialist renderers support
process inspection, folder maps, and relocation entry. CLI output remains
independent and retains JSON schema 5.
