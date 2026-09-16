## Summary

Describe the user-visible change and why it belongs in Mac Cleanup.

## Safety impact

Describe changes to allowlists, paths, process checks, native commands,
confirmations, or deletion behavior. Write `None` when they are unaffected.

## Validation

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings`
- [ ] `cargo test --all-targets --locked`
- [ ] `cargo build --release --locked`
- [ ] Tests use disposable directories and do not delete real user data.
- [ ] User-visible behavior is documented in `README.md` and `CHANGELOG.md`.
- [ ] No personal paths, cache listings, credentials, or generated reports are included.
