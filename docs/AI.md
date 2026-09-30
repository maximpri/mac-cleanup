# Local AI

Diskray uses Apple Foundation Models, the on-device model in Apple
Intelligence, to investigate and explain measured findings. Nothing is sent to
a cloud service, there is no account or API key, and the model can never add a
cleanup target or run an action.

Requirements: Apple silicon, macOS 26 or later, an SDK containing
FoundationModels to build the `diskray-ai` helper, and Apple Intelligence turned
on with its model downloaded. Without it, every measured finding still works and
investigations run as a fixed sequence of the same read-only tools.

> **Current Diskray AI requirement: English (United States).** For now, both
> the Mac language and Siri language must be **English (United States)**
> (`en-US`). Allow Apple's model setup to finish after changing either language.

## Availability and model selection

`F3` in the workspace shows the automatically detected Apple on-device model,
its availability, context size, and supported language tags. The installed
Foundation Models API exposes one general-purpose system model; macOS chooses
its version. It does not expose a list of older versions to switch between.
The specialized content-tagging use case is not a replacement for the general
model used to investigate storage. Diskray does not fall back to cloud inference.

A language listed by Apple's framework does not by itself mean Diskray's AI is
ready. Diskray's current setup requirement is English (United States) for both
Mac and Siri; the framework may report additional supported languages.
Apple requires the Mac and Siri languages to match. The helper checks model
language support through the public framework API and reads the Siri language
preference on a best-effort basis. Missing preferences are reported as unknown;
they never override the framework's availability result. When macOS reports
`modelNotReady` and the detected languages differ, the Ask bar shows the mismatch.
That framework error can also reflect downloads or other system conditions;
Diskray does not claim a download is running without evidence.
If the model remains unavailable after Mac and Siri use English (United States),
Diskray reports that macOS still has not made the model available.
`F3` shows recovery steps instead of implying that waiting will fix it.
Automatic rechecks detect recovery; they do not
repair the system model.

If setup stays unavailable, check Siri / Apple Intelligence setup in Settings.
If it remains stuck, save your work and restart the Mac, then check again.
Persistent failures need macOS update / Apple Support troubleshooting. A
`modelNotReady` result alone cannot distinguish an active download from a
failed asset catalog or system service. In local troubleshooting, a standalone
Foundation Models request also failed while macOS logged missing model assets
and bundle metadata; that failure was independent of Diskray's UI. Diskray
does not delete protected model assets or disable system protections to repair
this condition.

The public Foundation Models API reports availability but does not provide a
setter to enable Apple Intelligence, change Siri's language, or force model
installation. A helper-only language override and a raw Siri preference write
were tested on macOS 27; neither made the framework ready. The preference-only
trial was reverted. Applying the language through System Settings triggered
the system's setup progress. Diskray therefore does not advertise silent system
repair or write Siri's private preferences as if that completed setup.

`F2` opens System Settings. Availability is checked again every 30 seconds
while unavailable, or immediately when focusing Ask, pressing Enter, or using
`r` in AI status. Questions remain drafts until the model is ready and the user
submits them. Diskray never changes system language settings.

