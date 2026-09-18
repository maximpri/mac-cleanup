# Release process

Mac Cleanup is distributed from source and may also be attached to GitHub
Releases as prebuilt macOS binaries. The crate is intentionally not published
to crates.io because another project already owns the `mac-cleanup` package
name there.

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
   cargo package --locked
   ```

5. Exercise analysis mode and every confirmation/cancel path on a disposable
   macOS account. Never validate a release by deleting valuable user data.

## Tag and publish

1. Commit the version and changelog changes.
2. Create a signed `vMAJOR.MINOR.PATCH` tag.
3. Push the commit and tag only after CI succeeds.
4. Create a GitHub Release from the tag and copy its changelog section into the
   release notes.

The AI-enabled release targets Apple silicon and macOS 26+. Package both
`mac-cleanup` and `mac-cleanup-ai` in the same directory. Run
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
