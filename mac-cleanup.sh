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
DISABLE_COLOR=0
TUI_ENABLED=0
CONFIRM_ALL=0

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
  --no-color                Disable terminal colors
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
    --no-color)
      DISABLE_COLOR=1
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

# The interface uses only ANSI styling and falls back to plain, line-oriented
# output when stdout is redirected. NO_COLOR is also honored for callers that
# prefer an unstyled terminal.
if [[ -t 1 && "${TERM:-dumb}" != "dumb" ]]; then
  TUI_ENABLED=1
fi

if ((TUI_ENABLED)) && ((DISABLE_COLOR == 0)) && [[ -z "${NO_COLOR:-}" ]]; then
  C_RESET=$'\033[0m'
  C_BOLD=$'\033[1m'
  C_DIM=$'\033[2m'
  C_GREEN=$'\033[32m'
  C_YELLOW=$'\033[33m'
  C_RED=$'\033[31m'
  C_CYAN=$'\033[36m'
else
  C_RESET=""
  C_BOLD=""
  C_DIM=""
  C_GREEN=""
  C_YELLOW=""
  C_RED=""
  C_CYAN=""
fi

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

ui_header() {
  printf '\n%sMac Cleanup%s\n' "$C_BOLD" "$C_RESET"
  printf '%sSafe cleanup for known, regenerable macOS caches%s\n' "$C_DIM" "$C_RESET"
  printf '\n  %-10s %s\n' "Mode" "$1"
  printf '  %-10s %s\n' "Home" "$ACCOUNT_HOME"
}

ui_section() {
  printf '\n%s%s%s\n' "$C_BOLD" "$1" "$C_RESET"
}

ui_progress() {
  local current="$1"
  local total="$2"
  local action="$3"
  local label="$4"
  local width=20
  local filled=0
  local empty=0
  local bar=""

  if ((TUI_ENABLED)); then
    if ((total > 0)); then
      filled=$((current * width / total))
    fi
    empty=$((width - filled))
    while ((filled > 0)); do
      bar="${bar}#"
      filled=$((filled - 1))
    done
    while ((empty > 0)); do
      bar="${bar}-"
      empty=$((empty - 1))
    done
    printf '\r\033[2K  %s[%s]%s %2d/%d  %s %s' \
      "$C_CYAN" "$bar" "$C_RESET" "$current" "$total" "$action" "$label"
  elif ((VERBOSE)) || [[ "$action" != "Scanning" ]]; then
    printf '  [%d/%d] %s %s\n' "$current" "$total" "$action" "$label"
  fi
}

ui_progress_done() {
  if ((TUI_ENABLED)); then
    printf '\r\033[2K'
  fi
  printf '  %s[done]%s %s\n' "$C_GREEN" "$C_RESET" "$1"
}

ui_status_row() {
  local status="$1"
  local label="$2"
  local size="$3"
  local note="$4"
  local word=""
  local color=""

  case "$status" in
    ready)
      word="READY"
      color="$C_GREEN"
      ;;
    opt-in)
      word="OPTIONAL"
      color="$C_YELLOW"
      ;;
    "SKIP app/process running")
      word="IN USE"
      color="$C_YELLOW"
      ;;
    "SKIP symlink")
      word="SYMLINK"
      color="$C_RED"
      ;;
    "SKIP not a directory")
      word="INVALID"
      color="$C_RED"
      ;;
    "not present")
      word="MISSING"
      color="$C_DIM"
      ;;
    *)
      word="SKIP"
      color="$C_YELLOW"
      ;;
  esac

  printf '  %s%-10s%s %-25s %10s\n' \
    "$color" "$word" "$C_RESET" "$label" "$size"
  printf '  %-10s %s%s%s\n' "" "$C_DIM" "$note" "$C_RESET"
}

