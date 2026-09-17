# Product review: improve performance and clean up waste

## Goal and scope

The user's clarified goal is **improve Mac performance and remove waste**.
Understanding storage is part of that journey. Performance diagnosis is also a
primary capability; the previous proposal to make it a secondary tool was too
narrow and is superseded by this review.

Product promise: **show what is causing pressure or consuming unnecessary space,
explain worthwhile actions, perform the approved actions, and show what changed.**

This document reviews the existing journey and specifies the intended one.
Proposed diagnostics, plans, and history screens are not implemented features.
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

The requested left-menu/right-content layout is appropriate and should remain.
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

Retain one persistent left menu. Storage is the first destination and the app
opens its audit automatically. The current destinations are Storage audit,
Process health, and Move data. Future performance trends, cleanup planning, and
history can be added as destinations once those measurements and actions exist.

Settings and advanced commands can remain secondary. A Review plan control is
available from the findings that populate it. This is a proposed destination map;
it does not imply those screens already exist.

The main pane pairs a size-sorted folder browser with a large nested storage map
of the selected folder. Rectangle area shows size; branch colors distinguish
folders. Cleanup eligibility is stated separately, with a link to the findings
and their consequences. Size alone is never presented as evidence of waste. Exact implementation details belong in inspection
and review. User-facing action names should describe the outcome: review cleanup,
quit this app, inspect this folder, or move this folder. Preserve stable shortcuts
and keep all essential actions visible and reachable by mouse or keyboard.

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

1. **Establish the evidence model.** Add bounded performance sampling, measurement
   windows, coverage/freshness, and a distinction between recommendation and eligibility.
2. **Connect the journey.** Quick useful audit, progressive findings, selected-item
   actions, explicit review, persistent results, and targeted refresh.
3. **Improve action quality.** App-level context, exact blockers, selective supported
   cleanup adapters, and contextual relocation. Preserve per-action safety checks.
4. **Verify outcomes.** Before/after observations, recurring-problem history, and
   conservative explanations of what changed and what remains unproven.

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
