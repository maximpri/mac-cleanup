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

### Current validation status — 2026-09-24

On the development Mac, both languages are now `en-US` and the release helper
was rebuilt with the macOS 27.0 SDK. Apple still reports `modelNotReady`;
older helpers rebuilt on this Mac fail the same way. Helper protocol and tool
routing checks pass, but live inference and the tool-using model loop remain
unverified for this revision. A restart requested by macOS is deferred for the
user to perform manually. See the [readiness investigation](AI_READINESS_2026-09-24.md)
for the evidence and exact steps to resume. Matching languages and rebuilding
are not evidence of successful inference.

## How investigations work

**Local AI** first ranks measured key areas and eligible quick wins; triage can
only reorder Rust-approved findings and never changes eligibility. `i` opens an
investigation in which Apple Foundation Models calls the app's read-only tools
itself: the largest children of a folder, how recently its files changed, which
processes hold files open, earlier measurements from History, whether a cleanup
rule covers it, the likely owning app (via Spotlight), one process's details and
short samples, memory pressure and top processes, disk accounting and local
snapshots, mounted volumes, and bundled Apple reference notes. Each investigation
family gets at most six tools so their definitions fit the on-device model's
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

`R` controls saved one-time consent for online research. When enabled, the app
fetches only its fixed Apple documentation catalog and supplies short excerpts
as typed research evidence. It never builds a model-generated URL or sends local
paths, file contents, process arguments, or raw traces. Local Apple FM inference
remains on-device whether research is on or off.

## Checking quality

`python3 scripts/check_ai_helper.py` checks the helper protocol, and
`python3 scripts/eval_agent.py` asks twelve fixture questions and checks that
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
