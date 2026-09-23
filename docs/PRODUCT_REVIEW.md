# Product direction: a unified AI-assisted Mac cleanup utility

The [implementation plan](IMPLEMENTATION_PLAN.md) defines the current release
scope and acceptance gates: fast progressive triage, Key areas and Quick wins,
AI included in the first release, and macOS 26+ on compatible Apple silicon.
It supersedes earlier milestone ordering and optional-platform assumptions in
this review. These documents describe proposed work, not shipped capabilities.

## Goal and scope

The product goal is **improve Mac performance and remove waste through one
coherent experience**. Storage measurements, process observations, and application
context feed the same investigation and action plan. The operator should not need
to decide which scanner to run or connect separate reports manually.

AI enhances explanations, investigation, and prioritization inside the visual
interface. There is no chat screen, prompt box, or required conversation. The
primary controls are ordinary actions such as Investigate, Review actions, Keep,
and Recheck.

Product promise: **show what is causing pressure or consuming unnecessary space,
explain worthwhile actions, perform the approved actions, and show what changed.**

This document reviews the existing journey and specifies the intended one.
Proposed AI insights, unified findings, diagnostics, plans, and history screens
are not implemented features. The shipped app still has separate Storage audit,
Process health, and Move data destinations. This direction supersedes the earlier
proposal to retain those destinations as the primary product organization.
The review covers launch, measurement, interpretation, investigation, selection,
execution, verification, return visits, read-only mode, and CLI automation. It
is based on source inspection and platform documentation, not a diagnosis of the
user's current Mac or a new performance benchmark.

## Findings

### P0: Performance has no measurement-and-verification loop

`src/processes.rs` reads a process table on request, exposes CPU percentage and
process states, and supports identity-checked signals. `refresh_processes` in
`src/tui/input.rs` refreshes that snapshot manually. There is no CPU history,
system memory-pressure collection, swap activity measurement, disk-I/O baseline,
or before/after performance comparison.

Consequently, the application can report that a process exited; it cannot show
that the Mac became more responsive. Its abnormal-process ordering is useful for
inspection but is not a ranking of performance impact. The existing explanatory
text correctly notes that stopped processes may be intentional and zombies are
already dead; that nuance should drive recommendations, not remain buried in
details.

### P0: Eligibility is presented more clearly than benefit

The cleanup engine answers whether a known target can be cleaned under its
policy. It does not establish that clearing every eligible cache is worthwhile
for the user's current goal. `decision_guidance` already acknowledges rebuilds,
redownloads, and valuable app-managed state. Those costs need to influence the
recommendation and default selection, not only the confirmation wording.

A useful finding needs separate fields for evidence, eligibility, expected
benefit, disruption, and confidence. Storage recovered and performance improved
must remain separate outcomes. No score should imply that deleting more always
makes the Mac faster.

### P1: The app is organized around tools rather than the user's problem

The old standalone Overview provided instructions without measurements. Storage,
processes, and moving data have disconnected flows. Storage further separates
folders, cleanup findings, and coverage. The user still has to relate a symptom
to evidence, translate a path into its meaning, and assemble a plan.

The requested top-menu/content layout is appropriate and keeps the full width
available for storage evidence.
The content and sequence need to answer the user's questions, rather than mirror
the internal modules.

### P1: Time to a useful answer is unnecessarily long

`scan_next` waits for retention discovery, then scans cleanup candidates, then
starts full inventory. The UI enters review only after the inventory finishes.
The inventory-stage percentage advances with elapsed time toward 99%, without a
known total-work denominator (`advance_scan_progress`).

Quick performance measurements and completed cleanup findings should be usable
while deeper storage work proceeds. Long work needs real counts, elapsed time,
location, and an indeterminate indicator when completion cannot be estimated.
The scanner's own CPU and disk activity must also be accounted for when judging
performance before and after an action.

### P1: Investigation often ends before a supported action

