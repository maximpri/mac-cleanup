#!/usr/bin/env bash

# Conservative macOS cache cleanup.
#
# The default action is read-only analysis. Cleanup is limited to the exact
# allowlist below, refuses to run as root, skips symlinks, checks related
# processes, and asks before permanently deleting cache contents.

set -u
set -o pipefail

SCRIPT_NAME="$(basename "$0")"
MODE="analyze"
ASSUME_YES=0
INCLUDE_REINSTALLABLE=0
VERBOSE=0

usage() {
  cat <<EOF
Usage: $SCRIPT_NAME [options]

Analyze known, regenerable macOS caches (default):
  ./$SCRIPT_NAME

Interactively clean routine caches:
  ./$SCRIPT_NAME --clean

Also offer large tools/runtimes that must be downloaded again:
  ./$SCRIPT_NAME --clean --include-reinstallable

Options:
  --analyze                 Read-only report (default)
  --clean                   Clean eligible caches after confirmation
  --include-reinstallable   Include Playwright, npm npx, and tool runtimes
  --yes                     Confirm all eligible items (still applies safeguards)
  --verbose                 Show skipped and missing candidates
  -h, --help                Show this help

Cleanup is permanent. Close Telegram, Chrome, TradingView, ZCode, package
managers, and development tools first. Never run this script with sudo.
EOF
}

while (($#)); do
  case "$1" in
    --analyze)
      MODE="analyze"
      ;;
    --clean)
      MODE="clean"
      ;;
    --include-reinstallable)
      INCLUDE_REINSTALLABLE=1
      ;;
    --yes)
      ASSUME_YES=1
      ;;
    --verbose)
      VERBOSE=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'Unknown option: %s\n\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

if [[ "$(uname -s)" != "Darwin" && "${MAC_CLEANUP_TESTING:-0}" != "1" ]]; then
  printf 'Error: this script is intended for macOS.\n' >&2
  exit 1
fi

if ((EUID == 0)); then
  printf 'Error: do not run this script with sudo or as root.\n' >&2
  exit 1
fi

CURRENT_USER="$(id -un)"
ACCOUNT_HOME=""

if [[ "${MAC_CLEANUP_TESTING:-0}" == "1" ]]; then
  ACCOUNT_HOME="${HOME:-}"
elif command -v dscl >/dev/null 2>&1; then
  ACCOUNT_HOME="$(dscl . -read "/Users/$CURRENT_USER" NFSHomeDirectory 2>/dev/null | awk '{print $2}')"
fi

if [[ -z "$ACCOUNT_HOME" ]]; then
  # `id -P` uses the macOS master.passwd format. Reading from the end avoids
  # assumptions about optional account fields; home precedes the login shell.
  ACCOUNT_HOME="$(id -P "$CURRENT_USER" 2>/dev/null | awk -F: 'NF >= 2 {print $(NF - 1)}')"
fi

if [[ -z "$ACCOUNT_HOME" ]]; then
  ACCOUNT_HOME="$(awk -F: -v user="$CURRENT_USER" '$1 == user { print $6; exit }' /etc/passwd 2>/dev/null)"
fi

if [[ -z "$ACCOUNT_HOME" || "$ACCOUNT_HOME" == "/" || ! -d "$ACCOUNT_HOME" ]]; then
  printf 'Error: could not determine a safe home directory for %s.\n' "$CURRENT_USER" >&2
  exit 1
fi

# Each index describes one exact allowlisted directory.
CACHE_PATHS=(
  "$ACCOUNT_HOME/Library/Caches/pip"
  "$ACCOUNT_HOME/Library/Caches/node-gyp"
  "$ACCOUNT_HOME/Library/Caches/Homebrew"
  "$ACCOUNT_HOME/Library/Caches/com.apple.python"
  "$ACCOUNT_HOME/Library/Caches/tradingview-desktop-updater"
  "$ACCOUNT_HOME/Library/Caches/@zcodedesktop-updater"
  "$ACCOUNT_HOME/Library/Caches/ru.keepcoder.Telegram"
  "$ACCOUNT_HOME/Library/Caches/Google"
  "$ACCOUNT_HOME/.npm/_cacache"
  "$ACCOUNT_HOME/.cache/opencode"
  "$ACCOUNT_HOME/Library/Caches/ms-playwright"
  "$ACCOUNT_HOME/Library/Caches/ms-playwright-go"
  "$ACCOUNT_HOME/.npm/_npx"
  "$ACCOUNT_HOME/.cache/chrome-devtools-mcp"
  "$ACCOUNT_HOME/.cache/codex-runtimes"
)

CACHE_LABELS=(
  "pip cache"
  "node-gyp cache"
  "Homebrew cache"
  "Python cache"
  "TradingView updater"
  "ZCode updater"
  "Telegram cache"
  "Google app cache"
  "npm package cache"
  "OpenCode cache"
  "Playwright browsers"
  "Playwright Go browsers"
  "npm npx packages"
  "Chrome DevTools MCP"
  "Codex runtimes"
)

