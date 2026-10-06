#!/usr/bin/env bash
# codex-clean.sh — remove every trace of the Codex CLI and/or the Codex desktop
# app from this Mac, so a characterization run starts from a genuinely fresh machine.
#
# Usage: scripts/dev/codex-clean.sh [--cli] [--desktop] [--apply] [--no-backup] [--kill]
#   --cli        target Codex CLI footprints, including ~/.codex
#   --desktop    target the Codex desktop app (bundle id com.openai.codex; newer
#                releases install it as ChatGPT.app and bundle their own codex CLI)
#                (neither flag = both)
#   --apply      actually do it (default is a dry run that only lists targets)
#   --no-backup  delete instead of moving files into ~/codex-clean-backup-<ts>/
#   --kill       quit running Codex processes instead of refusing to continue
#
# Notes
# - ~/.codex is shared by the CLI and the desktop app. It holds the login
#   (auth.json), config and session transcripts. Only --cli removes it.
# - Keychain items cannot be backed up by this script (that would read secrets);
#   they are deleted. You will need to sign in again afterwards.
# - Restore transcripts with: mv ~/codex-clean-backup-<ts>/HOME/.codex ~/.codex
# - The ChatGPT Atlas browser, CodexBar and other OpenAI apps are left alone.
# - antiburn's own index still remembers sessions it already scanned. Reset it
#   separately if a test needs antiburn to have never seen Codex.
set -euo pipefail

want_cli=0 want_desktop=0 apply=0 backup=1 kill=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --cli) want_cli=1 ;;
    --desktop) want_desktop=1 ;;
    --apply) apply=1 ;;
    --no-backup) backup=0 ;;
    --kill) kill=1 ;;
    -h|--help) sed -n '2,23p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
  shift
done
[[ $want_cli -eq 0 && $want_desktop -eq 0 ]] && want_cli=1 want_desktop=1
[[ "$(uname)" == "Darwin" ]] || { echo "macOS only" >&2; exit 1; }

ts=$(date +%Y%m%d-%H%M%S)
backup_root="$HOME/codex-clean-backup-$ts"
AS="$HOME/Library/Application Support"
BUNDLE_ID="com.openai.codex"
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

