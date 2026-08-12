#!/usr/bin/env bash

# Convenience launcher for running the Ratatui application from a source checkout.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec cargo run --quiet --manifest-path "$SCRIPT_DIR/Cargo.toml" -- "$@"
