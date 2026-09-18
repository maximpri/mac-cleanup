# Fast triage and unified AI-assisted Mac care

Status: implementation in progress; the unified workspace and local AI helper are now in source. Release certification is pending. This plan supersedes earlier
milestone ordering and timing assumptions in the product discussion. It keeps
the accepted scope: a Rust/Ratatui utility for developers and AI-tool users,
Apple Intelligence in the first release, macOS 26+ on compatible Apple silicon,
and a visual workflow without chat.

## Implementation checkpoint — 2026-09-17

Implemented in source:

- Progressive storage/process assessment and a Findings / Explore / History workspace.
- Deterministic quick-win eligibility, stable finding selection, selected-folder maps,
  CPU deltas, native swap/device rates, pressure readings, and explicit coverage inspection.
- A compiled Swift FoundationModels helper with bounded structured responses,
  evidence/action reference validation, AI-ranked triage within Rust policy,
  stale-evidence rejection, and no cloud fallback.
- Contextual read-only investigation with permitted checks and cancellation.
- Actual-work cyan/violet activity visuals, completion accent, reduced motion,
  no-color support, and terminal focus tracking.
- Shared exact-target plans for cleanup, signals, and relocation; protected-data
  deletion remains a separate single-item typed confirmation. Process signals and
  relocation now have separate acknowledgments in mixed plans.
- Validated insight caching is keyed by the evidence revision and prompt contract;
  targeted process refresh reports the current PID identity rather than a generic note.
- High-memory system-owned daemons are visible as read-only observations. A selected
  `fseventsd` finding can request a bounded `sudo -n fs_usage` sample; swap-backed
  filesystem activity is explained as evidence to investigate, never as proof that
  the daemon caused memory pressure.
- Completed action sessions can receive a bounded local AI outcome summary in
  History; raw action records and before/after measurements remain authoritative.
- Per-action history, before/after resource windows, regrowth/restart observations,
  persistent results, confirmed history clearing, and recheck.
- Regression tests for focus, read-only behavior, hidden confirmations, overlapping
  plans, stale inference, responsive rendering, and reduced motion.

The first implementation deliberately does not certify the remaining release
acceptance gates below. Outstanding work includes richer app/session ownership
adapters, causal workload correlation, and cumulative attributable AI resource
budgets. The current CPU/pressure observations must not be
presented as causal performance gains. Folder investigation is a bounded read-only
measurement; the model never receives a general shell or filesystem mutation tool.

Validation on the development Mac passed 127 Rust tests, strict Clippy, the
read-only JSON schema check, and a live terminal test that cancels and executes
cleanup only inside a disposable fixture while preserving neighboring files.
The real helper returned validated synthetic explanations in approximately
three to four seconds. The release script successfully builds both binaries. These are local observations,
not a performance guarantee or a substitute for the planned evaluation corpus.

## 1. Outcome and first-release boundaries

The operator should quickly identify the areas that deserve attention, see
worthwhile low-disruption actions, investigate within the app, and verify the
result. Storage and processes contribute to the same finding and review plan.

The first release includes progressive assessment, Key areas, Quick wins,
Apple-generated explanations and bounded investigation, contextual relocation,
a unified action review, persistent results, and local session history. Keep
existing cleanup targets and explicit process actions available through this
journey. No new model-driven deletion targets are introduced.

Initial application/activity associations cover the current Xcode, package
manager, OpenCode/Codex, model-download, and browser-automation findings. Use
known locations, app executable paths, PID identities, process ancestry, and
targeted open-handle evidence. Generic containers and model directories remain
inspectable; volume resets, per-model removal, and deletion of arbitrary project
artifacts require future dedicated adapters.

Unknown and shared ownership are valid results. A launcher exiting or a process
being reparented to PID 1 does not establish an abandoned session. Do not promise
session-level cleanup where ownership and supported actions cannot be proven.
Rebranding, cloud providers, a background daemon, custom ML training, and a
general chat interface are outside this release.

## 2. Operator experience

Use one navigation line, **Findings · Explore · History**, with **Review plan (N)**
at the right, followed by one status line. Retain the bottom contextual command
bar. Menu labels have no numbers and selection highlights occupy one line.

**Findings** opens immediately. Use one scrollable list with short group labels:
Quick wins, Key areas, and Needs investigation. Show up to three quick wins and
five key areas initially; an All findings filter exposes the complete list.
Each subject appears once in that list. Never hide critical observed pressure
behind these limits: the status line exposes it and links to its finding.

A row shows the application/activity, its observed impact, and a short reason.
Keep file sizes and resource metrics as separately labeled quantities. The right
pane shows a concise explanation, the relevant visual evidence, tradeoffs, and
actions. A storage finding uses the existing selected-folder treemap; a resource
finding uses a sampled trend and process family. Combined findings show both
when space permits. Evidence and actions remain usable while AI is pending.

