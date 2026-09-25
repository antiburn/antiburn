#!/usr/bin/env bash
# claude-clean.sh — remove every trace of Claude Code CLI and/or Claude Desktop
# from this Mac, so a characterization run starts from a genuinely fresh machine.
#
# Usage: scripts/dev/claude-clean.sh [--cli] [--desktop] [--apply] [--no-backup] [--kill]
#   --cli        target Claude Code CLI footprints
#   --desktop    target Claude Desktop footprints
#                (neither flag = both)
#   --apply      actually do it (default is a dry run that only lists targets)
#   --no-backup  delete instead of moving files into ~/claude-clean-backup-<ts>/
#   --kill       quit running Claude processes instead of refusing to continue
#
# Notes
# - Keychain items cannot be backed up by this script (that would read secrets);
#   they are deleted. You will need to sign in again afterwards.
# - ~/.claude/projects holds your CLI transcripts. The backup keeps them; restore
#   with: mv ~/claude-clean-backup-<ts>/HOME/.claude ~/.claude
# - antiburn's own index still remembers sessions it already scanned. Reset it
#   separately if a test needs antiburn to have never seen Claude.
set -euo pipefail

want_cli=0 want_desktop=0 apply=0 backup=1 kill=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --cli) want_cli=1 ;;
    --desktop) want_desktop=1 ;;
    --apply) apply=1 ;;
    --no-backup) backup=0 ;;
    --kill) kill=1 ;;
    -h|--help) sed -n '2,21p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
  shift
done
[[ $want_cli -eq 0 && $want_desktop -eq 0 ]] && want_cli=1 want_desktop=1
[[ "$(uname)" == "Darwin" ]] || { echo "macOS only" >&2; exit 1; }

ts=$(date +%Y%m%d-%H%M%S)
backup_root="$HOME/claude-clean-backup-$ts"
AS="$HOME/Library/Application Support"
say() { printf '%s\n' "$*"; }
run() { if [[ $apply -eq 1 ]]; then "$@"; else say "  would run: $*"; fi; }