Folder drill-down is useful. However, some recommendations still require manual
work in another app, and guarded cleanup of app-managed data may reset an entire
directory instead of removing selected obsolete items. Relocation starts from
a separate list capped at 20 sources (`src/tui/helpers.rs`) rather than reliably
continuing from the selected item.

Keep investigation context through the next step. Provide selective actions
only where the application's supported interfaces and this app's safety model
can establish exact scope. Clearly label an external-app handoff when that is
the available option; do not describe a handoff as completed cleanup.

### P1: Outcomes do not support continuing the journey

Cleanup distinguishes measured bytes removed from filesystem free-space change,
which is useful. However, `SUMMARY_AUTO_CLOSE_AFTER` closes the app after five
seconds. Process verification waits only for disappearance for up to 750 ms; it
does not remeasure resource pressure or confirm that an app remains stopped.

`src/history.rs` records cleanup attempts in a local log. It is not a browsable
session history covering performance measurements, process actions, relocation,
and their outcomes. A returning user cannot easily see whether the same waste
or resource problem returned.

### Strengths to retain

- Known-target cleanup policy, separate review-data handling, and explicit opt-in.
- Exact-path, identity, symlink, ownership, in-use, and whitelist protections.
- PID identity checks, ancestor-process protection, and no automatic escalation.
- Relocation copy verification and failure recovery.
- Disk accounting separated from measured directory data.
- Keyboard/mouse navigation, modal input isolation, compact layouts, and CLI/JSON.

## The complete intended journey

| Stage | User question | Required experience | Completion condition |
| --- | --- | --- | --- |
| 1. Open | Is something wrong, and is there worthwhile waste? | Start a bounded, cancellable read-only quick check. Show scope and useful results progressively. Offer deeper scanning without making it a prerequisite for every answer. | A meaningful measured status or explicit unavailable result appears. |
| 2. Diagnose | What is causing pressure? | Show CPU trends, memory pressure, swap activity, disk capacity, and available disk-I/O evidence. Show sampling windows and coverage. | Evidence distinguishes an observed condition from a possible cause. |
| 3. Prioritize | What should I do first? | Present recommended actions with benefit type, evidence, disruption, and blockers. Put relevant low-disruption actions first. Say when no worthwhile action was found. | The user understands why an action is recommended. |
| 4. Investigate | What exactly is this item? | Open an app, process group, folder, or waste category in the same content pane. Preserve context and selection. Show exact paths/PIDs on demand and always during review. | The operator can inspect scope and consequences without shell commands. |
| 5. Prepare | What will happen if I continue? | Build a review plan with exact targets, selected actions, dependencies, estimated reclaimed space, and performance hypotheses. Keep high-impact actions separately confirmed. | Every executable action has understandable scope and consent. |
| 6. Act | Is it working, and can I stop? | Revalidate each action immediately before execution. Show per-action progress and outcomes. Stop at safe boundaries and explain recovery if interrupted. | Each action ends with a recorded success, skip, failure, or pending result. |
| 7. Verify | Did this actually help? | Refresh affected storage measurements and sample performance again under comparable conditions. Report uncertainty, unchanged results, and app relaunches honestly. | The result separates action completion, space recovered, and observed performance change. |
| 8. Continue | What remains or keeps returning? | Keep results open. Offer another investigation, a targeted rescan, and local session history. | The operator can continue without restarting the application. |

## Performance and waste: different evidence, connected decisions

| Situation | Suitable response | Success evidence |
| --- | --- | --- |
| Startup disk under pressure and disposable data found | Review low-disruption cleanup first; offer relocation for useful data only when appropriate. | Actual free-space change and refreshed capacity. |
| Sustained resource-heavy app while the user experiences slowness | Explain the observation, identify the app, and offer a separately confirmed supported quit action. Treat force termination as a stronger choice. | Resource trend after the action, exit/relaunch state, and whether the symptom persists. |
| High memory pressure | Identify measured contributors and offer appropriate app-level actions. Do not equate all used RAM with waste. | Pressure and swap activity over a comparable observation window. |
| High CPU during a deliberate build or render | Explain the load and allow the user to keep it running. | No unwanted interruption; a correct diagnosis can require no action. |
| Large reusable cache with sufficient free space | Explain the storage/rebuild tradeoff; avoid presenting its removal as a proven speed improvement. | The user makes an informed optional space decision. |
| Large personal files | Help inspect, protect, or explicitly relocate them. Size alone does not establish waste. | Intended files retained or a verified move. |
| Unexplained space or inaccessible paths | Show incomplete coverage and a specific access/settings action where available. | Improved coverage or an honest unresolved limitation. |
| No material pressure and little useful cleanup | Say the Mac needs no action based on the available evidence. | No manufactured problem or unnecessary deletion. |