Use slate surfaces, high-contrast text, blue focus, amber attention, and coral
destructive confirmations. Labels carry meaning independently of color. Treemap
colors distinguish branches, not cleanup safety. Avoid nested boxes around every
sentence, oversized metric cards, and generic health scores.

At widths of 110 columns or more, use a 45/55 list/detail split, with the list
capped at 60 columns. At 60–109 columns, show the list and open detail as a full
content view with a visible Back control. Retain the 60×16 minimum; smaller
terminals offer resize/cancel/exit without hidden action shortcuts.

Tab/Shift+Tab traverse navigation, list, and detail controls; arrow keys move in
the focused component; Enter opens/activates; Escape returns or cancels. Keep
F9 for commands, ? for help, and mouse hit regions derived from the rendered
geometry. Add to plan changes selection only. No ordinary selection keystroke
executes cleanup. Confirmation dialogs own all keyboard and mouse input.

Preserve the selected finding by stable ID as evidence arrives. Do not silently
reorder a list under the cursor: announce updated recommendations and offer
Refresh order. Safety and eligibility changes take effect immediately.

**Explore** offers Apps, Folders, and Processes as inspection filters over the
same evidence. Moving to Explore and back preserves the originating finding and
plan. The folder map retains exact-child navigation. Move data is an action on
an eligible selected folder, without the current largest-20-source restriction.

**History** lists assessment sessions and action outcomes. Results remain open
until the operator leaves; remove the five-second automatic exit. Keep dismisses
a finding for the session. A separate Protect action explicitly updates the
existing whitelist for eligible paths.

## 3. Progressive assessment and quick-win policy

Replace the scan-gated UI with an assessment coordinator whose background
events are processed on every UI tick, independently of the current screen.
Move all potentially blocking filesystem/process work off the render/input
thread. Use stable subject, evidence, and action IDs plus an assessment generation
to reject results from cancelled or replaced work.

Launch capacity/process/VM collection immediately and sample resources every two
seconds. Gather a ten-second quiet baseline before starting deep inventory or
AI inference. Show available readings throughout that interval. Then run one
filesystem measurement worker and a bounded metadata/check worker, prioritizing
known cleanup locations before the remaining inventory. Run at most one model
request at a time. Cancel obsolete selected-item requests.

The filesystem worker yields between bounded work slices and emits completed
subtree measurements. A shallow directory listing cannot supply recursive sizes.
Show incomplete measurements as “still measuring” or a labeled lower bound;
show historical sizes with their measurement time. Only completed current
measurements contribute to reclaimable estimates and quick-win eligibility.
External/network locations require explicit scope selection.

Collect process CPU from cumulative CPU-time deltas over a stated interval,
RSS as resident memory rather than reclaimable memory, native VM page counters,
swap allocation, and system memory-pressure state. Use read-only system queries;
an unavailable pressure reading remains unknown. Swap allocated is not swap rate:
derive rates only from appropriate counters. Sample disk activity through a
bounded collector; unavailable readings do not block triage. Do not describe
aggregate process RSS as unique physical memory or bytes that stopping an app
will necessarily recover.

Quick wins are selected by policy before AI:

- A current, complete, allowlisted routine cleanup candidate with known identity,
  no active-use or whitelist blocker, and an explicitly low-disruption policy.
- At least 100 MiB measured recovery by default; smaller candidates stay in All
  findings. When free disk is below 10% or 10 GiB, space-recovery actions receive
  higher priority; below 5% or 5 GiB, show urgent capacity attention. These are
  product defaults, not claims about macOS performance thresholds.
- Use explicit per-rule disruption metadata; Routine/Ready alone is insufficient.
  Active build outputs, redownload-heavy models/runtimes, Trash/personal data,
  review-only data, process termination, and relocation are excluded from Quick
  wins. Supported actions remain available through regular review.
- Rank by current pressure relevance, lower disruption, then measured recovery;
  use stable ID as a tie-breaker. AI can reorder eligible candidates within the
  same priority class and explain its choices. It cannot change eligibility.
- Confidence comes from evidence completeness and relationship provenance. Do
  not accept a model's self-reported confidence as proof of safety.

An empty quick-win list says “No quick wins confirmed” while assessment is
partial and “No worthwhile quick wins found” once the relevant checks finish.
Never manufacture recommendations to fill the layout.

Cache recent measurements for navigation and prioritizing follow-up scans.
They never authorize action. Compare growth only across complete measurements
of the same subject/scope. First launch has no growth baseline. Changes between
visits do not prove which process wrote the data.

