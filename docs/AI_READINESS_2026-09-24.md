# Apple model readiness investigation — 2026-09-24

## Result before restart

The local model is **not yet working**. The public Foundation Models API reports
`modelNotReady`, with context size zero. No successful inference has been observed
in this investigation.

## Changes applied

- With the user's confirmation, changed the Mac's primary language through
  System Settings from English (Canada) to English (United States). Siri was
  already English (United States). Both are now detected as `en-US`.
- Kept region Canada, Celsius, and metric measurements. English (Canada) and
  the other existing preferred languages remain in the list.
- macOS requested a restart after applying the language. Deferred the restart
  at the user's request; they will restart manually later. Siri's
  setup-in-progress notice disappeared, but the final pre-restart helper probe
  still reported `modelNotReady`, both languages `en-US`, and context size zero.
- Installed Apple's offered Command Line Tools for Xcode 27.0 update.
- Rebuilt the release with the macOS 27.0 SDK, without changing the global
  Xcode selection:

  ```sh
  DEVELOPER_DIR=/Library/Developer/CommandLineTools sh scripts/build-release.sh
  ```

## Evidence

Host: Mac16,10, 32 GiB RAM, macOS 27.0 (26A428), internal startup disk.

| Test | Result |
| --- | --- |
| Earliest helper, `af420f5`, rebuilt with SDK 26.5 | `modelNotReady` |
| Pre-working-tree helper, `b7e9950`, rebuilt with SDK 26.5 | `modelNotReady` |
| Current helper with SDK 26.5, both languages `en-US` | `modelNotReady` |
| Current helper rebuilt with SDK 27.0, both languages `en-US` | `modelNotReady` |
| Helper protocol, concurrent tool routing, call budget, cancellation checks | Passed |

Apple's `modelmanagerd` logs report missing model-catalog assets, including
`com.apple.fm.language.instruct_3b.base.generic_sparse`, and that the
`com.apple.fm.language.instruct_3b.fm_api_generic` bundle has no metadata.
These logs describe a model asset resolution failure; they do not establish
whether assets are still downloading or whether a restart will resolve it.

An earlier standalone Swift inference request also failed. An earlier attempt
to restart `generativeexperiencesd` through launchctl was rejected by System
Integrity Protection. No protected assets were deleted and protections were not
changed.

## Resume after restart

1. Check Siri / Apple Intelligence setup and confirm both languages remain
   English (United States).
2. From the repository root, run `python3 scripts/check_ai_helper.py --live`.
   Success requires actual
   synthetic triage and a tool-using investigation, not just a readiness label.
3. If successful, launch `./target/release/diskray` and submit an Ask question.
4. If still unavailable, inspect fresh model-catalog/download errors before
   making further changes. Do not present a wording change or a successful
   protocol self-test as working AI.

References: [Apple requirements](https://support.apple.com/en-us/121115),
[modelNotReady](https://developer.apple.com/documentation/foundationmodels/systemlanguagemodel/availability-swift.enum/unavailablereason/modelnotready),
[Apple's model asset troubleshooting discussion](https://developer.apple.com/forums/thread/787445).
