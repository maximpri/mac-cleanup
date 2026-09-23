# "System Data is 180 GB": where it actually went

Launch post outline. Target: 1,800–2,200 words, one hero GIF, three
screenshots, runnable commands throughout. Every number shown should come from
a real `diskray why --json` run (fixture or a volunteer Mac), never invented.

## 1. The hook (150 words)

- The Settings → Storage bar: a grey "System Data" slab nobody can open.
- Cleaners answer "delete these 40 caches". The honest answer starts with
  "here is what that space *is*".
- One-line pitch: Diskray is an X-ray for your Mac's disk. Understand before
  you delete.

## 2. Where "System Data" really lives (500 words)

- APFS basics: volumes share a container; "used" includes other volumes
  (VM, Preboot, Recovery) and local Time Machine snapshots.
- Why folder walks never add up: snapshots, purgeable space, protected
  folders, files the scan cannot read. Show the capacity bar
  (█ measured · ▓ hidden · ▒ not measured · ░ free) from the Why screen.
- `tmutil listlocalsnapshots /` and what deleting a snapshot really frees.
- Screenshot: `diskray why` on a real disk with its hidden-space line.

## 3. Not every cache is equal (350 words)

- Quick wins vs. review-only data: pip or Homebrew caches (rebuildable) vs.
  Xcode device support, simulators, Telegram's local data (valuable state).
- Evidence per item: age profile, open files, owning app, growth since last
  week, and why "large" never means "waste".
- Rule packs as data: `rules/*.toml`, how to contribute one.

## 4. Tool calling in a 4K-token context (450 words)

- Apple Foundation Models on-device: no account, no network, 4,096 tokens.
- Six tools per investigation family, so definitions fit; outputs capped at
  560 bytes; handles (`n2`, `p1`) instead of paths the model could mistype.
- Every result becomes evidence `E3`; Rust re-validates the report and drops
  citations of evidence that was never collected.
- Lessons: small models reason well over small, typed facts; they do badly
  with raw `du` output.
- Screenshot: details pane with MEASURED → INVESTIGATION → REPORT → ACTIONS.

## 5. Why the model can't delete anything (250 words)

- Suggestions only for actions Rust already allows; a badge, not an action.
- Typed confirmations, whitelist inside caches, identity checks just before
  removal, deletion log. Link to docs/SAFETY.md.
- The eval harness: prompt-injection folder names, ineligible suggestions,
  fixture-unchanged checks.

## 6. Letting coding agents look, not touch (250 words)

- `diskray mcp`: thirteen read-only tools and `propose_cleanup`, which only
  saves a proposal; `diskray review` re-measures and asks you.
- Why this beats an agent improvising `du | sort` and `rm -rf`.
- GIF: Claude Code answering "why is my disk full?" with Diskray tools.

## 7. Honest comparison and close (150 words)

- Mole is faster and broader for bulk cleaning; use it for that.
- Diskray is for understanding: hidden space, evidence, growth, verification.
- `brew install maximpri/diskray/diskray`, `diskray why`, GPL-3.0, issues and
  rule packs welcome.

## Assets checklist

- [ ] `docs/demo/hero.gif` (vhs), `why.gif`, `mcp.gif`
- [ ] Screenshot: Why screen on a real disk (redact the user name)
- [ ] Screenshot: details pane with an on-device AI report
- [ ] Numbers from `diskray why --json` for sections 2 and 3