## 4. AI integration and internal contracts

Bundle a small Swift Foundation Models helper beside the Rust executable. Use
the macOS 26 API surface for both macOS 26 and 27; the `fm` CLI is not a runtime
dependency. A release build script builds both executables and validates helper
discovery relative to the main executable. Ship an arm64 archive containing both;
update source-install instructions accordingly. End users need no Xcode or API
key. Source builds require Rust and Xcode with the macOS 26 SDK or newer.

Use a versioned JSON Lines protocol over pipes with request IDs, cancellation,
and typed responses for availability, triage, selected-finding insight, and
result summary. Rust resolves targets, executes investigation checks, and sends
the resulting evidence back for a fresh bounded request. The helper performs
inference only. Avoid shell invocation and raw command interpolation.

Keep these application-level contracts outside the TUI:

| Contract | Required meaning |
| --- | --- |
| AssessmentEvent | Generation, collector status, timestamp, completed measurement or failure |
| Finding / KeyArea | Stable subject, linked evidence/actions, priority, coverage, current insight |
| Evidence | Source, scope, observation/window, completeness, revision, relationship provenance |
| ActionCandidate / QuickWin | Typed existing operation, exact target identity, benefit, disruption, blockers |
| Insight | Evidence revision, cited evidence IDs, explanation, permitted action IDs, optional check requests |
| ReviewPlan / ActionResult | Reviewed targets, dependencies, consents, execution states, before/after observations |

Use Rust to calculate all sizes, totals, rates, and rankings needed for immediate
display. Give the model a compact shortlist (at most eight subjects), known
purpose/disruption facts, and permitted IDs. It returns up to five key-area IDs,
three eligible quick-win IDs, and short reasons. Guided generation enforces the
response shape; Rust validates IDs, revisions, and allowed relationships. Display
numeric facts from Rust rather than accepting model-generated quantities.

For Investigate further, allow at most three read-only checks scoped to the
selected subject: inspect its measured children, measure an already discovered
subtree, refresh its process family, check open handles, compare saved complete
measurements, or sample the selected system-owned `fseventsd` with the fixed
`fs_usage` collector. Limit the entire investigation to 60 seconds; cancel at
safe read boundaries and retain marked partial evidence. No file-content
ingestion or free-form shell tools in this release.

Respect the model's context limit, including instructions, schema, and response.
Reserve 768 output tokens, compact evidence deterministically, and use token
counting when available. On context overflow, retry once with a smaller packet.
Use a 20-second per-request deadline and no periodic retry loop after failure.
Exclude raw environment variables and full process arguments from model/history
packets; only extract the metadata needed for the supported association checks.

Cache validated insights by evidence revision, prompt version, and OS/model
version. Changed safety evidence invalidates related recommendations immediately.
Treat paths and process labels as untrusted data, including embedded terminal
control sequences. A valid schema and valid IDs do not prove that prose is true;
explanation quality is an explicit evaluation gate.

On supported hardware, disabled Intelligence, unavailable assets, refusal,
timeout, or helper failure leaves the same workspace usable with deterministic
explanations and an AI availability indicator. Offer Retry and an appropriate
settings link. Pause inference under critical memory pressure and during
verification. Do not silently use a cloud service.

## 5. Review, execution, history, and compatibility

Refactor navigation, assessment, and execution into independent state. A scan or
AI response must never trigger a page transition or overwrite an open review.
The shared plan can contain cache cleanup, explicit process actions, and verified
relocation. No actions are preselected on launch.

Review shows each exact target, expected benefit, disruption, and prerequisite.
Process termination and relocation require their own explicit acknowledgments;
advanced review-data deletion retains its single-item typed confirmation. Force
termination remains a separately chosen action without automatic escalation.

Resolve duplicate/overlapping cleanup scopes and native-command footprints before
review. Reject incompatible operations such as cleaning and moving the same
directory. Execute sequentially in dependency order. Stop/quit must complete and
active-use checks must pass before dependent cleanup. A failed prerequisite
skips its dependents; independent approved actions may continue. A stale plan
requires renewed review when targets or consequences change. Never expand a
confirmed plan using fresh AI output.

Cancel stops at the next supported safe boundary. Relocation retains its existing
copy verification and rollback rules and explains when an in-progress step must
finish. Preserve outcomes even if the operator cancels or a worker fails.

Pause inventory and inference before a ten-second pre-action sample and during
a matching post-action sample. Record action completion, target disappearance,
process restart, measured bytes removed, and filesystem free-space delta as
separate outcomes. Report resource changes as observations over those windows,
not causal proof of improved responsiveness. If baseline capture is skipped or
unavailable, report performance verification as incomplete.

