#!/usr/bin/env bash
# codex-clean.sh — remove every trace of the Codex CLI and/or the Codex desktop
# app from this Mac, so a characterization run starts from a genuinely fresh machine.
#
# Usage: scripts/dev/codex-clean.sh [--cli] [--desktop] [--ide] [--apply] [--no-backup] [--kill]
#   --cli        target the Codex CLI, for each way Codex documents installing it:
#                - the standalone installer (chatgpt.com/codex/install.sh):
#                  ~/.local/bin/codex (or $CODEX_INSTALL_DIR), codex-code-mode-host,
#                  $CODEX_HOME/packages/standalone, and its PATH block in shell profiles
#                - npm -g @openai/codex, under plain npm, Volta, nvm, fnm, asdf or mise
#                - bun, pnpm and yarn global installs of @openai/codex
#                - Homebrew: brew install --cask codex
#                - a GitHub release binary copied onto PATH
#                and also ~/.codex (or $CODEX_HOME), which holds the login and transcripts
#   --desktop    target the Codex desktop app (bundle id com.openai.codex): the new
#                ChatGPT.app (Homebrew cask chatgpt) or the earlier Codex.app (cask
#                codex-app), with their computer-use helper (com.openai.sky.CUAService)
#   --ide        target the Codex IDE extension (openai.chatgpt) in VS Code, VS Code
#                Insiders, Cursor and Windsurf. It bundles its own codex binary.
#                (no target flag = all three)
#   --apply      actually do it (default is a dry run that only lists targets)
#   --no-backup  delete instead of moving files into ~/codex-clean-backup-<ts>/
#   --kill       quit running Codex processes instead of refusing to continue
#
# Notes
# - ~/.codex is shared by the CLI, the desktop app and the IDE extension. Only
#   --cli removes it.
# - Keychain items cannot be backed up by this script (that would read secrets);
#   they are deleted. You will need to sign in again afterwards.
# - Files outside your home folder (/Applications, Homebrew, system plugins) are
#   deleted, not backed up.
# - Restore transcripts with: mv ~/codex-clean-backup-<ts>/HOME/.codex ~/.codex
# - ChatGPT Classic (com.openai.chat, chat only), the ChatGPT Atlas browser and
#   CodexBar are left alone.
# - macOS only. Windows (install.ps1, the Store app) and Linux are not covered.
# - antiburn's own index still remembers sessions it already scanned. Reset it
#   separately if a test needs antiburn to have never seen Codex.
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
backup_root="$HOME/codex-clean-backup-$ts"
AS="$HOME/Library/Application Support"
BUNDLE_ID="com.openai.codex"
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
# A global @openai/codex package: remove the package and the bin link that
# points into it. Removing files works the same for npm, Volta, nvm, fnm, bun,
# pnpm and yarn, where each tool's own uninstall command does not always reach
# a package that another tool installed.
remove_node_package() {
  local pkg="$1" bin_dir
  [[ -d "$pkg" ]] || return 0
  for bin_dir in "$(dirname "$(dirname "$(dirname "$pkg")")")/bin" \
                 "$(dirname "$(dirname "$(dirname "$(dirname "$pkg")")")")/bin"; do
    [[ -L "$bin_dir/codex" && "$(readlink "$bin_dir/codex")" == *@openai/codex* ]] && remove_path "$bin_dir/codex"
  done
  remove_path "$pkg"
}

# True when the file is a Codex CLI binary or launcher.
is_codex_cli() {
  local f="$1" target
  [[ -e "$f" ]] || return 1
  target=$(readlink "$f" 2>/dev/null || true)
  [[ "$target" == *@openai/codex* || "$target" == *codex/packages/standalone* ]] && return 0
  "$f" --version 2>/dev/null | head -n 1 | grep -qE '^codex(-cli)? '
}

