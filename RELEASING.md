# Release process

Diskray is distributed from source through GitHub Releases and the
Homebrew tap (`maximpri/homebrew-diskray`). It is never published to crates.io.

## Prepare

1. Confirm the working tree contains only intended changes.
2. Update the version in `Cargo.toml` and run `cargo update -w` so `Cargo.lock`
   records the same package version.
3. Move the relevant `CHANGELOG.md` entries from `Unreleased` into a dated
   version section.
4. Run:

   ```bash
   cargo fmt --all --check
   cargo clippy --all-targets --all-features --locked -- -D warnings
   cargo test --all-targets --locked
   cargo doc --no-deps --document-private-items
   sh scripts/build-release.sh
   python3 scripts/check_ai_helper.py
   python3 scripts/smoke_tui.py
   python3 scripts/check_mcp.py
   python3 scripts/eval_agent.py        # quality checks need Apple Intelligence
   cargo package --locked
   ```

5. Exercise analysis mode and every confirmation/cancel path on a disposable
   macOS account. Never validate a release by deleting valuable user data.

## Tag and publish

1. Commit the version and changelog changes.
2. Create a signed `vMAJOR.MINOR.PATCH` tag matching `Cargo.toml` and push it.
3. `.github/workflows/release.yml` then:
   - reruns the full CI gate;
   - builds `diskray-VERSION.tar.gz` with `git archive` plus a `.sha256` file;
   - creates the GitHub Release with the matching `CHANGELOG.md` section;
   - opens a pull request in `maximpri/homebrew-diskray` that updates the
     formula's `url` and `sha256` (when the tap is enabled, below).
4. Merge the tap pull request after its CI (`brew install --build-from-source`,
   `brew test`, `brew audit --strict`) passes.

## Homebrew tap (one-time setup)

1. Create the public repository `maximpri/homebrew-diskray` and copy
   `packaging/homebrew/diskray.rb` to `Formula/diskray.rb` in it.
2. Create a fine-grained token with *Contents* and *Pull requests* write access
   to that repository only. Save it as the `TAP_TOKEN` secret in this
   repository, and set the repository variable `TAP_ENABLED` to `true`.
3. Users install with `brew install maximpri/diskray/diskray`. The formula
   builds from source; it adds the AI helper only on Apple silicon with macOS
   26 or later and a FoundationModels SDK.

The AI-enabled release targets Apple silicon and macOS 26+. Package both
`diskray` and `diskray-ai` in the same directory. Run
`python3 scripts/check_ai_helper.py --live` on a compatible development Mac,
and complete the model-quality and performance gates in
`docs/IMPLEMENTATION_PLAN.md` before promoting this implementation to a release.
CI verifies compilation and availability framing; hosted runners need not have
Apple Intelligence assets. Rust-only Intel builds have no local AI provider.
Record SHA-256
checksums beside every artifact. Code signing and notarization require an Apple
Developer identity and should be completed before calling a binary trusted or
installer-ready.

Do not upload binaries built from a dirty working tree, and do not commit
signing certificates, notarization credentials, or generated cleanup reports.
