# Contributing

Thank you for helping improve Mac Cleanup. Changes that affect deletion need
extra care because the program operates on real user data.

## Development setup

Mac Cleanup requires macOS and Rust 1.88 or newer. Fork and clone the
repository, then run:

```bash
cargo build
cargo test --all-targets
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
```

Use `cargo run -- --analyze` for manual testing. Analysis is read-only. Never
run the project or its tests with `sudo`.

## Safety requirements

Pull requests that add or change a cleanup target must:

- identify one exact path beneath a validated home or mounted-volume root;
- explain what is deleted, the user impact, and how data can be recovered;
- reject symlinked roots and symlinked parent components;
- retain the allowlisted directory and remove only its contents;
- block cleanup while the owning application or package manager is running;
- keep reinstallable downloads opt-in and personal/app-managed data in the
  guarded `REVIEW` flow;
- prefer a documented, non-interactive vendor command only when its scope can
  be confined to the displayed finding; and
- include tests that use temporary directories rather than a contributor's
  real home directory, caches, Trash, or mounted volumes.

Native commands must be launched directly, without a shell, and must have a
bounded timeout. If a native command can affect data outside the displayed
allowlisted path, it is not suitable for this project.

## Pull requests

Keep changes focused and update the README when behavior, controls, output, or
safety guarantees change. Add an entry under `Unreleased` in `CHANGELOG.md` for
user-visible changes.

Before opening a pull request, run the full validation sequence above plus:

```bash
cargo build --release --locked
cargo package --locked
```

Do not include real cache listings, usernames, home-directory paths, tokens,
cookies, browser profiles, or cleanup reports in issues, fixtures, or commits.

By contributing, you agree that your contribution is licensed under the MIT
License.