## Navigation and information hierarchy

Retain one persistent, single-line top menu with a status bar. The proposed
destinations are **Findings**, **Explore**, and **History**. Settings and advanced
commands remain secondary. A contextual **Review plan** control shows the number
of selected actions and opens the same plan from any destination. Navigation
labels have no numeric prefixes; highlights occupy one line.

**Findings is the working screen on launch.** Begin a bounded local assessment
and populate actionable findings progressively, before the deep disk inventory
finishes. It contains measured conditions and decisions, with no welcome cards
or passive Overview screen. Show a useful all-clear state when evidence supports
it, and distinguish that state from an incomplete assessment.

Findings are organized around the responsible application, activity, or concrete
problem. One finding can combine storage growth, associated processes, memory
pressure, and supported actions. Filters can narrow benefit or action type, but
the default list includes all relevant findings. Priority uses measured impact,
confidence, and disruption; size and CPU usage alone do not establish waste.

The status bar reports current assessment progress, measurement freshness, disk
capacity, and resource pressure when available. Never collapse these into an
unexplained health score or a fabricated overall scan percentage.

The main pane shows findings on the left and the selected finding's explanation
and evidence on the right. Its detail pane answers, in order: what was observed,
why it may matter, what can be done, and what the tradeoff is. Storage findings
include a nested map; process findings include process relationships and sampled
trends; combined findings can show both. Findings stay usable while optional AI
explanations are pending.

**Explore** provides deeper inspection of applications, folders, and processes
in the same workspace. The folder browser and selected-folder map remain
available here. Rectangle area shows size; branch colors distinguish folders.
Moving from a finding to its folder or process preserves the finding, selection,
and plan. Relocation is a contextual action on eligible useful data.

**History** shows completed plans, actual outcomes, and evidence of recurrence.
History entry points lead back to the same application or finding. A history
entry must not imply an undo operation when no recovery mechanism exists.

Exact paths and PIDs are available in inspection and always visible in action
review. Preserve stable shortcuts and keep essential actions reachable by mouse
and keyboard. This is the target navigation, not a claim about shipped screens.

## One finding across storage and processes

Use a shared finding model with the following information:

- Subject: application, project, activity, directory, or system condition.
- Evidence: measurement IDs, timestamps, sampling windows, scope, and coverage.
- Relationships: owning app, process family, generated data, or shared resource,
  each with its provenance and uncertainty.
- Interpretation: observed condition, possible explanation, and unresolved checks.
- Actions: supported operations, benefit type, dependencies, disruption, and blockers.
- Outcome: action completion, refreshed observations, and any remaining uncertainty.

Retain explicit unknown and shared ownership. An AI-suggested relationship cannot
become verified ownership or authorize deletion. Deduplicate overlapping targets
and shared process relationships across findings before constructing a plan.

For example, a browser-automation finding could combine residual process
activity and associated temporary profiles. The app first checks whether an
active session still owns the activity. If supported and approved, it can stop
the selected process, recheck file use, clean eligible temporary data, and then
measure the outcome. This is one continuing investigation with separate action
consents, not separate storage and process tasks for the user to assemble.

Other cases require only one kind of evidence: a full disk with disposable data,
an intentional build that should continue, or a useful directory suitable for
verified relocation. Do not force every finding to contain every measurement.

## AI as part of the interface

