#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Build diskray, plus the on-device AI helper when this Mac can build it:
# Apple silicon, macOS 26 or later, and an SDK that contains FoundationModels.
set -eu
cd "$(dirname "$0")/.."
cargo build --release --locked

reason=""
if [ "$(uname -m)" != "arm64" ]; then
  reason="the AI helper needs Apple silicon"
elif [ "$(sw_vers -productVersion | cut -d. -f1)" -lt 26 ]; then
  reason="the AI helper needs macOS 26 or later"
elif ! sdk=$(xcrun --show-sdk-path 2>/dev/null) || [ ! -d "$sdk/System/Library/Frameworks/FoundationModels.framework" ]; then
  reason="the active SDK has no FoundationModels framework (install Xcode 26 or later)"
fi

if [ -n "$reason" ]; then
  rm -f target/release/diskray-ai
  echo "Built target/release/diskray without local AI: $reason."
  echo 'Measured findings, why, mcp, and review all work without it.'
  exit 0
fi

xcrun swiftc -O -parse-as-library -target arm64-apple-macosx26.0 native/AIHelper.swift -o target/release/diskray-ai
echo 'Built target/release/diskray and target/release/diskray-ai. Distribute both together.'
