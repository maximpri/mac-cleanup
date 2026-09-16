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
   cargo build --release --locked
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

For prebuilt binaries, build separate `aarch64-apple-darwin` and
`x86_64-apple-darwin` artifacts on trusted macOS systems. Record SHA-256
checksums beside every artifact. Code signing and notarization require an Apple
Developer identity and should be completed before calling a binary trusted or
installer-ready.

Do not upload binaries built from a dirty working tree, and do not commit
signing certificates, notarization credentials, or generated cleanup reports.