Use Apple's on-device Foundation Models as the proposed default provider on
supported machines. Model availability is checked at runtime; unsupported
hardware, disabled Apple Intelligence, or unavailable model assets must leave
the ordinary audit and cleanup workflow functional. A Swift helper can support
macOS 26; macOS 27 also provides Apple's `fm` command-line interface. Cloud
inference is a separate, explicit option and is never a silent fallback.

AI has three initial jobs:

1. **Explain a finding.** Turn supplied measurements and known cleanup rules into
   a concise description of purpose, benefit, disruption, and uncertainty.
2. **Investigate further.** Select bounded read-only checks from an explicit tool
   catalog and update the current finding with the resulting evidence. Show
   progress, allow cancellation, and cap work; no manual terminal commands.
3. **Explain a plan and its results.** Describe dependencies and tradeoffs before
   execution, then summarize measured outcomes without inventing improvement.

The Rust engine owns measurements, target identities, eligibility, confirmations,
and execution. Model output refers to known evidence and action IDs and is
validated before display or use. A model cannot introduce an executable shell
command, expand a confirmed target set, or waive an in-use check. Treat filenames,
process arguments, and inspected content as data, never as instructions.

AI prioritization can suggest a better ordering within the engine's eligibility
and disruption constraints. It must not relabel an unsupported operation as safe.
Unknown purpose stays unknown when the supplied evidence cannot establish it.
User choices such as Keep can be remembered as visible, editable preferences;
they do not grant permission for later destructive actions.

Run inference on demand for the selected finding or a bounded summary. Cache
insights against evidence version and invalidate them when measurements or
eligibility change. Do not generate an explanation for every file. Keep model
work out of performance verification windows, or label those windows as affected.

The product's distinction is the complete evidence-to-action journey. Model access
alone does not establish an advantage. Evaluate both explanation accuracy and
whether users reach a sound decision with less manual investigation.

## Review-plan contract

Each proposed action carries:

- A concrete problem and evidence, including timestamp and scope.
- The exact target and supported operation.
- Separate expected storage benefit and performance hypothesis, where applicable.
- Cost: redownload, rebuild, loss of local state, unsaved work, or external-drive dependency.
- Eligibility, blockers, and dependencies; being eligible is not the same as being recommended.
- Confirmation requirements and a verification method.

Do not add overlapping cleanup targets or count parent and child sizes twice.
Closing an app to unlock cleanup needs explicit consent and a subsequent in-use
check; cleanup confirmation does not imply permission to force-kill applications.
Preserve the stronger confirmation for review-only data. CLI unattended cleanup
must retain its narrow policy and must not start process termination.

## Recovery, repeat visits, and operational quality

- Interrupted scans retain clearly marked partial evidence where safe; do not
  represent missing data as an empty directory.
- Failed/blocked checks explain the specific unresolved prerequisite and allow a
  targeted retry instead of forcing a complete new workflow.
- Cancelling a review preserves investigation and selection. Cancelling execution
  stops at the next supported safe boundary, with completed actions still visible.
- Deleted data is not undoable merely because a history entry exists. Show recovery
  only where a real mechanism exists, such as retained backups or regeneration.
- A process may restart automatically. Report that observation rather than claiming
  permanent resolution or repeatedly killing it.
- Performance baselines should be gathered before expensive inventory work, or
  explicitly marked as affected by scanning. Do not attribute a drop in load caused
  by the scan ending to a cleanup operation.
- Startup read-only checks stay local and bounded. External/network scans require
  the selected scope. Unsupported diagnostics are unavailable, not healthy.
- Plain/JSON output keeps stable schema semantics; adding diagnostics requires an
  intentional schema extension and compatibility tests.

## Implementation priorities

1. **Unify findings and evidence.** Adapt existing storage and process collectors
   to a shared finding model. Add bounded performance sampling, measurement
   windows, coverage/freshness, and a distinction between recommendation and eligibility.