# Remove the "# >>> Codex installer >>>" PATH block the standalone installer adds.
remove_installer_path_block() {
  local profile="$1"
  [[ -f "$profile" ]] && grep -qF '# >>> Codex installer >>>' "$profile" || return 0
  if [[ $apply -eq 0 ]]; then say "  would remove the Codex installer PATH block from: $profile"; return 0; fi
  if [[ $backup -eq 1 ]]; then
    mkdir -p "$backup_root/HOME/$(dirname "${profile#"$HOME"/}")"
    cp -p "$profile" "$backup_root/HOME/${profile#"$HOME"/}"
  fi
  awk '/^# >>> Codex installer >>>$/{skip=1; next} skip && /^# <<< Codex installer <<<$/{skip=0; next} !skip' \
    "$profile" >"$profile.codex-clean.$$" && mv "$profile.codex-clean.$$" "$profile"
  say "  removed the Codex installer PATH block from: $profile"
}

if [[ $want_cli -eq 1 ]]; then
  say ""; say "== Codex CLI"
  # Homebrew (brew install --cask codex). There is no codex formula.
  if command -v brew >/dev/null 2>&1 && brew list --cask codex >/dev/null 2>&1; then
    run brew uninstall --cask --zap codex
  fi

  # Global JavaScript package installs.
  for pkg in \
    "$HOME"/.volta/tools/image/node/*/lib/node_modules/@openai/codex \
    "$HOME"/.volta/tools/image/packages/@openai/codex \
    "$HOME"/.nvm/versions/node/*/lib/node_modules/@openai/codex \
    "$HOME"/.local/share/fnm/node-versions/*/installation/lib/node_modules/@openai/codex \
    "$HOME"/Library/Application\ Support/fnm/node-versions/*/installation/lib/node_modules/@openai/codex \
    "$HOME"/.asdf/installs/nodejs/*/lib/node_modules/@openai/codex \
    "$HOME"/.local/share/mise/installs/node/*/lib/node_modules/@openai/codex \
    "$HOME"/.npm-global/lib/node_modules/@openai/codex \
    /opt/homebrew/lib/node_modules/@openai/codex \
    /usr/local/lib/node_modules/@openai/codex \
    "$HOME"/.bun/install/global/node_modules/@openai/codex \
    "$HOME"/Library/pnpm/global/*/node_modules/@openai/codex \
    "$HOME"/.config/yarn/global/node_modules/@openai/codex; do
    remove_node_package "$pkg"
  done
  if command -v npm >/dev/null 2>&1; then
    remove_node_package "$(npm root -g 2>/dev/null)/@openai/codex"
  fi
  for shim in "$HOME/.volta/bin/codex" "$HOME/.bun/bin/codex" "$HOME/Library/pnpm/codex" \
              "$HOME/.yarn/bin/codex"; do
    [[ -e "$shim" || -L "$shim" ]] && remove_path "$shim"
  done

  # Standalone installer: the command links and the PATH block. Its releases
  # live in $CODEX_HOME/packages/standalone, removed with the folder below.
  install_dir="${CODEX_INSTALL_DIR:-$HOME/.local/bin}"
  for f in "$install_dir/codex" "$install_dir/codex-code-mode-host" \
           "$HOME/.local/bin/codex" "$HOME/.local/bin/codex-code-mode-host"; do
    [[ -e "$f" || -L "$f" ]] && remove_path "$f"
  done
  for profile in "$HOME/.zprofile" "$HOME/.zshrc" "$HOME/.bash_profile" "$HOME/.bashrc" "$HOME/.profile"; do
    remove_installer_path_block "$profile"
  done

  # A GitHub release binary, or anything else still answering as Codex on PATH.
  # Desktop-app and IDE-bundled copies are handled by their own sections.
  { zsh -lic 'which -a codex' 2>/dev/null || true
    ls -d /usr/local/bin/codex /opt/homebrew/bin/codex "$HOME/bin/codex" 2>/dev/null || true
  } | grep -v -E 'not found|\.app/|/extensions/' | sort -u >"${TMPDIR:-/tmp}/codex-clean-path.$$" || true
  while IFS= read -r f; do
    if is_codex_cli "$f"; then remove_path "$f"; fi
  done <"${TMPDIR:-/tmp}/codex-clean-path.$$"
  rm -f "${TMPDIR:-/tmp}/codex-clean-path.$$"

  codex_home="${CODEX_HOME:-$HOME/.codex}"
  remove_path "$codex_home"
  [[ "$codex_home" != "$HOME/.codex" ]] && remove_path "$HOME/.codex"
  for p in "$HOME/.config/codex" "$HOME/.cache/codex"; do
    remove_path "$p"
  done
  # The CLI's keyring credential store (cli_auth_credentials_store = "keyring").
  remove_keychain_services '^Codex Auth$'
  say "  note: project-level AGENTS.md and .codex/ files in your repos are left alone."
fi

# ---- Codex desktop ---------------------------------------------------------
if [[ $want_desktop -eq 1 ]]; then
  say ""; say "== Codex desktop ($BUNDLE_ID)"
  if command -v brew >/dev/null 2>&1; then
    for cask in chatgpt codex-app; do
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
           "$HOME/Library/Logs/$BUNDLE_ID" "$HOME/Library/Logs/Codex" \
           "$HOME/Library/Caches/com.openai.sky.CUAService" \
           "$HOME/Library/HTTPStorages/com.openai.sky.CUAService" \
           "$HOME/Library/HTTPStorages/com.openai.sky.CUAService.binarycookies" \
           "$HOME/Library/Preferences/com.openai.sky.CUAService.plist" \
           "$HOME/Library/Preferences/com.openai.sky.CUAService.cli.plist" \
           "/Library/Application Support/CodexComputerUseAuthorizationPlugin"; do
    remove_path "$p"
  done
  for p in "$HOME"/Library/Group\ Containers/*."$BUNDLE_ID".* \
           "$HOME"/Library/Group\ Containers/*.com.openai.sky.CUAService \
           "$HOME"/Library/Application\ Scripts/*.com.openai.sky.CUAService \
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

# ---- Codex IDE extension -------------------------------------------------
if [[ $want_ide -eq 1 ]]; then
  say ""; say "== Codex IDE extension (openai.chatgpt)"
  for editor in "code:$HOME/.vscode" "code-insiders:$HOME/.vscode-insiders" \
                "cursor:$HOME/.cursor" "windsurf:$HOME/.windsurf"; do
    cli="${editor%%:*}" root="${editor#*:}"
    found=0
    for ext in "$root"/extensions/openai.chatgpt-*; do [[ -d "$ext" ]] && found=1; done
    [[ $found -eq 1 ]] || continue
    # The editor's CLI also clears its extension registry; remove the folders
    # when the CLI is not on PATH.
    if command -v "$cli" >/dev/null 2>&1; then
      run "$cli" --uninstall-extension openai.chatgpt || true
    fi
    for ext in "$root"/extensions/openai.chatgpt-*; do [[ -d "$ext" ]] && remove_path "$ext"; done
  done
fi

# ---- leftovers ------------------------------------------------------------
say ""; say "== Leftovers matching codex/openai (review manually)"
{
  find "$HOME/Library" -maxdepth 3 \( -iname '*codex*' -o -iname '*openai*' \) 2>/dev/null \
    | grep -v -E 'codexbar|CodexBar' || true
  ls -d "$HOME"/.codex* "$HOME"/.{vscode,vscode-insiders,cursor,windsurf}/extensions/openai.chatgpt-* 2>/dev/null || true
  grep -lF '# >>> Codex installer >>>' "$HOME"/.zprofile "$HOME"/.zshrc "$HOME"/.bash_profile "$HOME"/.bashrc "$HOME"/.profile 2>/dev/null | sed 's/^/installer PATH block: /' || true
  zsh -lic 'which -a codex' 2>/dev/null | grep -v 'not found' | sed 's/^/on PATH: /' || true
  security dump-keychain 2>/dev/null | sed -n 's/.*"svce"<blob>="\(.*\)"/keychain: \1/p' \
    | grep -iE 'codex|openai|chatgpt' | sort -u || true
} | sed 's/^/  /'
say ""; say "Done$([[ $apply -eq 0 ]] && echo ' (dry run)')."
