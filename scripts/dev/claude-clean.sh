#!/usr/bin/env bash
# claude-clean.sh — remove every trace of Claude Code CLI and/or Claude Desktop
# from this Mac, so a characterization run starts from a genuinely fresh machine.
#
# Usage: scripts/dev/claude-clean.sh [--cli] [--desktop] [--ide] [--apply] [--no-backup] [--kill]
#   --cli        target Claude Code, for each way Anthropic documents installing it:
#                - the native installer (claude.ai/install.sh): ~/.local/bin/claude and
#                  ~/.local/share/claude/versions
#                - Homebrew: brew install --cask claude-code (or claude-code@latest)
#                - npm -g @anthropic-ai/claude-code (and its per-platform package),
#                  under plain npm, Volta, nvm, fnm, asdf or mise, plus bun/pnpm/yarn globals
#                - the legacy local install in ~/.claude/local and the old
#                  `alias claude=` line it added to shell profiles
#                - any other `claude` on PATH that reports itself as Claude Code
#                and also ~/.claude (or $CLAUDE_CONFIG_DIR), ~/.claude.json* and the
#                Keychain login, which the CLI, the IDE plugins and Claude Desktop share
#   --desktop    target Claude Desktop (bundle id com.anthropic.claudefordesktop):
#                download, Homebrew cask claude or the enterprise PKG
#   --ide        target the IDE integrations: the anthropic.claude-code extension in
#                VS Code, VS Code Insiders, Cursor, Windsurf and Kiro (it bundles its
#                own claude), and the Claude Code plugin in JetBrains IDEs
#                (no target flag = all three)
#   --apply      actually do it (default is a dry run that only lists targets)
#   --no-backup  delete instead of moving files into ~/claude-clean-backup-<ts>/
#   --kill       quit running Claude processes instead of refusing to continue
#
# Notes
# - Keychain items cannot be backed up by this script (that would read secrets);
#   they are deleted. You will need to sign in again afterwards.
# - Files outside your home folder (/Applications, Homebrew) are deleted, not
#   backed up. Managed settings in /Library/Application Support/ClaudeCode are
#   left alone and listed as leftovers.
# - ~/.claude/projects holds your CLI transcripts. The backup keeps them; restore
#   with: mv ~/claude-clean-backup-<ts>/HOME/.claude ~/.claude
# - A PATH line for ~/.local/bin is left alone, because other tools use it too.
# - macOS only. Windows (install.ps1, WinGet) and Linux packages are not covered.
# - antiburn's own index still remembers sessions it already scanned. Reset it
#   separately if a test needs antiburn to have never seen Claude.
set -euo pipefail

want_cli=0 want_desktop=0 want_ide=0 apply=0 backup=1 kill=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --cli) want_cli=1 ;;
    --desktop) want_desktop=1 ;;
    --ide) want_ide=1 ;;
    --apply) apply=1 ;;
    --no-backup) backup=0 ;;
    --kill) kill=1 ;;
    -h|--help) sed -n '2,38p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
  shift
done
[[ $want_cli -eq 0 && $want_desktop -eq 0 && $want_ide -eq 0 ]] && want_cli=1 want_desktop=1 want_ide=1
[[ "$(uname)" == "Darwin" ]] || { echo "macOS only" >&2; exit 1; }

ts=$(date +%Y%m%d-%H%M%S)
backup_root="$HOME/claude-clean-backup-$ts"
AS="$HOME/Library/Application Support"
BUNDLE_ID="com.anthropic.claudefordesktop"
say() { printf '%s\n' "$*"; }
run() { if [[ $apply -eq 1 ]]; then "$@"; else say "  would run: $*"; fi; }

