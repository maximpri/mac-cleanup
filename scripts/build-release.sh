#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --release --locked
xcrun swiftc -O -parse-as-library -target arm64-apple-macosx26.0 native/AIHelper.swift -o target/release/mac-cleanup-ai
echo 'Built target/release/mac-cleanup and target/release/mac-cleanup-ai. Distribute both together.'