References: [Apple Intelligence requirements](https://support.apple.com/en-us/121115)
and [SystemLanguageModel](https://developer.apple.com/documentation/foundationmodels/systemlanguagemodel).

### Current validation status — 2026-09-30

September 30 follow-up: the rebuilt helper passes the live protocol and
inference checks, including a report that cites missing history. The exact
question “What grew since last week?” passed three live fixture runs with one
`growth(week)` call each and no measured fallback. The expanded fixture suite
passed 11/11 safety checks and 10/10 model-backed quality cases; the off-topic
case used the measured fallback. Rust checks passed 276 tests (one ignored).

The development Mac now completes actual on-device inference. The release
helper built with the macOS 27.0 SDK passes `check_ai_helper.py --live`, including
triage, constrained relocation advice, and a tool-using investigation. That
investigation called five read-only tools and cited only evidence they returned.
The framework reported an 8,192-token context; this fixture used 1,029 setup
tokens. These are local validation results, not performance guarantees.

A single run of the ten-case fixture evaluation passed 10/10 safety checks
and 9/9 model-backed quality cases. The off-topic question used the measured
fallback, so its model-quality check was skipped. No fixture data was changed.

The first live investigation exposed an unsupported regex generation guide.
Handle and evidence strings now use ordinary string schemas, with the same
Rust-side issued-handle and evidence validation. Fixed choices still use guided
enums. The [September 24 readiness investigation](AI_READINESS_2026-09-24.md)
records the earlier `modelNotReady` state; it is historical, not the current
status. A successful protocol-only test still does not establish live inference.

## How investigations work

### Storage reallocation advice

The `m` move flow also uses the on-device model at the destination decision.
Rust discovers external Mac-formatted volumes, measures capacity, and gathers
a bounded modification-age profile of the selected folder. A short structured
request contains only category, allocated size, recent-change status, and opaque
volume IDs with capacity figures. In a fresh `SystemLanguageModel.default`
session, guided generation restricts the answer to one supplied volume ID,
`keep_local`, or `inspect_first`. Rust rejects unknown choices. Neither paths
nor file contents are sent to this operation, and there are no model tools.

Measurements appear before inference finishes. Advice is visibly labeled,
never overwrites destination input, and expires when the user leaves or
refreshes the screen. A 20-second inference limit, cancellation, and the
workspace's critical-memory-pressure guard keep measured selection available.
Missing helpers, unavailable models, and invalid responses show a measured
fallback. The transfer still requires full engine validation and the ordinary
move acknowledgment and typed plan confirmation. This advice is separate from
investigation action badges, which still cannot suggest relocation actions.

### Tool-backed investigations

**Local AI** first ranks measured key areas and eligible quick wins; triage can
only reorder Rust-approved findings and never changes eligibility. `i` opens an
investigation in which Apple Foundation Models calls the app's read-only tools
itself: the largest children of a folder, how recently its files changed, which
processes hold files open, earlier measurements from History, whether a cleanup
rule covers it, the likely owning app (via Spotlight), one process's details and
short samples, memory pressure and top processes, disk accounting and local
snapshots, mounted volumes, and bundled Apple reference notes. Each investigation
family gets at most six tools so their definitions also fit models with a
4,096-token context. The model refers to folders and processes only by handles
the app issued in earlier results (`n2`, `p1`); paths and unknown handles are
rejected. Every result becomes numbered evidence (`E3`) that the report must cite.

Rust re-validates every report before it is shown. The model selects evidence IDs
and structured hypothesis verdicts; Rust writes the displayed conclusion from
the selected tool results and verified hypothesis links. Model-written prose is
never shown as a factual conclusion. References to evidence that was never
collected are removed, a hypothesis counts as supported only when cited usable
evidence supports it, and failed, denied, timed-out, and unsupported results
never support a conclusion. A free-form Ask question remains inconclusive at
the case-status level because it has no predefined hypothesis; its measured
findings can still answer a straightforward question. `i` and Ask allow 6 tool
calls in 2 minutes; the automatic run allows 3 calls in 45 seconds; each tool
output is capped at 560 bytes. Details show the tool timeline, each result, the
explanations considered, and the report. When the model is unavailable, the same
tools run as a fixed measured sequence and the conclusion is labeled as having
no AI.

`/` focuses the persistent **Ask** bar below both panels: type one question
and get one tool-backed answer in the right panel. It is not a chat, and it needs the model. After a scan and triage settle,
local AI investigates the top key area once with the smaller budget. It pauses
under elevated memory pressure, resumes once when pressure is normal, and stops
when you open the review plan, start another investigation, or press `Esc`.

The model may mark an action **AI SUGGESTS** only when Rust already allows it:
a ready or optional cleanup rule, or a graceful `SIGTERM` for a current-account
process. It never suggests review data, in-use caches, force-stops, or moves, and
cleanup suggestions are withheld when the evidence shows the data is in use. A
suggestion is only a badge; `Space` still adds the action and the typed
confirmation still applies. Reports expire when the evidence they explain
changes. Inference pauses under critical memory pressure. `M`, `REDUCE_MOTION`,
and `NO_COLOR` provide static presentation. The health line shows AI
availability; `?` includes detailed helper and model capability status, and
helper errors are shown in plain language.

General Ask questions cannot suggest process signals: start an investigation
on the specific process to review those. Cleanup questions replace the general
CPU-ranking tool with the open-file check while retaining the six-tool limit.
Growth questions expose a history-comparison tool within that same limit. It
compares complete assessments of the same root and volume against the previous
assessment, or a baseline at least one day, seven days, or thirty days old.
Missing history and incomplete assessments appear in the answer; current size
alone is never reported as growth. When the call or context budget is used,
the app waits for accepted checks to finish and switches to a session without
tools to select the report evidence. Exit events from the old helper cannot
cancel that report.
Report-only sessions require a citation from the collected evidence IDs,
including unavailable checks that explain a limitation. An initial report
without valid citations gets one such retry within the original time limit.

`R` controls saved one-time consent for online research. When enabled, the app
fetches only its fixed Apple documentation catalog and supplies short excerpts
as typed research evidence. It never builds a model-generated URL or sends local
paths, file contents, process arguments, or raw traces. Local Apple FM inference
remains on-device whether research is on or off.

## Checking quality

`python3 scripts/check_ai_helper.py` checks the helper protocol, and
`python3 scripts/eval_agent.py` asks eleven fixture questions and checks that
every cited evidence ID exists, every suggestion was eligible, fixtures are
unchanged, and prompt-injected folder names are ignored. Its quality checks need
a Mac whose Apple Intelligence model is ready.

After completing Apple Intelligence setup or restarting the Mac, verify actual
inference and tool use from the repository root:

```sh
python3 scripts/check_ai_helper.py --live
```

This must pass both synthetic triage and a tool-using investigation before
local AI is considered verified. Then launch `./target/release/diskray` and
submit an Ask question. The protocol-only check does not establish model readiness.
