#!/usr/bin/env bash
# claude-probe.sh — metadata-only inventory of Claude Code CLI / Claude Desktop
# footprints on macOS. Never prints secrets or transcript content.
#
# Usage: scripts/dev/claude-probe.sh [--label NAME] [--out DIR] [--keychain-shape]
#        scripts/dev/claude-probe.sh --shape-from-file PATH
#   --label           tag for this snapshot (e.g. "03-desktop-signed-in")
#   --out             directory to write the report to (default: .agent-artifacts/claude-probe)
#   --keychain-shape  also read the secret of each "Claude Code-credentials*" Keychain
#                     item and report its shape. This read can show a Keychain prompt.
#   --shape-from-file print the login shape of a JSON file to stdout and stop.
#                     Use it on synthetic data. It works on any OS.
#
# Run it after each characterization step and diff successive reports.
#
# Login shape: key names, value types, and empty/blank/zero/non-empty state only.
# It shows if expiresAt is in the past or the future, in minutes. It never shows
# a token, a refresh token, an account ID or a scope (scopes show as a count).
# The secret goes from `security -w` straight into jq. It is never kept.
#
# Four-point login run (issue #704): run with --keychain-shape at each point:
#   1. signed in            --label 01-signed-in
#   2. after normal expiry  --label 02-expired      (about 8 hours after sign-in)
#   3. after `claude /logout` --label 03-logout
#   4. after `claude /login`  --label 04-login
# At each point, write down what `claude /status` says for the login, and if a
# read showed a Keychain prompt. Then diff the reports. They show if /logout
# deletes the item, removes claudeAiOauth, or makes it blank, and if a new item
# shows a prompt for the secret read.
set -euo pipefail

# Print the login shape of the JSON on stdin as Markdown list lines.
# Never print a value. Print names, types, states, counts and expiry only.
# Set CLAUDE_PROBE_NOW_MS to fix the clock (tests only).
login_shape() {
  local now_ms="${CLAUDE_PROBE_NOW_MS:-}"
  [[ -n "$now_ms" ]] || now_ms=$(( $(date +%s) * 1000 ))
  jq -Rrs --argjson nowms "$now_ms" '
    def state:
      if type == "string" then
        (if . == "" then "empty" elif test("^\\s*$") then "blank" else "non-empty" end)
      elif type == "number" then
        (if . == 0 then "zero" elif . < 0 then "negative" else "non-zero" end)
      elif type == "array" then "\(length) items"
      elif type == "object" then "\(length) keys"
      elif type == "boolean" then "set"
      else "null" end;
    def expiry:
      if type != "number" then "not a number"
      elif . <= 0 then "not set (zero or negative)"
      else ((. - $nowms) / 60000 | floor) as $m
        | if $m >= 0 then "future, in about \($m) min"
          else "past, about \(-$m) min ago" end
      end;
    . as $raw
    | if ($raw | test("^\\s*$")) then "- data: empty"
      else (try [$raw | fromjson] catch null) as $w
      | if $w == null then
          (if ($raw | test("^[0-9a-fA-F\\s]+$")) then "- data: not valid JSON (looks hex-encoded)"
           else "- data: not valid JSON" end)
        else $w[0] as $d
        | if ($d | type) != "object" then "- data: JSON \($d | type), not an object"
          else
            "- top-level keys: \($d | keys | join(", "))",
            ($d | to_entries[] | "  - \(.key): \(.value | type), \(.value | state)"),
            (if ($d | has("claudeAiOauth")) | not then "- claudeAiOauth: absent"
             elif ($d.claudeAiOauth | type) != "object" then "- claudeAiOauth: \($d.claudeAiOauth | type), not an object"
             else
               "- claudeAiOauth keys:",
               ($d.claudeAiOauth | to_entries[] | "  - \(.key): \(.value | type), \(.value | state)"),
               (if ($d.claudeAiOauth | has("expiresAt")) then "- claudeAiOauth.expiresAt: \($d.claudeAiOauth.expiresAt | expiry)"
                else "- claudeAiOauth.expiresAt: absent" end)
             end)
          end
        end
      end
  ' 2>/dev/null || echo "- data: could not parse"
}