2. **Ship one working journey.** Progressive assessment, the Findings/Explore
   workspace, contextual actions and relocation, one review plan, persistent
   results, and targeted refresh. Validate it with deterministic explanations.
3. **Add local AI insights.** Evaluate selected-finding explanations and bounded
   read-only investigation. Include unavailable-model and invalid-output handling;
   the same workflow must remain usable without AI.
4. **Close the outcome loop.** Before/after observations, editable Keep preferences,
   recurring-problem history, and conservative AI summaries of what changed and
   what remains unproven. Preserve per-action safety checks throughout.

## Acceptance scenarios

- Nearly full startup disk with safe waste: understand the pressure, review useful
  cleanup, execute, verify free space, and continue.
- Plenty of disk space but sustained CPU load: reach performance evidence without
  completing a disk walk; do not recommend cache removal as the remedy.
- High memory use without pressure: avoid an unsupported memory-cleaning recommendation.
- Intended heavy workload or intentionally stopped process: investigate without
  treating its state as sufficient reason to terminate it.
- In-use cleanup target: identify the blocker, request any supported close action
  separately, recheck, and clean only if eligible.
- Incomplete disk visibility: show known amounts and uncertainty without fabricated totals.
- Cancellation, app relaunch, permission changes, and partial failures: preserve
  understandable state and offer an appropriate next step.
- Read-only mode and unattended CLI: no newly introduced action bypasses.
- Combined process/data finding: investigate ownership, review dependent actions,
  revalidate between actions, and verify both storage and process outcomes in one place.
- An active app restarts between stop and cleanup: skip newly blocked cleanup and
  explain the updated finding without repeated automatic termination.
- AI unavailable, cancelled, slow, or returning unsupported claims/IDs: retain
  evidence, navigation, and valid deterministic actions without inventing an insight.
- New scan evidence invalidates an explanation: visibly refresh or remove the old
  insight; never execute a plan based only on a stale AI recommendation.
- No chat interaction: complete assessment, investigation, review, action, and
  verification entirely through visible controls.

Measure time to first useful finding, whether the operator can explain the
recommendation, whether the action can be completed inside the app, and whether
the result can be verified. Rendering tests and input-safety tests remain
necessary; they do not prove these product outcomes.

## Platform references

Apple identifies insufficient startup-disk space and app memory demand among
possible causes of slowness. That supports diagnosing the constraint before
choosing an action. [If your Mac runs slowly](https://support.apple.com/en-gb/guide/mac-help/mchlp1731/mac).

Apple describes memory pressure using several factors, including swap rate,
and explains that cached files in unused RAM improve performance. System RAM
cache is distinct from this app's on-disk cleanup targets; neither its presence
nor a high used-memory total alone establishes waste.
[View memory usage](https://support.apple.com/guide/activity-monitor/view-memory-usage-actmntr1004/10.14/mac/15.0).

Apple's disk-activity view measures reads and writes. Capacity and activity
therefore require separate evidence in our product.
[View disk activity](https://support.apple.com/en-ca/guide/activity-monitor/actmntr1005/mac).

These references ground the diagnostic distinctions. The proposed workflow,
ranking rules, and verification requirements are this review's design conclusions.

Apple documents on-device Foundation Models availability starting with macOS 26
on Apple Intelligence-compatible devices with the feature enabled, including
offline inference at no inference charge.
[Foundation Models availability](https://www.apple.com/ca/newsroom/2025/09/apples-foundation-models-framework-unlocks-new-intelligent-app-experiences/).
Apple introduces the preinstalled macOS 27 `fm` tool, structured output, and the
Python SDK in [Build AI-powered scripts](https://developer.apple.com/videos/play/wwdc2026/334/).
Provider integration above is proposed architecture and has not been implemented
or benchmarked in this application.

## Implementation checkpoint

The unified workspace, bounded local AI helper, shared plans, and local outcome
history are now implemented in source. See the dated checkpoint in
[IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) for completed capabilities and
outstanding release evaluation. Earlier UX proposals in this review are design
context; [USAGE.md](USAGE.md) describes the current controls.
