## Summary

Describe the user-visible change and why it belongs in Diskray.

## Safety impact

Describe changes to allowlists, paths, process checks, native commands,
confirmations, or deletion behavior. Write `None` when they are unaffected.

## Validation

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings`
- [ ] `cargo test --all-targets --locked`
- [ ] `python3 -m unittest discover -s scripts -p 'test_*.py' -v`
- [ ] Dependency changes pass `cargo deny --locked check licenses` and preserve upstream notices.
- [ ] `cargo build --release --locked`
- [ ] Tests use disposable directories and do not delete real user data.
- [ ] User-visible behavior is documented (`README.md` or `docs/`) and in `CHANGELOG.md`.
- [ ] No personal paths, cache listings, credentials, or generated reports are included.
