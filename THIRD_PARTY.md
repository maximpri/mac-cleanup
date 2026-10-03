# Third-party dependencies

Diskray's own code is licensed under GPL-3.0-or-later. Dependencies retain
their own licenses and copyright notices; the project license does not
replace them. `Cargo.lock` records the exact Rust package versions and
registry checksums used to build a release.

## License policy

[`deny.toml`](deny.toml) checks the dependency graph for both supported macOS
architectures, including build and development dependencies. The allowlist
contains MIT, Apache-2.0, Unicode-3.0, and Zlib. GPL-3.0-or-later is approved
specifically for the `diskray` package. Unlicensed packages and expressions
that cannot be satisfied by this policy fail the check.

The current dependency graph uses these licenses individually or in SPDX
expressions. An `OR` permits a choice; an `AND` requires both licenses. In
particular, `unicode-ident` declares
`(MIT OR Apache-2.0) AND Unicode-3.0`: its Unicode notice must be retained
alongside the selected MIT or Apache notice. Some crates also offer
Unlicense, BSL-1.0, or the LLVM exception as alternatives; the policy uses
their MIT or Apache-2.0 option instead. Legacy slash-separated MIT/Apache
metadata is interpreted by cargo-deny, not by an ad hoc text search.

CI uses cargo-deny 0.20.2 through the commit-pinned upstream action. To run
the same check locally with that version installed:

```sh
cargo deny --locked check licenses
```

Review new license expressions and upstream license files whenever
dependencies change. Do not widen the allowlist just to make CI pass.
The check verifies the declared license policy; it does not generate a
distribution's notices or establish provenance for copied source code.

## Inspect the exact inventory

Generate metadata from the release checkout without changing the lockfile:

```sh
cargo metadata --locked --format-version 1 > dependency-metadata.json
```

Each package entry contains its name, version, source, `license`, optional
`license_file`, and `manifest_path`. Inspect the dependency source directory
beside that manifest for its actual `LICENSE*`, `COPYING*`, `NOTICE*`, and
copyright statements. Metadata lists packages for other targets as well;
use the configured cargo-deny graph for the supported macOS builds. Keep
the metadata with release audit records, rather than committing local
absolute paths to this repository.

## Distribution and provenance

The release workflow currently publishes Diskray source, not prebuilt
binaries. It includes `Cargo.lock`, the project license, Rust source, Swift
helper source, rules, and build scripts. Cargo downloads dependencies from
their original registries during the build; those sources carry their own
notices.

Before distributing compiled binaries or a vendored source bundle, collect
the applicable license texts, copyright statements, and required notices
from the exact dependency versions used. Include those notices with the
distribution and provide its complete corresponding source as described in
[`RELEASING.md`](RELEASING.md). Preserve upstream notices verbatim; a list
of SPDX identifiers, this document, or links to registry pages is not a
replacement for them. Build tools and generated or embedded data also need
review when determining what enters an artifact.

The Swift helper uses Apple's system frameworks; this repository does not
redistribute Apple's frameworks or Apple Intelligence model assets. Third-party
source copied into the project must retain its original attribution and
license, with its origin and changes documented in the contribution.

The policy follows [cargo-deny's license-expression checks](https://embarkstudios.github.io/cargo-deny/checks/licenses/cfg.html).
See the [GNU license compatibility list](https://www.gnu.org/licenses/license-list.html)
and [GPL source-distribution guidance](https://www.gnu.org/licenses/gpl-faq.html#SourceAndBinaryOnDifferentSites)
when reviewing a release.
