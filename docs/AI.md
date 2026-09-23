# Local AI

Diskray uses Apple Foundation Models, the on-device model in Apple
Intelligence, to investigate and explain measured findings. Nothing is sent to
a cloud service, there is no account or API key, and the model can never add a
cleanup target or run an action.

Requirements: Apple silicon, macOS 26 or later, an SDK containing
FoundationModels to build the `diskray-ai` helper, and Apple Intelligence turned
on with its model downloaded. Without it, every measured finding still works and
investigations run as a fixed sequence of the same read-only tools.

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

Rust re-validates every report before it is shown. References to evidence that
was never collected are removed, a hypothesis counts as supported only when cited
usable evidence supports it, and failed, denied, timed-out, and unsupported results
never support a conclusion. `i` and Ask allow 6 tool calls in 2 minutes; the
automatic run allows 3 calls in 45 seconds; each tool output is capped at 560
bytes. Details show the tool timeline, each result, the explanations considered,
and the report. When the model is unavailable, the same tools run as a fixed
measured sequence and the conclusion is labeled as having no AI.

`/` opens **Ask**: type one question and get one tool-backed answer on its own
page. It is not a chat, and it needs the model. After a scan and triage settle,
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