# routine: normal cache cleanup; reinstall: large assets that are downloaded again.
CACHE_TIERS=(
  "routine"
  "routine"
  "routine"
  "routine"
  "routine"
  "routine"
  "routine"
  "routine"
  "routine"
  "routine"
  "reinstall"
  "reinstall"
  "reinstall"
  "reinstall"
  "reinstall"
)

# Extended regular expressions passed to pgrep -f. Empty means no process check.
PROCESS_PATTERNS=(
  "[p]ip(3)? (install|download|cache)"
  "[n]ode-gyp"
  "[b]rew (install|upgrade|update|cleanup)"
  "[p]ython(3)? .*pip"
  "/TradingView.app/|[t]radingview-desktop-updater"
  "/Zed.app/|/[Zz][Cc]ode.app/|@[z]codedesktop-updater"
  "/Telegram.app/Contents/MacOS/Telegram"
  "/Google Chrome.app/|/Google Drive.app/|[k]eystone.*Google"
  "[n]pm |[n]px "
  "[o]pencode"
  "[p]laywright|[m]s-playwright"
  "[p]laywright|[m]s-playwright-go"
  "[n]pm |[n]px "
  "[c]hrome-devtools-mcp"
  "/Codex.app/|[c]odex-runtimes"
)

CACHE_NOTES=(
  "downloaded Python packages"
  "downloaded Node build files"
  "downloaded Homebrew files"
  "Python-generated cache files"
  "obsolete updater downloads"
  "obsolete updater downloads"
  "messages remain; media thumbnails reload"
  "browser/app cache reloads"
  "downloaded npm packages"
  "OpenCode regenerates this cache"
  "browser binaries download again"
  "browser binaries download again"
  "temporary npx packages install again"
  "browser/runtime downloads may return"
  "Codex runtime downloads may return"
)

format_kb() {
  awk -v kb="$1" 'BEGIN {
    if (kb >= 1048576) printf "%.1f GiB", kb / 1048576;
    else if (kb >= 1024) printf "%.1f MiB", kb / 1024;
    else printf "%d KiB", kb;
  }'
}

directory_kb() {
  local target="$1"
  local size

  if [[ ! -d "$target" || -L "$target" ]]; then
    printf '0'
    return
  fi

  size="$(du -sk "$target" 2>/dev/null | awk 'NR == 1 {print $1}')"
  printf '%s' "${size:-0}"
}

free_kb() {
  df -k "$ACCOUNT_HOME" 2>/dev/null | awk 'NR == 2 {print $4}'
}

is_allowlisted() {
  local target="$1"
  local allowed

  for allowed in "${CACHE_PATHS[@]}"; do
    [[ "$target" == "$allowed" ]] && return 0
  done
  return 1
}

process_is_running() {
  local pattern="$1"
  [[ -z "$pattern" ]] && return 1
  pgrep -f "$pattern" >/dev/null 2>&1
}

eligible_status() {
  local index="$1"
  local target="${CACHE_PATHS[$index]}"
  local tier="${CACHE_TIERS[$index]}"
  local pattern="${PROCESS_PATTERNS[$index]}"

  if [[ ! -e "$target" ]]; then
    printf 'not present'
  elif [[ -L "$target" ]]; then
    printf 'SKIP symlink'
  elif [[ ! -d "$target" ]]; then
    printf 'SKIP not a directory'
  elif process_is_running "$pattern"; then
    printf 'SKIP app/process running'
  elif [[ "$tier" == "reinstall" && "$INCLUDE_REINSTALLABLE" != "1" ]]; then
    printf 'opt-in'
  else
    printf 'ready'
  fi
}

confirm_delete() {
  local label="$1"
  local size="$2"
  local reply

  if ((ASSUME_YES)); then
    return 0
  fi

  if [[ ! -t 0 ]]; then
    printf '  SKIP: confirmation requires a terminal (or use --yes).\n'
    return 1
  fi

  printf '  Permanently clear %s (%s)? [y/N] ' "$label" "$size"
  IFS= read -r reply
  [[ "$reply" == "y" || "$reply" == "Y" || "$reply" == "yes" || "$reply" == "YES" ]]
}