ui_result() {
  local kind="$1"
  local message="$2"
  local color="$C_GREEN"

  [[ "$kind" == "skip" ]] && color="$C_YELLOW"
  [[ "$kind" == "error" ]] && color="$C_RED"
  printf '  %s[%s]%s %s\n' "$color" "$kind" "$C_RESET" "$message"
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
  local note="$3"
  local position="$4"
  local total="$5"
  local reply=""

  if ((ASSUME_YES)) || ((CONFIRM_ALL)); then
    return 0
  fi

  if [[ ! -t 0 ]]; then
    ui_result "skip" "Confirmation requires a terminal; use --yes for unattended cleanup."
    return 1
  fi

  printf '\n  %s%d of %d%s  %s%s%s  %s\n' \
    "$C_DIM" "$position" "$total" "$C_RESET" "$C_BOLD" "$label" "$C_RESET" "$size"
  printf '  %s%s%s\n' "$C_DIM" "$note" "$C_RESET"

  while true; do
    printf '  Clear this cache? %s[y]%ses  %s[n]%so  %s[a]%sll remaining  %s[q]%suit: ' \
      "$C_CYAN" "$C_RESET" "$C_CYAN" "$C_RESET" \
      "$C_CYAN" "$C_RESET" "$C_CYAN" "$C_RESET"
    if ! IFS= read -r reply; then
      printf '\n'
      return 2
    fi
    case "$reply" in
      y|Y|yes|YES)
        return 0
        ;;
      ""|n|N|no|NO)
        return 1
        ;;
      a|A|all|ALL)
        CONFIRM_ALL=1
        return 0
        ;;
      q|Q|quit|QUIT)
        return 2
        ;;
      *)
        printf '  Please enter y, n, a, or q.\n'
        ;;
    esac
  done
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

if [[ "$MODE" == "clean" ]]; then
  MODE_LABEL="Cleanup with safety checks"
else
  MODE_LABEL="Analysis only (no changes)"
fi

ui_header "$MODE_LABEL"
ui_section "1. Scan"
printf '  Checking %d allowlisted locations for cache data and active apps...\n' "${#CACHE_PATHS[@]}"

TOTAL_KB=0
ROUTINE_READY_KB=0
REINSTALL_READY_KB=0
OPT_IN_KB=0
KNOWN_COUNT=0
DISPLAY_COUNT=0
READY_COUNT=0
OPT_IN_COUNT=0
IN_USE_COUNT=0
UNSAFE_COUNT=0
CACHED_SIZES=()
CACHED_STATUSES=()
INDEX=0

while ((INDEX < ${#CACHE_PATHS[@]})); do
  TARGET="${CACHE_PATHS[$INDEX]}"
  ui_progress "$((INDEX + 1))" "${#CACHE_PATHS[@]}" "Scanning" "${CACHE_LABELS[$INDEX]}"
  SIZE_KB="$(directory_kb "$TARGET")"
  STATUS="$(eligible_status "$INDEX")"
  CACHED_SIZES[INDEX]="$SIZE_KB"
  CACHED_STATUSES[INDEX]="$STATUS"

  TOTAL_KB=$((TOTAL_KB + SIZE_KB))
  if ((SIZE_KB > 0)) || [[ "$STATUS" == SKIP* ]]; then
    DISPLAY_COUNT=$((DISPLAY_COUNT + 1))
  fi
  case "$STATUS" in
    "SKIP app/process running")
      IN_USE_COUNT=$((IN_USE_COUNT + 1))
      ;;
    "SKIP symlink"|"SKIP not a directory")
      UNSAFE_COUNT=$((UNSAFE_COUNT + 1))
      ;;
  esac
  if ((SIZE_KB > 0)); then
    KNOWN_COUNT=$((KNOWN_COUNT + 1))
    case "$STATUS" in
      ready)
        READY_COUNT=$((READY_COUNT + 1))
        if [[ "${CACHE_TIERS[$INDEX]}" == "routine" ]]; then
          ROUTINE_READY_KB=$((ROUTINE_READY_KB + SIZE_KB))
        else
          REINSTALL_READY_KB=$((REINSTALL_READY_KB + SIZE_KB))
        fi
        ;;
      opt-in)
        OPT_IN_COUNT=$((OPT_IN_COUNT + 1))
        OPT_IN_KB=$((OPT_IN_KB + SIZE_KB))
        ;;
    esac
  fi
  INDEX=$((INDEX + 1))
done

ui_progress_done "Checked ${#CACHE_PATHS[@]} locations."

ui_section "2. Review"
if ((DISPLAY_COUNT == 0)) && ((VERBOSE == 0)); then
  printf '  No data was found in the known cache locations.\n'