remove_path() {
  local p="$1"
  [[ -e "$p" || -L "$p" ]] || return 0
  if [[ $apply -eq 0 ]]; then say "  would remove: $p"; return 0; fi
  if [[ $backup -eq 1 && "$p" == "$HOME"/* ]]; then
    local dest="$backup_root/HOME/${p#"$HOME"/}"
    mkdir -p "$(dirname "$dest")"
    mv "$p" "$dest" && say "  moved: $p -> $dest"
  else
    rm -rf "$p" && say "  removed: $p"
  fi
}

# Delete every generic-password item whose service matches an extended regex.
remove_keychain_services() {
  local pattern="$1" svc
  security dump-keychain 2>/dev/null \
    | sed -n 's/.*"svce"<blob>="\(.*\)"/\1/p' | { grep -E "$pattern" || true; } | sort -u \
    | while IFS= read -r svc; do
        if [[ $apply -eq 0 ]]; then say "  would delete keychain item(s): $svc"; continue; fi
        while security delete-generic-password -s "$svc" >/dev/null 2>&1; do
          say "  deleted keychain item: $svc"
        done
      done
}

# ---- processes -------------------------------------------------------------
procs=()
[[ $want_desktop -eq 1 ]] && pgrep -x Claude >/dev/null 2>&1 && procs+=("Claude (Desktop)")
[[ $want_cli -eq 1 ]] && pgrep -x claude >/dev/null 2>&1 && procs+=("claude (CLI)")
if [[ ${#procs[@]} -gt 0 ]]; then
  if [[ $kill -eq 1 ]]; then
    say "Quitting: ${procs[*]}"
    [[ $want_desktop -eq 1 ]] && run osascript -e 'quit app "Claude"' || true
    [[ $want_cli -eq 1 ]] && run pkill -x claude || true
    [[ $apply -eq 1 ]] && sleep 2
  else
    say "Running: ${procs[*]}. Quit them first, or pass --kill." >&2
    [[ $apply -eq 1 ]] && exit 1
  fi
fi

[[ $apply -eq 0 ]] && say "DRY RUN — pass --apply to make changes."
[[ $apply -eq 1 && $backup -eq 1 ]] && say "Backing up removed files to $backup_root"

# ---- Claude Code CLI -------------------------------------------------------
if [[ $want_cli -eq 1 ]]; then
  say ""; say "== Claude Code CLI"
  if command -v brew >/dev/null 2>&1; then
    for cask in claude-code claude-code@latest; do
      brew list --cask "$cask" >/dev/null 2>&1 && run brew uninstall --cask --zap "$cask"
    done
  fi
  if command -v npm >/dev/null 2>&1 && npm ls -g --depth=0 2>/dev/null | grep -q '@anthropic-ai/claude-code'; then
    run npm uninstall -g @anthropic-ai/claude-code
  fi
  for p in "$HOME/.local/bin/claude" "$HOME/.local/share/claude" "$HOME/.local/state/claude" \
           "$HOME/.cache/claude" "$HOME/.claude" "$HOME/.config/claude" \
           "$HOME/Library/Caches/claude-cli-nodejs" "${CLAUDE_CONFIG_DIR:-}"; do
    [[ -n "$p" ]] && remove_path "$p"
  done
  for p in "$HOME"/.claude.json "$HOME"/.claude.json.backup*; do remove_path "$p"; done
  # Plain item, CLAUDE_CONFIG_DIR-hashed items, and Desktop-runtime hashed items.
  remove_keychain_services '^Claude Code-credentials(-[0-9a-f]+)?$'
  remaining=$(zsh -lic 'which -a claude' 2>/dev/null | grep -v 'not found' || true)
  [[ -n "$remaining" && $apply -eq 1 ]] && say "  WARNING: claude still on PATH: $remaining"
  say "  note: project-level .claude/ and .mcp.json files in your repos are left alone."
fi

# ---- Claude Desktop --------------------------------------------------------
if [[ $want_desktop -eq 1 ]]; then
  say ""; say "== Claude Desktop"
  if command -v brew >/dev/null 2>&1 && brew list --cask claude >/dev/null 2>&1; then
    run brew uninstall --cask --zap claude
  fi
  remove_path "/Applications/Claude.app"
  remove_path "$HOME/Applications/Claude.app"
  for p in "$AS/Claude" "$AS/Claude-3p" \
           "$HOME/Library/Caches/com.anthropic.claudefordesktop" \
           "$HOME/Library/Caches/com.anthropic.claudefordesktop.ShipIt" \
           "$HOME/Library/HTTPStorages/com.anthropic.claudefordesktop" \
           "$HOME/Library/HTTPStorages/com.anthropic.claudefordesktop.binarycookies" \
           "$HOME/Library/Preferences/com.anthropic.claudefordesktop.plist" \
           "$HOME/Library/Saved Application State/com.anthropic.claudefordesktop.savedState" \
           "$HOME/Library/WebKit/com.anthropic.claudefordesktop" \
           "$HOME/Library/Logs/Claude" "$HOME/Claude"; do
    remove_path "$p"
  done
  for p in "$HOME"/Library/Preferences/ByHost/com.anthropic.claudefordesktop*.plist \
           "$HOME"/Library/Application\ Support/com.apple.sharedfilelist/*/com.anthropic.claudefordesktop.sfl*; do
    [[ -e "$p" ]] && remove_path "$p"
  done
  for base in "$(getconf DARWIN_USER_CACHE_DIR)" "$(getconf DARWIN_USER_TEMP_DIR)"; do
    for p in "$base"com.anthropic.claudefordesktop*; do [[ -e "$p" ]] && remove_path "$p"; done
  done
  for p in "$HOME"/Library/LaunchAgents/*anthropic* "$HOME"/Library/LaunchAgents/*claude*; do
    [[ -e "$p" ]] && { run launchctl bootout "gui/$(id -u)" "$p" || true; remove_path "$p"; }
  done
  # Electron safeStorage key (encrypts Desktop's cookies/session).
  remove_keychain_services '^Claude (Safe Storage|Key)$'
  [[ $apply -eq 1 ]] && run defaults delete com.anthropic.claudefordesktop 2>/dev/null || true
  say "  note: ~/Documents/Claude (if any) may hold your own files; not removed."
  say "  note: remove Claude from System Settings → General → Login Items if listed."
fi

# ---- leftovers ------------------------------------------------------------
say ""; say "== Leftovers matching claude/anthropic (review manually)"
{
  find "$HOME/Library" -maxdepth 3 \( -iname '*anthropic*' -o -iname 'claude*' \) 2>/dev/null \
    | grep -v -E 'ClaudeOAuthProviderTests' || true
  ls -d "$HOME"/.claude* "$HOME"/.local/*/claude 2>/dev/null || true
  security dump-keychain 2>/dev/null | sed -n 's/.*"svce"<blob>="\(.*\)"/keychain: \1/p' \
    | grep -iE 'claude|anthropic' | sort -u || true
} | sed 's/^/  /'
say ""; say "Done$([[ $apply -eq 0 ]] && echo ' (dry run)')."
