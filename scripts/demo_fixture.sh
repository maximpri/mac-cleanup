#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Build a synthetic home folder for demos and screenshots, and print its path.
# Files are APFS clones of one seed file (`cp -c`), so the fixture reports
# gigabytes of allocated space while using little real disk.
#
#   DEMO=$(sh scripts/demo_fixture.sh) && diskray why --volume "$DEMO" --no-save
set -eu
root="${1:-${TMPDIR:-/tmp}/diskray-demo}"
rm -rf "$root"
mkdir -p "$root"
seed="$root/.seed"
dd if=/dev/urandom of="$seed" bs=1m count=256 2>/dev/null

# clone PATH COUNT: COUNT clones of the 256 MB seed inside PATH
clone() {
  mkdir -p "$root/$1"
  i=0
  while [ "$i" -lt "$2" ]; do
    cp -c "$seed" "$root/$1/blob-$i"
    i=$((i + 1))
  done
}
# age PATH DAYS: backdate everything inside PATH
age() {
  stamp=$(date -v-"$2"d +%Y%m%d%H%M)
  find "$root/$1" -exec touch -h -t "$stamp" {} +
}

clone "Library/Caches/pip/http" 3                          # quick win
clone "Library/Caches/Homebrew/downloads" 4                # quick win
clone "Library/Developer/Xcode/DerivedData/App-abc" 6      # quick win
clone "Library/Developer/Xcode/iOS DeviceSupport/17.4 (21E213)" 20
clone "Library/Developer/CoreSimulator/Devices/5F1C" 12
clone ".npm/_cacache/content-v2" 8
clone "Movies/Final Cut Projects" 24
clone "Documents/Archive" 6
clone "code/old-landing/node_modules/.cache" 3
echo '{"name": "old-landing"}' > "$root/code/old-landing/package.json"
age "code/old-landing" 140
clone "code/api/target/debug" 5
echo '[package]' > "$root/code/api/Cargo.toml"
age "code/api" 95
cp -c "$seed" "$root/Downloads.partial"
mkdir -p "$root/Downloads" && mv "$root/Downloads.partial" "$root/Downloads/installer.dmg.crdownload"
age "Downloads" 30
mkdir -p "$root/.Trash" && cp -c "$seed" "$root/.Trash/old-export.mov"

rm -f "$seed"
echo "$root"