Persist schema-versioned session records and compact complete measurement
snapshots under `~/Library/Application Support/mac-cleanup/`, with user-only
permissions and explicit exclusion from cleanup candidates. Retain the most
recent 30 days, bounded to 50 MiB; prune oldest records and offer Clear history.
Persist action-state transitions so an interrupted run becomes “verification
needed,” never automatically resumed. Keep the existing deletion audit log;
do not fabricate historical sessions by importing it. Report history-write
failure without losing the in-memory execution result.

Preserve the existing CLI flags, unattended-cleanup policy, and JSON schema 5.
The new insight/plan protocol is internal; default noninteractive commands never
invoke AI or begin new process actions. The redesigned supported release is
arm64/macOS 26+; unsupported systems receive an actionable compatibility message.

## 6. Delivery sequence and acceptance gates

These are work packages within one first release; AI is included before release.

Feasibility checked during this review: the current macOS 26.5.1 / Swift 6.3.3
environment successfully imports Foundation Models and reports
`SystemLanguageModel.default.availability` as available. Generation quality,
helper packaging, and cold/warm inference latency remain to be evaluated.

1. Build and exercise the real Swift helper and compact insight contract on the
   available macOS 26.5.1 machine. Add synthetic evaluation fixtures and validate
   packaging before committing the UI to model-dependent behavior.
2. Add shared findings, collector events, explicit disruption policy, progressive
   measurement, baseline sampling, and stable IDs. Adapt existing domain engines.
3. Build Findings/Explore/History with final geometry, keyboard/mouse behavior,
   quick-win policy, treemaps, and pending/partial/unavailable states.
4. Connect automatic bounded triage, selected-item insights and investigation,
   shared review/execution, persistent results, and history.
5. Complete visual, safety, performance, model-quality, CLI, and release checks;
   update README, product/navigation/architecture docs, install instructions,
   changelog, and synthetic screenshots to describe the implemented behavior.

Performance targets are measured acceptance goals, not guaranteed scan times:

- On a declared Apple-silicon test machine, render an interactive shell within
  500 ms and available capacity/process facts within three seconds at p95 over
  20 starts. Record cold and warm starts separately. A timed-out collector shows
  an explicit unavailable state; that is not counted as successfully measured.
- First-launch folder totals and completed AI triage have no three-second claim.
  Report time to first confirmed quick win and time to validated AI insight
  separately. Show findings incrementally; never wait for full inventory.
- During deep scan/inference, target p95 input response below 100 ms. Record
  app/helper CPU, memory, and disk activity alongside latency and full-scan time.
- In at least six representative operator tasks, users can identify the main
  area, explain the action's tradeoff, review its exact scope, and read the result
  entirely in the app. Record decision time and incorrect decisions against the
  current app; do not use GitHub stars as a product acceptance metric.

Automated tests cover partial/cancelled collectors, stale generations and insight
revisions, duplicate targets, active/restarted/PID-reused processes, shared paths,
protected/unknown ownership, relocation failures, history interruption, false
all-clear states, and malformed/injected model output. Test zero quick wins,
large useful data, intentional heavy builds, and same-scope growth comparisons.

Render and PTY-test 60×16, 80×24, 120×30, and 160×40, with and without color.
Verify no selection jumps, hidden actions, nested-modal input leakage, or lost
review context. Use disposable filesystem and process fixtures for execution.

Use at least 30 synthetic model-evaluation cases, each run three times on the
supported macOS 26 and 27 model environments. Require zero accepted unauthorized
actions and at least 95% evidence-supported explanations under an explicit
human-reviewed rubric. Compare AI-assisted triage/investigation against the
deterministic baseline; it must improve decision quality or investigation effort.
Persist model/OS/prompt versions and results, never real user inventory.

CI runs Rust format/Clippy/tests/docs, Swift compilation and protocol tests,
release packaging, and deterministic provider fixtures without requiring an
available Apple model. Live model evaluations and a two-binary install smoke test
run on an Apple Intelligence-enabled Mac before release. Missing live evaluation
is an incomplete release gate, not a skipped success. No release/tag/push is
part of this plan review.

### References

Apple documents macOS 26 on-device availability and free offline inference in
[Foundation Models availability](https://www.apple.com/ca/newsroom/2025/09/apples-foundation-models-framework-unlocks-new-intelligent-app-experiences/).
[Context management](https://developer.apple.com/documentation/technotes/tn3193-managing-the-on-device-foundation-model-s-context-window)
describes the on-device session limit, and
[generation guidance](https://developer.apple.com/documentation/foundationmodels/generating-content-and-performing-tasks-with-foundation-models)
notes asynchronous generation and capability limitations. Timing budgets,
ranking thresholds, architecture, and acceptance gates above are this project's
design decisions and require measurement.