# Print the modification time of a file.
file_mtime() { stat -f '%Sm' -t '%Y-%m-%dT%H:%M:%S%z' "$1" 2>/dev/null || echo '?'; }

label="snapshot"
out_dir=".agent-artifacts/claude-probe"
keychain_shape=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --label) label="$2"; shift 2 ;;
    --out) out_dir="$2"; shift 2 ;;
    --keychain-shape) keychain_shape=1; shift ;;
    --shape-from-file)
      [[ -f "${2:-}" ]] || { echo "file not found" >&2; exit 2; }
      login_shape < "$2"; exit 0 ;;
    -h|--help) sed -n '2,32p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

[[ "$(uname)" == "Darwin" ]] || { echo "macOS only" >&2; exit 1; }

mkdir -p "$out_dir"
report="$out_dir/$(date +%Y%m%d-%H%M%S)-$label.md"
AS="$HOME/Library/Application Support/Claude"

exists() { if [[ -e "$1" || -L "$1" ]]; then echo "present"; else echo "absent"; fi; }
section() { printf '\n## %s\n\n' "$1"; }

# Print JSON key names of a file (no values). Arrays/objects summarized.
json_keys() { jq -c 'if type=="object" then keys else type end' "$1" 2>/dev/null || echo "(not json)"; }

# For a JSONL transcript: per-line type/entrypoint/version + key set, first N lines.
jsonl_shape() {
  local f="$1" n="${2:-8}"
  head -n "$n" "$f" | jq -c '{type, entrypoint, version, userType, keys: keys}' 2>/dev/null || echo "(unparseable)"
}