clear_directory_contents() {
  local target="$1"
  local entries=()

  # Revalidate immediately before deletion. Never follow or clear a symlink.
  if ! is_allowlisted "$target"; then
    printf '  REFUSED: path is not on the exact allowlist: %s\n' "$target" >&2
    return 1
  fi
  if [[ ! -d "$target" || -L "$target" ]]; then
    printf '  REFUSED: target changed or is not a real directory: %s\n' "$target" >&2
    return 1
  fi

  shopt -s dotglob nullglob
  entries=("$target"/*)
  shopt -u dotglob nullglob

  if ((${#entries[@]} == 0)); then
    return 0
  fi

  rm -rf -- "${entries[@]}"
}

printf 'macOS cache cleanup analysis\n'
printf 'Home: %s\n' "$ACCOUNT_HOME"
printf 'Mode: %s\n\n' "$MODE"
printf '%-25s %10s  %-24s  %s\n' "Candidate" "Size" "Status" "Effect"
printf '%-25s %10s  %-24s  %s\n' "-------------------------" "----------" "------------------------" "------"

TOTAL_KB=0
ROUTINE_READY_KB=0
REINSTALL_READY_KB=0
INDEX=0

while ((INDEX < ${#CACHE_PATHS[@]})); do
  TARGET="${CACHE_PATHS[$INDEX]}"
  SIZE_KB="$(directory_kb "$TARGET")"
  STATUS="$(eligible_status "$INDEX")"
  SIZE_HUMAN="$(format_kb "$SIZE_KB")"

  TOTAL_KB=$((TOTAL_KB + SIZE_KB))
  if [[ "$STATUS" == "ready" ]]; then
    if [[ "${CACHE_TIERS[$INDEX]}" == "routine" ]]; then
      ROUTINE_READY_KB=$((ROUTINE_READY_KB + SIZE_KB))
    else
      REINSTALL_READY_KB=$((REINSTALL_READY_KB + SIZE_KB))
    fi
  fi

  if ((SIZE_KB > 0)) || ((VERBOSE)); then
    printf '%-25s %10s  %-24s  %s\n' \
      "${CACHE_LABELS[$INDEX]}" "$SIZE_HUMAN" "$STATUS" "${CACHE_NOTES[$INDEX]}"
    if ((VERBOSE)); then
      printf '  %s\n' "$TARGET"
    fi
  fi
  INDEX=$((INDEX + 1))
done

printf '\nFound in known candidates: %s\n' "$(format_kb "$TOTAL_KB")"
printf 'Eligible in this run:       %s\n' "$(format_kb "$((ROUTINE_READY_KB + REINSTALL_READY_KB))")"

if ((INCLUDE_REINSTALLABLE == 0)); then
  printf 'Reinstallable downloads are analysis-only; add --include-reinstallable to clean them.\n'
fi

if [[ "$MODE" == "analyze" ]]; then
  printf '\nNo files were changed. Run %s --clean to review each eligible item.\n' "$SCRIPT_NAME"
  exit 0
fi

printf '\nCleanup phase: only entries marked ready are offered. Deletion is permanent.\n'
FREE_BEFORE_KB="$(free_kb)"
FREED_KB=0
FAILURES=0
INDEX=0

while ((INDEX < ${#CACHE_PATHS[@]})); do
  TARGET="${CACHE_PATHS[$INDEX]}"
  STATUS="$(eligible_status "$INDEX")"

  if [[ "$STATUS" != "ready" ]]; then
    if ((VERBOSE)) && [[ -e "$TARGET" ]]; then
      printf 'Skip %-25s (%s)\n' "${CACHE_LABELS[$INDEX]}" "$STATUS"
    fi
    INDEX=$((INDEX + 1))
    continue
  fi

  SIZE_BEFORE_KB="$(directory_kb "$TARGET")"
  if ((SIZE_BEFORE_KB == 0)); then
    INDEX=$((INDEX + 1))
    continue
  fi

  # Check again directly before prompting/deleting in case an app just started.
  if process_is_running "${PROCESS_PATTERNS[$INDEX]}"; then
    printf 'Skip %-25s (related process started)\n' "${CACHE_LABELS[$INDEX]}"
    INDEX=$((INDEX + 1))
    continue
  fi

  if ! confirm_delete "${CACHE_LABELS[$INDEX]}" "$(format_kb "$SIZE_BEFORE_KB")"; then
    INDEX=$((INDEX + 1))
    continue
  fi

  # Final process check narrows the race window before deletion.
  if process_is_running "${PROCESS_PATTERNS[$INDEX]}"; then
    printf 'Skip %-25s (related process started)\n' "${CACHE_LABELS[$INDEX]}"
    INDEX=$((INDEX + 1))
    continue
  fi

  if clear_directory_contents "$TARGET"; then
    SIZE_AFTER_KB="$(directory_kb "$TARGET")"
    ITEM_FREED_KB=$((SIZE_BEFORE_KB - SIZE_AFTER_KB))
    ((ITEM_FREED_KB < 0)) && ITEM_FREED_KB=0
    FREED_KB=$((FREED_KB + ITEM_FREED_KB))
    printf '  Cleared %-25s reclaimed about %s\n' \
      "${CACHE_LABELS[$INDEX]}" "$(format_kb "$ITEM_FREED_KB")"
  else
    FAILURES=$((FAILURES + 1))
    printf '  FAILED: %s\n' "${CACHE_LABELS[$INDEX]}" >&2
  fi

  INDEX=$((INDEX + 1))
done

FREE_AFTER_KB="$(free_kb)"
DISK_DELTA_KB=$((FREE_AFTER_KB - FREE_BEFORE_KB))
((DISK_DELTA_KB < 0)) && DISK_DELTA_KB=0

printf '\nCleanup complete. Cache contents removed: about %s.\n' "$(format_kb "$FREED_KB")"
printf 'Filesystem free-space increase:             about %s.\n' "$(format_kb "$DISK_DELTA_KB")"

if ((FAILURES > 0)); then
  printf '%d item(s) could not be fully cleaned. Review the messages above.\n' "$FAILURES" >&2
  exit 1
fi