# The desktop app's bundles, found by bundle id so a renamed app is caught too.
desktop_apps() {
  {
    mdfind "kMDItemCFBundleIdentifier == '$BUNDLE_ID'" 2>/dev/null || true
    for p in /Applications/Codex.app "$HOME/Applications/Codex.app" \
             /Applications/ChatGPT.app "$HOME/Applications/ChatGPT.app"; do
      [[ -d "$p" ]] && echo "$p"
    done
  } | sort -u | while IFS= read -r app; do
    [[ "$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Contents/Info.plist" 2>/dev/null)" == "$BUNDLE_ID" ]] \
      && echo "$app"
  done
}

# ---- processes -------------------------------------------------------------
procs=()
if [[ $want_desktop -eq 1 ]]; then
  while IFS= read -r app; do
    pgrep -f "$app/Contents/MacOS/" >/dev/null 2>&1 && procs+=("$(basename "$app") (Codex desktop)")
  done < <(desktop_apps)
fi
[[ $want_cli -eq 1 ]] && pgrep -x codex >/dev/null 2>&1 && procs+=("codex (CLI)")
if [[ ${#procs[@]} -gt 0 ]]; then
  if [[ $kill -eq 1 ]]; then
    say "Quitting: ${procs[*]}"
    [[ $want_desktop -eq 1 ]] && run osascript -e "quit app id \"$BUNDLE_ID\"" || true
    [[ $want_cli -eq 1 ]] && run pkill -x codex || true
    [[ $apply -eq 1 ]] && sleep 2
  else
    say "Running: ${procs[*]}. Quit them first, or pass --kill." >&2
    [[ $apply -eq 1 ]] && exit 1
  fi
fi

[[ $apply -eq 0 ]] && say "DRY RUN — pass --apply to make changes."
[[ $apply -eq 1 && $backup -eq 1 ]] && say "Backing up removed files to $backup_root"

# ---- Codex CLI -------------------------------------------------------------
if [[ $want_cli -eq 1 ]]; then
  say ""; say "== Codex CLI"
  if command -v brew >/dev/null 2>&1; then
    brew list --cask codex >/dev/null 2>&1 && run brew uninstall --cask --zap codex
    brew list --formula codex >/dev/null 2>&1 && run brew uninstall --formula codex
  fi
  # Volta owns its global packages; npm cannot remove them.
  if command -v volta >/dev/null 2>&1 && volta list --format plain 2>/dev/null | grep -q '@openai/codex'; then
    run volta uninstall @openai/codex
  elif command -v npm >/dev/null 2>&1 && ! command -v volta >/dev/null 2>&1 \
       && npm ls -g --depth=0 2>/dev/null | grep -q '@openai/codex'; then
    run npm uninstall -g @openai/codex
  fi
  # npm under a Volta-managed node puts the package in the node image, where
  # neither tool removes it cleanly. Remove the package and its bin link.
  zsh -lic 'which -a codex' 2>/dev/null | grep -v 'not found' | sort -u | while IFS= read -r bin; do
    target=$(readlink "$bin" 2>/dev/null || true)
    [[ "$target" == *node_modules/@openai/codex/* ]] || continue
    remove_path "$(cd "$(dirname "$bin")" && cd "$(dirname "$target")/.." && pwd)"
    remove_path "$bin"
  done
  codex_home="${CODEX_HOME:-$HOME/.codex}"
  remove_path "$codex_home"
  [[ "$codex_home" != "$HOME/.codex" ]] && remove_path "$HOME/.codex"
  for p in "$HOME/.local/bin/codex" "$HOME/.config/codex" "$HOME/.cache/codex"; do
    remove_path "$p"
  done
  # The CLI's keyring credential store (cli_auth_credentials_store = "keyring").
  remove_keychain_services '^Codex Auth$'
  remaining=$(zsh -lic 'which -a codex' 2>/dev/null | grep -v 'not found' || true)
  [[ -n "$remaining" && $apply -eq 1 ]] && say "  WARNING: codex still on PATH: $remaining"
  say "  note: project-level AGENTS.md and .codex/ files in your repos are left alone."
fi

# ---- Codex desktop ---------------------------------------------------------
if [[ $want_desktop -eq 1 ]]; then
  say ""; say "== Codex desktop ($BUNDLE_ID)"
  if command -v brew >/dev/null 2>&1; then
    for cask in codex-app chatgpt; do
      brew list --cask "$cask" >/dev/null 2>&1 && run brew uninstall --cask --zap "$cask"
    done
  fi
  while IFS= read -r app; do remove_path "$app"; done < <(desktop_apps)
  for p in "$AS/Codex" "$AS/$BUNDLE_ID" "$AS/OpenAI/Codex" \
           "$HOME/Library/Caches/Codex" "$HOME/Library/Caches/$BUNDLE_ID" \
           "$HOME/Library/Caches/$BUNDLE_ID.ShipIt" \
           "$HOME/Library/HTTPStorages/$BUNDLE_ID" \
           "$HOME/Library/HTTPStorages/$BUNDLE_ID.binarycookies" \
           "$HOME/Library/Preferences/$BUNDLE_ID.plist" \
           "$HOME/Library/Saved Application State/$BUNDLE_ID.savedState" \
           "$HOME/Library/WebKit/$BUNDLE_ID" \
           "$HOME/Library/Logs/$BUNDLE_ID" "$HOME/Library/Logs/Codex"; do
    remove_path "$p"
  done
  for p in "$HOME"/Library/Group\ Containers/*."$BUNDLE_ID".* \
           "$HOME"/Library/Preferences/ByHost/"$BUNDLE_ID"*.plist \
           "$HOME"/Library/Application\ Support/com.apple.sharedfilelist/*/"$BUNDLE_ID".sfl*; do
    [[ -e "$p" ]] && remove_path "$p"
  done
  for base in "$(getconf DARWIN_USER_CACHE_DIR)" "$(getconf DARWIN_USER_TEMP_DIR)"; do
    for p in "$base$BUNDLE_ID"*; do [[ -e "$p" ]] && remove_path "$p"; done
  done
  for p in "$HOME"/Library/LaunchAgents/*"$BUNDLE_ID"*; do
    [[ -e "$p" ]] && { run launchctl bootout "gui/$(id -u)" "$p" || true; remove_path "$p"; }
  done
  # Electron safeStorage key (encrypts the app's cookies/session).
  remove_keychain_services '^(Codex|ChatGPT) Safe Storage$'
  [[ $apply -eq 1 ]] && run defaults delete "$BUNDLE_ID" 2>/dev/null || true
  say "  note: remove the app from System Settings → General → Login Items if listed."
fi

# ---- leftovers ------------------------------------------------------------
say ""; say "== Leftovers matching codex/openai (review manually)"
{
  find "$HOME/Library" -maxdepth 3 \( -iname '*codex*' -o -iname '*openai*' \) 2>/dev/null \
    | grep -v -E 'codexbar|CodexBar' || true
  ls -d "$HOME"/.codex* 2>/dev/null || true
  zsh -lic 'which -a codex' 2>/dev/null | grep -v 'not found' | sed 's/^/on PATH: /' || true
  security dump-keychain 2>/dev/null | sed -n 's/.*"svce"<blob>="\(.*\)"/keychain: \1/p' \
    | grep -iE 'codex|openai|chatgpt' | sort -u || true
} | sed 's/^/  /'
say ""; say "Done$([[ $apply -eq 0 ]] && echo ' (dry run)')."