else
  printf '  %-10s %-25s %10s\n' "STATUS" "CACHE" "SIZE"
  INDEX=0
  while ((INDEX < ${#CACHE_PATHS[@]})); do
    SIZE_KB="${CACHED_SIZES[$INDEX]}"
    if ((SIZE_KB > 0)) || ((VERBOSE)) || [[ "${CACHED_STATUSES[$INDEX]}" == SKIP* ]]; then
      ui_status_row \
        "${CACHED_STATUSES[$INDEX]}" \
        "${CACHE_LABELS[$INDEX]}" \
        "$(format_kb "$SIZE_KB")" \
        "${CACHE_NOTES[$INDEX]}"
      if ((VERBOSE)); then
        printf '  %s%-10s%s %s\n' "$C_DIM" "" "$C_RESET" "${CACHE_PATHS[$INDEX]}"
      fi
    fi
    INDEX=$((INDEX + 1))
  done
fi

printf '\n  %-22s %s in %d cache(s)\n' "Found" "$(format_kb "$TOTAL_KB")" "$KNOWN_COUNT"
printf '  %-22s %s in %d cache(s)\n' \
  "Ready to clean" "$(format_kb "$((ROUTINE_READY_KB + REINSTALL_READY_KB))")" "$READY_COUNT"
if ((OPT_IN_COUNT > 0)); then
  printf '  %-22s %s in %d cache(s)\n' \
    "Needs explicit opt-in" "$(format_kb "$OPT_IN_KB")" "$OPT_IN_COUNT"
fi
if ((IN_USE_COUNT > 0)); then
  printf '  %-22s %d cache(s); close the related apps and scan again\n' \
    "Currently in use" "$IN_USE_COUNT"
fi
if ((UNSAFE_COUNT > 0)); then
  printf '  %-22s %d path(s); unexpected type or symlink\n' \
    "Refused for safety" "$UNSAFE_COUNT"
fi

if ((INCLUDE_REINSTALLABLE == 0)) && ((OPT_IN_COUNT > 0)); then
  printf '\n  %sOptional items contain tools or runtimes that must be downloaded again.%s\n' \
    "$C_DIM" "$C_RESET"
  printf '  Add --include-reinstallable if you also want to review those items.\n'
fi

if [[ "$MODE" == "analyze" ]]; then
  ui_section "Result"
  printf '  No files were changed. To review ready items interactively, run:\n'
  printf '  %s./%s --clean%s\n' "$C_CYAN" "$SCRIPT_NAME" "$C_RESET"
  exit 0
fi

ui_section "3. Clean"
if ((READY_COUNT == 0)); then
  printf '  Nothing is currently eligible for cleanup. No files were changed.\n'
  exit 0
fi

if ((ASSUME_YES == 0)) && [[ ! -t 0 ]]; then
  printf '  Interactive confirmation needs a terminal. No files were changed.\n'
  printf '  Run this command in a terminal, or add --yes for unattended cleanup.\n'
  exit 0
fi

printf '  Only the %d item(s) marked READY will be offered. Deletion is permanent.\n' "$READY_COUNT"
if ((ASSUME_YES)); then
  printf '  --yes is active; every ready item will be cleared automatically.\n'
else
  printf '  At each prompt, choose yes, no, all remaining, or quit.\n'
fi

FREE_BEFORE_KB="$(free_kb)"
FREE_BEFORE_KB="${FREE_BEFORE_KB:-0}"
FREED_KB=0
FAILURES=0
CLEARED_COUNT=0
USER_SKIPPED_COUNT=0
SAFETY_SKIPPED_COUNT=0
CLEAN_POSITION=0
QUIT_EARLY=0
INDEX=0

while ((INDEX < ${#CACHE_PATHS[@]})); do
  TARGET="${CACHE_PATHS[$INDEX]}"

  # Cleanup is limited to candidates shown as ready in the review above.
  if [[ "${CACHED_STATUSES[$INDEX]}" != "ready" ]] || ((CACHED_SIZES[INDEX] == 0)); then
    INDEX=$((INDEX + 1))
    continue
  fi

  CLEAN_POSITION=$((CLEAN_POSITION + 1))
  STATUS="$(eligible_status "$INDEX")"
  if [[ "$STATUS" != "ready" ]]; then
    SAFETY_SKIPPED_COUNT=$((SAFETY_SKIPPED_COUNT + 1))
    ui_result "skip" "${CACHE_LABELS[$INDEX]} — status changed to: $STATUS"
    INDEX=$((INDEX + 1))
    continue
  fi

  SIZE_BEFORE_KB="$(directory_kb "$TARGET")"
  if ((SIZE_BEFORE_KB == 0)); then
    SAFETY_SKIPPED_COUNT=$((SAFETY_SKIPPED_COUNT + 1))
    ui_result "skip" "${CACHE_LABELS[$INDEX]} — already empty"
    INDEX=$((INDEX + 1))
    continue
  fi

  if confirm_delete \
    "${CACHE_LABELS[$INDEX]}" \
    "$(format_kb "$SIZE_BEFORE_KB")" \
    "${CACHE_NOTES[$INDEX]}" \
    "$CLEAN_POSITION" \
    "$READY_COUNT"; then
    DECISION=0
  else
    DECISION=$?
  fi

  if ((DECISION == 2)); then
    QUIT_EARLY=1
    ui_result "skip" "Cleanup stopped; remaining items were left unchanged."
    break
  elif ((DECISION != 0)); then
    USER_SKIPPED_COUNT=$((USER_SKIPPED_COUNT + 1))
    ui_result "skip" "${CACHE_LABELS[$INDEX]} — left unchanged"
    INDEX=$((INDEX + 1))
    continue
  fi

  # Final process check narrows the race window immediately before deletion.
  if process_is_running "${PROCESS_PATTERNS[$INDEX]}"; then
    SAFETY_SKIPPED_COUNT=$((SAFETY_SKIPPED_COUNT + 1))
    ui_result "skip" "${CACHE_LABELS[$INDEX]} — a related process just started"
    INDEX=$((INDEX + 1))
    continue
  fi

  ui_progress "$CLEAN_POSITION" "$READY_COUNT" "Clearing" "${CACHE_LABELS[$INDEX]}"
  if ((TUI_ENABLED)); then
    printf '\n'
  fi
  if clear_directory_contents "$TARGET"; then
    SIZE_AFTER_KB="$(directory_kb "$TARGET")"
    ITEM_FREED_KB=$((SIZE_BEFORE_KB - SIZE_AFTER_KB))
    ((ITEM_FREED_KB < 0)) && ITEM_FREED_KB=0
    FREED_KB=$((FREED_KB + ITEM_FREED_KB))
    CLEARED_COUNT=$((CLEARED_COUNT + 1))
    ui_result "done" "${CACHE_LABELS[$INDEX]} — reclaimed about $(format_kb "$ITEM_FREED_KB")"
  else
    FAILURES=$((FAILURES + 1))
    ui_result "error" "${CACHE_LABELS[$INDEX]} could not be fully cleared"
  fi

  INDEX=$((INDEX + 1))
done

FREE_AFTER_KB="$(free_kb)"
FREE_AFTER_KB="${FREE_AFTER_KB:-0}"
DISK_DELTA_KB=$((FREE_AFTER_KB - FREE_BEFORE_KB))
((DISK_DELTA_KB < 0)) && DISK_DELTA_KB=0

ui_section "Summary"
if ((QUIT_EARLY)); then
  printf '  Cleanup stopped early. Reviewed %d of %d ready item(s).\n' "$CLEAN_POSITION" "$READY_COUNT"
else
  printf '  Cleanup finished. Reviewed all %d ready item(s).\n' "$READY_COUNT"
fi
printf '\n  %-24s %s\n' "Cache data removed" "about $(format_kb "$FREED_KB")"
printf '  %-24s %s\n' "Filesystem space change" "about $(format_kb "$DISK_DELTA_KB")"
printf '  %-24s %d\n' "Cleared" "$CLEARED_COUNT"
printf '  %-24s %d\n' "Skipped by you" "$USER_SKIPPED_COUNT"
printf '  %-24s %d\n' "Skipped for safety" "$SAFETY_SKIPPED_COUNT"
printf '  %-24s %d\n' "Failed" "$FAILURES"

if ((FAILURES > 0)); then
  printf '\n  %d item(s) could not be fully cleaned. Review the messages above.\n' "$FAILURES" >&2
  exit 1
fi