# Paths already listed or removed in this run, one per line.
seen=$'\n'
remove_path() {
  local p="$1"
  [[ -e "$p" || -L "$p" ]] || return 0
  [[ "$seen" == *$'\n'"$p"$'\n'* ]] && return 0
  seen+="$p"$'\n'
  if [[ $apply -eq 0 ]]; then say "  would remove: $p"; return 0; fi
  if [[ $backup -eq 1 && "$p" == "$HOME"/* ]]; then
    local dest="$backup_root/HOME/${p#"$HOME"/}"
    mkdir -p "$(dirname "$dest")"
    mv "$p" "$dest" && say "  moved: $p -> $dest"
  else
    if rm -rf "$p" 2>/dev/null; then say "  removed: $p"
    else say "  WARNING: could not remove (try with sudo): $p"; fi
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

# A global npm-style package: remove it and the bin link that points into it.
# Removing files works the same for npm, Volta, nvm, fnm, bun, pnpm and yarn,
# where each tool's own uninstall command does not always reach a package that
# another tool installed.
remove_node_package() {
  local pkg="$1" bin_dir
  [[ -d "$pkg" ]] || return 0
  for bin_dir in "$(dirname "$(dirname "$(dirname "$pkg")")")/bin" \
                 "$(dirname "$(dirname "$(dirname "$(dirname "$pkg")")")")/bin"; do
    [[ -L "$bin_dir/claude" && "$(readlink "$bin_dir/claude")" == *@anthropic-ai/claude-code* ]] \
      && remove_path "$bin_dir/claude"
  done
  remove_path "$pkg"
}

# True when the file is a Claude Code binary or launcher.
is_claude_code() {
  local f="$1" target
  [[ -e "$f" ]] || return 1
  target=$(readlink "$f" 2>/dev/null || true)
  [[ "$target" == *@anthropic-ai/claude-code* || "$target" == *share/claude/versions* \
     || "$target" == *.claude/local* ]] && return 0
  "$f" --version 2>/dev/null | head -n 1 | grep -q 'Claude Code'
}

# Remove the `alias claude=…/.claude/local/claude` line older installers added.
remove_legacy_alias() {
  local profile="$1"
  [[ -f "$profile" ]] && grep -qE '^[[:space:]]*alias claude=.*\.claude/local' "$profile" || return 0
  if [[ $apply -eq 0 ]]; then say "  would remove the legacy claude alias from: $profile"; return 0; fi
  if [[ $backup -eq 1 && "$profile" == "$HOME"/* ]]; then
    mkdir -p "$backup_root/HOME/$(dirname "${profile#"$HOME"/}")"
    cp -p "$profile" "$backup_root/HOME/${profile#"$HOME"/}"
  fi
  grep -vE '^[[:space:]]*alias claude=.*\.claude/local' "$profile" >"$profile.claude-clean.$$" || true
  mv "$profile.claude-clean.$$" "$profile"
  say "  removed the legacy claude alias from: $profile"
}

# Claude Desktop bundles, found by bundle id so a renamed copy is caught too.
desktop_apps() {
  {
    mdfind "kMDItemCFBundleIdentifier == '$BUNDLE_ID'" 2>/dev/null || true
    # Spotlight can miss an app, so also check every app in the Applications folders.
    for p in /Applications/*.app "$HOME"/Applications/*.app; do [[ -d "$p" ]] && echo "$p"; done
  } | sort -u | while IFS= read -r app; do
    [[ "$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Contents/Info.plist" 2>/dev/null)" == "$BUNDLE_ID" ]] \
      && echo "$app"
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

  # npm -g @anthropic-ai/claude-code and its per-platform native package.
  npm_root=""
  command -v npm >/dev/null 2>&1 && npm_root="$(npm root -g 2>/dev/null || true)"
  for scope in \
    "$HOME"/.volta/tools/image/node/*/lib/node_modules/@anthropic-ai \
    "$HOME"/.volta/tools/image/packages/@anthropic-ai \
    "$HOME"/.nvm/versions/node/*/lib/node_modules/@anthropic-ai \
    "$HOME"/.local/share/fnm/node-versions/*/installation/lib/node_modules/@anthropic-ai \
    "$HOME"/Library/Application\ Support/fnm/node-versions/*/installation/lib/node_modules/@anthropic-ai \
    "$HOME"/.asdf/installs/nodejs/*/lib/node_modules/@anthropic-ai \
    "$HOME"/.local/share/mise/installs/node/*/lib/node_modules/@anthropic-ai \
    "$HOME"/.npm-global/lib/node_modules/@anthropic-ai \
    /opt/homebrew/lib/node_modules/@anthropic-ai \
    /usr/local/lib/node_modules/@anthropic-ai \
    "$HOME"/.bun/install/global/node_modules/@anthropic-ai \
    "$HOME"/Library/pnpm/global/*/node_modules/@anthropic-ai \
    "$HOME"/.config/yarn/global/node_modules/@anthropic-ai \
    ${npm_root:+"$npm_root/@anthropic-ai"}; do
    for pkg in "$scope"/claude-code "$scope"/claude-code-*; do remove_node_package "$pkg"; done
  done
  for shim in "$HOME/.volta/bin/claude" "$HOME/.bun/bin/claude" "$HOME/Library/pnpm/claude" \
              "$HOME/.yarn/bin/claude"; do
    [[ -e "$shim" || -L "$shim" ]] && remove_path "$shim"
  done

  # Native installer, legacy local install, and their profile lines.
  for p in "$HOME/.local/bin/claude" "$HOME/.local/share/claude" "$HOME/.local/state/claude" \
           "$HOME/.cache/claude" "$HOME/.config/claude" "$HOME/Library/Caches/claude-cli-nodejs"; do
    remove_path "$p"
  done
  for profile in "$HOME/.zshrc" "${ZDOTDIR:+$ZDOTDIR/.zshrc}" "$HOME/.zprofile" "$HOME/.bashrc" \
                 "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile" "$HOME/.config/fish/config.fish"; do
    [[ -n "$profile" ]] && remove_legacy_alias "$profile"
  done

  # Anything else still answering as Claude Code on PATH. Desktop- and
  # IDE-bundled copies are handled by their own sections.
  { zsh -lic 'which -a claude' 2>/dev/null || true
    ls -d /usr/local/bin/claude /opt/homebrew/bin/claude "$HOME/bin/claude" 2>/dev/null || true
  } | grep -v -E 'not found|aliased to|\.app/|/extensions/' | sort -u >"${TMPDIR:-/tmp}/claude-clean-path.$$" || true
  while IFS= read -r f; do
    if is_claude_code "$f"; then remove_path "$f"; fi
  done <"${TMPDIR:-/tmp}/claude-clean-path.$$"
  rm -f "${TMPDIR:-/tmp}/claude-clean-path.$$"

  # Shared state: settings, transcripts, ~/.claude/local, IDE lock files.
  [[ -n "${CLAUDE_CONFIG_DIR:-}" ]] && remove_path "$CLAUDE_CONFIG_DIR"
  remove_path "$HOME/.claude"
  for p in "$HOME"/.claude.json "$HOME"/.claude.json.backup*; do remove_path "$p"; done
  # Plain item, CLAUDE_CONFIG_DIR-hashed items, and Desktop-runtime hashed items.
  remove_keychain_services '^Claude Code-credentials(-[0-9a-f]+)?$'
  say "  note: project-level .claude/ and .mcp.json files in your repos are left alone."
fi

# ---- Claude Desktop --------------------------------------------------------
if [[ $want_desktop -eq 1 ]]; then
  say ""; say "== Claude Desktop"
  if command -v brew >/dev/null 2>&1 && brew list --cask claude >/dev/null 2>&1; then
    run brew uninstall --cask --zap claude
  fi
  while IFS= read -r app; do remove_path "$app"; done < <(desktop_apps)
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
  say "  note: Claude Desktop also uses ~/.claude and the Keychain login; --cli removes those."
  say "  note: ~/Documents/Claude (if any) may hold your own files; not removed."
  say "  note: remove Claude from System Settings → General → Login Items if listed."
fi

# ---- IDE integrations -----------------------------------------------------
if [[ $want_ide -eq 1 ]]; then
  say ""; say "== IDE integrations"
  for editor in "code:$HOME/.vscode" "code-insiders:$HOME/.vscode-insiders" "cursor:$HOME/.cursor" \
                "windsurf:$HOME/.windsurf" "kiro:$HOME/.kiro"; do
    cli="${editor%%:*}" root="${editor#*:}" found=0
    for ext in "$root"/extensions/anthropic.claude-code-*; do [[ -d "$ext" ]] && found=1; done
    [[ $found -eq 1 ]] || continue
    # The editor's CLI also clears its extension registry.
    if command -v "$cli" >/dev/null 2>&1; then
      run "$cli" --uninstall-extension anthropic.claude-code || true
    fi
    for ext in "$root"/extensions/anthropic.claude-code-*; do [[ -d "$ext" ]] && remove_path "$ext"; done
  done
  # JetBrains IDEs and Android Studio keep plugins per IDE version.
  for plugin in "$AS"/JetBrains/*/plugins/*[Cc]laude* "$AS"/Google/AndroidStudio*/plugins/*[Cc]laude*; do
    [[ -e "$plugin" ]] && remove_path "$plugin"
  done
  say "  note: restart open editors so they unload the extension."
fi

# ---- leftovers ------------------------------------------------------------
say ""; say "== Leftovers matching claude/anthropic (review manually)"
{
  find "$HOME/Library" -maxdepth 3 \( -iname '*anthropic*' -o -iname 'claude*' \) 2>/dev/null \
    | grep -v -E 'ClaudeOAuthProviderTests' || true
  ls -d "$HOME"/.claude* "$HOME"/.local/*/claude \
    "$HOME"/.{vscode,vscode-insiders,cursor,windsurf,kiro}/extensions/anthropic.claude-code-* \
    "$AS"/JetBrains/*/plugins/*[Cc]laude* 2>/dev/null || true
  ls -d "/Library/Application Support/ClaudeCode" 2>/dev/null | sed 's/^/managed settings (left alone): /' || true
  zsh -lic 'which -a claude' 2>/dev/null | grep -v 'not found' | sed 's/^/on PATH: /' || true
  security dump-keychain 2>/dev/null | sed -n 's/.*"svce"<blob>="\(.*\)"/keychain: \1/p' \
    | grep -iE 'claude|anthropic' | sort -u || true
} | sed 's/^/  /'
say ""; say "Done$([[ $apply -eq 0 ]] && echo ' (dry run)')."