{
  echo "# Claude probe: $label"
  echo
  echo "- taken: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "- macOS: $(sw_vers -productVersion) ($(uname -m))"
  echo "- CLAUDE_CONFIG_DIR: ${CLAUDE_CONFIG_DIR:-unset}"

  section "Apps and binaries"
  echo "- /Applications/Claude.app: $(exists /Applications/Claude.app)"
  if [[ -d /Applications/Claude.app ]]; then
    echo "  - version: $(defaults read /Applications/Claude.app/Contents/Info CFBundleShortVersionString 2>/dev/null || echo '?')"
    echo "  - bundle id: $(defaults read /Applications/Claude.app/Contents/Info CFBundleIdentifier 2>/dev/null || echo '?')"
  fi
  echo "- ~/Applications/Claude.app: $(exists "$HOME/Applications/Claude.app")"
  echo "- \`which -a claude\` (login shell PATH):"
  (zsh -lic 'which -a claude' 2>/dev/null || true) | sort -u | sed 's/^/  - /'
  echo "- \`claude\` on GUI default PATH (/usr/bin:/bin:/usr/sbin:/sbin): $(PATH=/usr/bin:/bin:/usr/sbin:/sbin command -v claude || echo absent)"
  echo "- ~/.local/bin/claude: $(exists "$HOME/.local/bin/claude") $( [[ -L "$HOME/.local/bin/claude" ]] && echo "-> $(readlink "$HOME/.local/bin/claude")")"
  echo "- ~/.local/share/claude/versions: $(find "$HOME/.local/share/claude/versions" -mindepth 1 -maxdepth 1 -exec basename {} \; 2>/dev/null | tr '\n' ' ')"
  echo "- Desktop-bundled Claude Code (\$AS/claude-code/*): $(find "$AS/claude-code" -mindepth 1 -maxdepth 1 -exec basename {} \; 2>/dev/null | tr '\n' ' ')"
  echo "- brew casks: $(brew list --cask 2>/dev/null | grep -Ei '^claude' | tr '\n' ' ')"
  echo "- npm -g @anthropic-ai/claude-code: $(npm ls -g --depth=0 2>/dev/null | grep -o '@anthropic-ai/claude-code@[^ ]*' || echo absent)"
  echo "- running processes: $(pgrep -fl -i 'claude' 2>/dev/null | grep -v claude-probe | awk '{print $2}' | sort | uniq -c | tr '\n' ';' || true)"

  section "Keychain (service names + attributes only, never the secret)"
  security dump-keychain 2>/dev/null \
    | grep -E '"svce"<blob>=' | grep -iE 'claude|anthropic' | sort | uniq -c | sed 's/^/- /' || true
  # Find hashed login items ("Claude Code-credentials-<hex>") from the attributes.
  login_items=$(security dump-keychain 2>/dev/null \
    | sed -n 's/.*"svce"<blob>="\(Claude Code-credentials-[0-9a-fA-F]*\)".*/\1/p' | sort -u || true)
  while IFS= read -r svc; do
    [[ -n "$svc" ]] || continue
    if meta=$(security find-generic-password -s "$svc" 2>/dev/null); then
      acct=$(sed -n 's/.*"acct"<blob>="\(.*\)"/\1/p' <<<"$meta" | head -1)
      mdat=$(sed -n 's/.*"mdat"<timedate>=.*"\(.*\)\\000"/\1/p' <<<"$meta" | head -1)
      echo "- \`$svc\`: present (acct set: $([[ -n "$acct" ]] && echo yes || echo no), mdat: ${mdat:-?})"
      if [[ "$svc" == "Claude Code-credentials"* ]]; then
        if [[ $keychain_shape -eq 1 ]]; then
          echo "  - login shape (secret read, can show a Keychain prompt):"
          # Pipe the secret straight into jq. Do not keep it.
          { security find-generic-password -s "$svc" -w 2>/dev/null || true; } | login_shape | sed 's/^/    /'
        else
          echo "  - login shape: skipped (use --keychain-shape)"
        fi
      fi
    else
      echo "- \`$svc\`: absent"
    fi
  done <<<"$(printf '%s\n' "Claude Code-credentials" "$login_items" "Claude Safe Storage")"

  section "CLI config/credential files"
  for p in "$HOME/.claude" "$HOME/.claude.json" "$HOME/.claude/.credentials.json" \
           "$HOME/.config/claude" "$HOME/.local/state/claude" "$HOME/.cache/claude" \
           "$HOME/Library/Caches/claude-cli-nodejs" "$HOME/.claude/local"; do
    echo "- ${p/#$HOME/~}: $(exists "$p")"
  done
  if [[ -f "$HOME/.claude.json" ]]; then
    echo "- ~/.claude.json top-level keys: $(json_keys "$HOME/.claude.json")"
    echo "- ~/.claude.json has oauthAccount: $(jq 'has("oauthAccount")' "$HOME/.claude.json" 2>/dev/null)"
    echo "- ~/.claude.json has cachedUsageUtilization: $(jq 'has("cachedUsageUtilization")' "$HOME/.claude.json" 2>/dev/null)"
  fi
  creds="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/.credentials.json"
  if [[ -f "$creds" ]]; then
    echo "- credentials file (\$CLAUDE_CONFIG_DIR or ~/.claude)/.credentials.json: present (mtime: $(file_mtime "$creds"))"
    echo "  - login shape:"
    login_shape < "$creds" | sed 's/^/    /'
  else
    echo "- credentials file (\$CLAUDE_CONFIG_DIR or ~/.claude)/.credentials.json: absent"
  fi
  [[ -d "$HOME/.claude" ]] && echo "- ~/.claude entries: $(find "$HOME/.claude" -mindepth 1 -maxdepth 1 -exec basename {} \; | sort | tr '\n' ' ')"

  section "CLI transcripts (~/.claude/projects)"
  if [[ -d "$HOME/.claude/projects" ]]; then
    echo "- project dirs: $(find "$HOME/.claude/projects" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
    echo "- jsonl files: $(find "$HOME/.claude/projects" -name '*.jsonl' | wc -l | tr -d ' ')"
    echo "- entrypoint values:"
    (grep -rhoE '"entrypoint":"[^"]*"' "$HOME/.claude/projects" 2>/dev/null || true) | sort | uniq -c | sed 's/^/  - /'
  else
    echo "- absent"
  fi

  section "Claude Desktop Application Support"
  echo "- $AS: $(exists "$AS")"
  if [[ -d "$AS" ]]; then
    echo "- top-level entries:"
    find "$AS" -mindepth 1 -maxdepth 1 -exec basename {} \; | sort | sed 's/^/  - /'
    for sub in claude-code-sessions local-agent-mode-sessions claude-code-vm; do
      d="$AS/$sub"
      [[ -d "$d" ]] || { echo "- $sub: absent"; continue; }
      echo "- $sub: $(find "$d" -type f | wc -l | tr -d ' ') files, $(find "$d" -name '*.jsonl' | wc -l | tr -d ' ') jsonl, $(find "$d" -name '*.json' | wc -l | tr -d ' ') json"
      echo "  - nested .claude/projects roots: $(find "$d" -type d -path '*/.claude/projects' | wc -l | tr -d ' ')"
      echo "  - audit.jsonl files: $(find "$d" -name audit.jsonl | wc -l | tr -d ' ')"
      echo "  - directory shape (depth 4, names anonymized to type):"
      find "$d" -maxdepth 4 | sed "s|$d||" | sed -E 's/[0-9a-f]{8}-[0-9a-f-]{27}/<uuid>/g; s/local_<uuid>/local_<uuid>/g' | sort -u | head -40 | sed 's/^/    /'
    done
    first_manifest=$(find "$AS/claude-code-sessions" -maxdepth 4 -name '*.json' 2>/dev/null | head -1 || true)
    [[ -n "${first_manifest:-}" ]] && echo "- sample claude-code-sessions manifest keys: $(json_keys "$first_manifest")"
    first_local=$(find "$AS/local-agent-mode-sessions" -maxdepth 4 -name 'local_*.json' 2>/dev/null | head -1 || true)
    [[ -n "${first_local:-}" ]] && echo "- sample local-agent-mode manifest keys: $(json_keys "$first_local")"
    nested=$(find "$AS" -path '*/.claude/projects/*' -name '*.jsonl' 2>/dev/null | head -1 || true)
    if [[ -n "${nested:-}" ]]; then
      echo "- sample nested transcript line shapes (${nested/#$AS/\$AS}):"
      jsonl_shape "$nested" 10 | sed 's/^/  - /'
    fi
    audit=$(find "$AS" -name audit.jsonl 2>/dev/null | head -1 || true)
    [[ -n "${audit:-}" ]] && { echo "- sample audit.jsonl line keys:"; head -3 "$audit" | jq -c 'keys' 2>/dev/null | sed 's/^/  - /'; }
  fi
  echo "- ~/Library/Application Support/Claude-3p: $(exists "$HOME/Library/Application Support/Claude-3p")"

  section "Desktop-originated sessions visible in ~/.claude/projects"
  (grep -rlE '"entrypoint":"(claude-desktop|desktop)[^"]*"' "$HOME/.claude/projects" 2>/dev/null || true) | head -3 | while read -r f; do
    echo "- ${f/#$HOME/~}"
    jsonl_shape "$f" 10 | sed 's/^/  - /'
  done

  section "Other macOS footprints"
  for p in "$HOME/Library/Caches/com.anthropic.claudefordesktop" \
           "$HOME/Library/Caches/com.anthropic.claudefordesktop.ShipIt" \
           "$HOME/Library/HTTPStorages/com.anthropic.claudefordesktop" \
           "$HOME/Library/Preferences/com.anthropic.claudefordesktop.plist" \
           "$HOME/Library/Saved Application State/com.anthropic.claudefordesktop.savedState" \
           "$HOME/Library/Logs/Claude" "$HOME/Claude" "$HOME/Documents/Claude"; do
    echo "- ${p/#$HOME/~}: $(exists "$p")"
  done
  echo "- other ~/Library matches: $(find "$HOME/Library" -maxdepth 2 -iname '*anthropic*' 2>/dev/null | sed "s|$HOME|~|" | tr '\n' ' ')"
  echo "- LaunchAgents: $(find "$HOME/Library/LaunchAgents" -maxdepth 1 \( -iname '*claude*' -o -iname '*anthropic*' \) 2>/dev/null | tr '\n' ' ')"
  echo "- VS Code/Cursor extensions: $(find "$HOME/.vscode/extensions" "$HOME/.cursor/extensions" -maxdepth 1 -iname '*anthropic*' 2>/dev/null | tr '\n' ' ')"
} > "$report"

echo "wrote $report"
