#!/bin/sh
set -eu

REPOSITORY="antiburn/antiburn"
GITHUB_URL="https://github.com/${REPOSITORY}"
TMP_DIR=""
MOUNT_POINT=""
MACOS_STAGING_ROOT=""
MACOS_BACKUP=""
MACOS_DESTINATION=""
APPIMAGE_STAGED=""
APPIMAGE_BACKUP=""
APPIMAGE_DESTINATION=""
APPIMAGE_NEW_INSTALLED="0"
APPIMAGE_SWAP_COMPLETE="0"

info() {
  printf '%s\n' "antiburn: $*"
}

# --- fire banner ---------------------------------------------------------
# The opening banner is a fire simulation. Flames rise from the outline of
# the wordmark, die down, and leave the word behind. The tuning harness for
# these values lives in the antiburn_assets repository under fire-term.
FIRE_FRAMES=39      # displayed frames
FIRE_SETTLE=24      # the last N frames let the fire die down to the wordmark
FIRE_WARM=16        # frames computed before the first one is shown
FIRE_DELAY=0.095    # seconds between frames
FIRE_DECAY=0.39     # heat lost per row; a higher value makes shorter flames
FIRE_BASE=1.15      # heat injected at the flame base on the word outline
FIRE_GUST=0.04      # chance that a cell cools hard, which breaks flat fronts
FIRE_HALO=0.0       # heat drawn beside the letters, inside the word rows
FIRE_CAP=0.76       # the hottest the flames go; the wordmark stays brighter
FIRE_GAP=2          # clear cells between the letters and the flames
FIRE_PAD=5          # cells the fire can reach from a letter before it fades

fire_supported() {
  # A pipe, a redirect, or a dumb terminal gets no fire. The install tests
  # capture stdout, so this is the path continuous integration takes.
  [ -t 1 ] || return 1
  [ "${ANTIBURN_NO_BANNER:-0}" != "1" ] || return 1
  [ -z "${NO_COLOR:-}" ] || return 1
  case "${TERM:-dumb}" in
    dumb | '') return 1 ;;
  esac
  return 0
}

fire_depth() {
  # 24 means truecolor, 8 means the 256 color cube, 0 means no color.
  case "${COLORTERM:-}" in
    truecolor | 24bit) printf '24\n'; return 0 ;;
  esac
  case "${TERM:-}" in
    *256color* | *direct*) printf '8\n'; return 0 ;;
  esac
  printf '0\n'
}

fire_width() {
  # Read the terminal, not standard output. Inside a command substitution
  # the output is a pipe, so tput would report the terminfo default.
  width=""
  if command -v stty >/dev/null 2>&1; then
    width=$(stty size </dev/tty 2>/dev/null | awk '{ print $2 }' || true)
  fi
  if [ -z "$width" ] && command -v tput >/dev/null 2>&1; then
    width=$(tput cols </dev/tty 2>/dev/null || true)
  fi
  case "$width" in
    '' | *[!0-9]*) width=80 ;;
  esac
  # A pseudo terminal with no size set reports zero columns.
  [ "$width" -gt 0 ] || width=80
  printf '%s\n' "$width"
}

fire_show_cursor() {
  # Safe to call more than once, and safe when no fire was drawn.
  [ -t 1 ] || return 0
  printf '\033[?25h'
}

fire_banner() {
  fire_supported || return 1
  fire_d=$(fire_depth)
  [ "$fire_d" != "0" ] || return 1
  fire_w=$(fire_width)
  [ "$fire_w" -ge 44 ] || return 1

  # A wide terminal gets round dots, two columns for each cell, so the dot
  # grid is square. A narrower terminal gets half blocks at half the width.
  if [ "$fire_w" -ge 90 ]; then fire_mode=dots; else fire_mode=half; fi

  # POSIX sleep only promises whole seconds. Without fractions the
  # animation would crawl, so fall back to a single still frame.
  fire_frames="$FIRE_FRAMES"
  if ! sleep 0.01 >/dev/null 2>&1; then
    fire_frames=1
  fi

  # This function sets no trap of its own. cleanup calls fire_show_cursor,
  # and the traps are already installed when the banner runs.
  printf '\033[?25l'

  awk -v depth="$fire_d" -v mode="$fire_mode" -v frames="$fire_frames" \
      -v settle="$FIRE_SETTLE" -v warm="$FIRE_WARM" -v delay="$FIRE_DELAY" \
      -v decay="$FIRE_DECAY" -v base="$FIRE_BASE" -v gust="$FIRE_GUST" \
      -v halo="$FIRE_HALO" -v cap="$FIRE_CAP" -v gap="$FIRE_GAP" \
      -v pad="$FIRE_PAD" -v seed=$$ '
BEGIN {
  W = 44; H = 20; PADL = 2; WORDTOP = 13
  ROWS = (mode == "dots") ? H : int(H / 2)
  core = ".................O......................" \
         ".............O.....O...................." \
         ".OOO..O.OO..OOOO.O.O.OO..O...O.O.O.O.OO." \
         "....O.OO..O..O...O.OO..O.O...O.OO..OO..O" \
         ".OOOO.O...O..O...O.O...O.O...O.O...O...O" \
         "O...O.O...O..O...O.O...O.O..OO.O...O...O" \
         ".OO.O.O...O...OO.O.OOOO...OO.O.O...O...O"

  # top[x] holds the topmost letter dot in each column. The flame base
  # injects heat above it, so the fire follows the letter outlines.
  ndot = 0
  for (x = 0; x < W; x++) top[x] = -1
  for (r = 0; r < 7; r++) {
    for (c = 0; c < 40; c++) {
      if (substr(core, r * 40 + c + 1, 1) == "O") {
        x = PADL + c; y = WORDTOP + r; k = y * W + x
        dot[ndot] = k
        word[k] = 1
        ndot++
        if (top[x] < 0 || y < top[x]) top[x] = y
      }
    }
  }

  # The rows that hold the wordmark are masked when they are drawn. A
  # letter is only one cell thick, so fire beside it makes the word hard
  # to read.
  for (y = WORDTOP; y < WORDTOP + 7; y++) {
    for (x = 0; x < W; x++) {
      k = y * W + x
      if (!(k in word)) ring[k] = 1
    }
  }

  # Chebyshev distance from every cell to the nearest letter dot, by
  # breadth first search. It clears a dark margin of gap cells around the
  # letters and fades the fire out beyond pad cells.
  for (i = 0; i < W * H; i++) dist[i] = 1e9
  nfr = 0
  for (i = 0; i < ndot; i++) { dist[dot[i]] = 0; fr[nfr] = dot[i]; nfr++ }
  d = 0
  while (nfr > 0) {
    d++
    nnew = 0
    for (i = 0; i < nfr; i++) {
      k = fr[i]; x = k % W; y = int(k / W)
      for (dy = -1; dy <= 1; dy++) {
        for (dx = -1; dx <= 1; dx++) {
          nx = x + dx; ny = y + dy
          if (nx < 0 || nx >= W || ny < 0 || ny >= H) continue
          nk = ny * W + nx
          if (dist[nk] > d) { dist[nk] = d; nq[nnew] = nk; nnew++ }
        }
      }
    }
    for (i = 0; i < nnew; i++) fr[i] = nq[i]
    nfr = nnew
  }

  # The ramp runs from a purple ember through the brand orange to a hot
  # tint.
  nstop = 7
  sp[0] = 0.00; sr[0] =   0; sg[0] =   0; sb[0] =   0
  sp[1] = 0.16; sr[1] =  60; sg[1] =  18; sb[1] =  70
  sp[2] = 0.34; sr[2] = 150; sg[2] =  40; sb[2] =  60
  sp[3] = 0.54; sr[3] = 232; sg[3] =  80; sb[3] =  34
  sp[4] = 0.74; sr[4] = 255; sg[4] = 106; sb[4] =  44
  sp[5] = 0.89; sr[5] = 255; sg[5] = 170; sb[5] =  90
  sp[6] = 1.00; sr[6] = 255; sg[6] = 225; sb[6] = 180
  wordcol = paint(255, 106, 44)

  srand(seed)
  for (i = 0; i < W * H; i++) heat[i] = 0

  total = warm + frames
  for (f = 0; f < total; f++) {
    shown = f - warm
    fuel = 1.0
    if (frames > 1 && settle > 0 && shown > frames - settle) {
      fuel = (frames - shown) / settle
      if (fuel < 0) fuel = 0
      # Square it so the last frames leave the wordmark on a clear field.
      fuel = fuel * fuel
    }
    step(fuel)
    if (shown < 0) continue
    if (shown > 0) printf "\033[%dA\r", ROWS
    draw()
    if (frames > 1 && shown < frames - 1) system("sleep " delay)
  }
  exit 0
}

function step(fuel,   x, y, i, s, v, b) {
  # While the fire dies down, drain what is already in the field as well.
  if (fuel < 1) for (i = 0; i < W * H; i++) heat[i] *= 0.82
  # Each row takes heat from the row below it, so the fire climbs.
  for (y = 0; y < H - 1; y++) {
    for (x = 0; x < W; x++) {
      s = x + int(rand() * 3) - 1
      if (s < 0) s = 0
      if (s >= W) s = W - 1
      v = heat[(y + 1) * W + s] - rand() * decay
      if (rand() < gust) v -= 0.30
      heat[y * W + x] = (v > 0) ? v : 0
    }
  }
  # Heat enters only above the topmost dot of each column, offset by the
  # gap. The letter bodies stay cold, so no heat leaks between letters.
  for (x = 0; x < W; x++) {
    if (top[x] < 0) continue
    b = top[x] - 1 - gap
    if (b < 0) b = 0
    heat[b * W + x] = fuel * base * (0.7 + rand() * 0.6)
  }
}

function ramp(h,   i, t) {
  for (i = 1; i < nstop; i++) {
    if (h <= sp[i]) {
      t = (h - sp[i - 1]) / (sp[i] - sp[i - 1])
      return paint(sr[i - 1] + (sr[i] - sr[i - 1]) * t,
                   sg[i - 1] + (sg[i] - sg[i - 1]) * t,
                   sb[i - 1] + (sb[i] - sb[i - 1]) * t)
    }
  }
  return paint(sr[nstop - 1], sg[nstop - 1], sb[nstop - 1])
}

# Returns a red;green;blue triple, or an index into the 256 color cube.
function paint(r, g, b) {
  if (depth == 24) return int(r) ";" int(g) ";" int(b)
  return 16 + 36 * int(r / 255 * 5 + 0.5) + 6 * int(g / 255 * 5 + 0.5) \
            + int(b / 255 * 5 + 0.5)
}

function fg(c) { return (depth == 24) ? "\033[38;2;" c "m" : "\033[38;5;" c "m" }
function bg(c) { return (depth == 24) ? "\033[48;2;" c "m" : "\033[48;5;" c "m" }

function draw() {
  if (mode == "dots") drawdots()
  else drawhalf()
}

# Round glyphs use two character cells for each simulation cell. A cell is
# about twice as tall as it is wide, so the doubled width makes the dot
# grid square.
function drawdots(   y, x, line, c, cur, out) {
  out = ""
  for (y = 0; y < H; y++) {
    line = ""; cur = ""
    for (x = 0; x < W; x++) {
      c = cell(y, x)
      if (c == "") { line = line "  "; continue }
      if (c != cur) { line = line fg(c); cur = c }
      line = line "\342\227\217 "
    }
    out = out line "\033[0m\n"
  }
  printf "%s", out
  fflush()
}

function drawhalf(   y, x, line, ct, cb, curf, curb, ch, out) {
  out = ""
  for (y = 0; y < H; y += 2) {
    line = ""; curf = ""; curb = ""
    for (x = 0; x < W; x++) {
      ct = cell(y, x); cb = cell(y + 1, x)
      if (ct == "" && cb == "") {
        if (curb != "") { line = line "\033[49m"; curb = "" }
        line = line " "
        continue
      }
      # One half block carries the top color in the foreground and the
      # bottom color in the background.
      if (ct == "") { ch = "\204"; ct = cb; cb = "" }
      else ch = "\200"
      if (ct != curf) { line = line fg(ct); curf = ct }
      if (cb != curb) { line = line (cb == "" ? "\033[49m" : bg(cb)); curb = cb }
      line = line "\342\226" ch
    }
    out = out line "\033[0m\n"
  }
  printf "%s", out
  fflush()
}

function cell(y, x,   h, k, over) {
  k = y * W + x
  if (k in word) return wordcol
  h = heat[k]
  # No flame shows within gap cells of a letter dot, so a dark margin
  # traces the whole word outline.
  if (dist[k] <= gap) return ""
  if (k in ring) {
    # A word row cell above the local letter top is open sky, so the
    # flame base can ride the outline. Other word row cells stay masked.
    if (!(top[x] >= 0 && y < top[x])) h *= halo
  }
  over = dist[k] - pad
  if (over > 0) {
    h *= 1 - over / 2.5
    if (h < 0) h = 0
  }
  if (h < 0.07) return ""
  if (h > cap) h = cap
  return ramp(h)
}
' || true

  fire_show_cursor
  return 0
}

static_wordmark() {
  orange=$(printf '\033[38;5;208m')
  reset=$(printf '\033[0m')
  printf '%s\n' ''
  printf '%s%s%s\n' "$orange" ' ▄▄        █  ▀ █' "$reset"
  printf '%s%s%s\n' "$orange" ' ▄▄█ █▀▀▄ ▀█▀ █ █▀▀▄ █  █ █▄▀▀ █▀▀▄' "$reset"
  printf '%s%s%s\n' "$orange" '▀▄▄█ █  █  █▄ █ █▄▄▀ ▀▄▄█ █    █  █' "$reset"
}

# Print the wordmark and one line about what antiburn does. Fire, color,
# and art appear only on an interactive terminal that did not opt out; a
# piped or dumb terminal gets one plain line.
banner() {
  if [ -t 1 ] && [ "${TERM:-dumb}" != "dumb" ] && [ -z "${NO_COLOR:-}" ]; then
    fire_banner || static_wordmark
    printf '%s\n' ''
    printf '%s\n' 'Stop hitting your token limits.'
    printf '%s\n' 'antiburn reads your coding agent sessions locally, finds what'
    printf '%s\n' 'burns tokens, and nudges you before you hit a limit.'
    printf '%s\n' ''
  else
    info "Stop hitting your token limits - antiburn finds what burns tokens in your coding agent sessions."
  fi
}

# --- live status line ----------------------------------------------------
status_animates() {
  [ -t 1 ] && [ "${TERM:-dumb}" != "dumb" ] && [ -z "${NO_COLOR:-}" ]
}

spin_glyph() {
  case $(( $1 % 10 )) in
    0) printf '⠋' ;; 1) printf '⠙' ;; 2) printf '⠹' ;; 3) printf '⠸' ;;
    4) printf '⠼' ;; 5) printf '⠴' ;; 6) printf '⠦' ;; 7) printf '⠧' ;;
    8) printf '⠇' ;; 9) printf '⠏' ;;
  esac
}

# Settle the live line into an orange dot and the final text. A terminal
# that cannot animate gets a plain appended line with the same words.
status_done() {
  if status_animates; then
    printf '\r\033[K\033[38;5;208m●\033[0m %s\n' "$1"
  else
    info "$1"
  fi
}

fail() {
  printf '%s\n' "antiburn: error: $*" >&2
  exit 1
}

cleanup() {
  fire_show_cursor
  if [ -n "$MACOS_BACKUP" ] && [ -e "$MACOS_BACKUP" ] && [ ! -e "$MACOS_DESTINATION" ]; then
    as_root mv "$MACOS_BACKUP" "$MACOS_DESTINATION" >/dev/null 2>&1 || true
  fi
  if [ -n "$MACOS_STAGING_ROOT" ]; then
    as_root rm -rf "$MACOS_STAGING_ROOT" >/dev/null 2>&1 || true
  fi
  if [ -n "$APPIMAGE_STAGED" ]; then
    rm -f "$APPIMAGE_STAGED"
  fi
  if [ "$APPIMAGE_SWAP_COMPLETE" != "1" ]; then
    if [ "$APPIMAGE_NEW_INSTALLED" = "1" ]; then
      rm -f "$APPIMAGE_DESTINATION"
    fi
    if [ -n "$APPIMAGE_BACKUP" ] && [ -e "$APPIMAGE_BACKUP" ]; then
      mv "$APPIMAGE_BACKUP" "$APPIMAGE_DESTINATION" >/dev/null 2>&1 || true
      APPIMAGE_BACKUP=""
    fi
  fi
  if [ -n "$APPIMAGE_BACKUP" ]; then
    rm -f "$APPIMAGE_BACKUP"
  fi
  if [ -n "$MOUNT_POINT" ]; then
    hdiutil detach "$MOUNT_POINT" -quiet >/dev/null 2>&1 || true
  fi
  if [ -n "$TMP_DIR" ]; then
    rm -rf "$TMP_DIR"
  fi
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "$1 is required but was not found in PATH."
}

as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  else
    require_command sudo
    sudo "$@"
  fi
}

download() {
  url="$1"
  output="$2"
  curl --fail --silent --show-error --location \
    --proto '=https' --proto-redir '=https' \
    --retry 3 --retry-delay 1 \
    --output "$output" "$url"
}

# Download in the background while a spinner and a percentage rewrite one
# status line in place. A terminal that cannot animate gets the plain
# download with an appended log line.
download_with_progress() {
  dl_url="$1"
  dl_output="$2"
  dl_label="$3"
  if ! status_animates || ! sleep 0.01 >/dev/null 2>&1; then
    info "$dl_label"
    download "$dl_url" "$dl_output"
    return
  fi
  dl_length=$(curl --silent --location --head \
    --proto '=https' --proto-redir '=https' "$dl_url" 2>/dev/null \
    | awk 'tolower($1) == "content-length:" { n = $2 }
           END { sub(/\r/, "", n); if (n + 0 > 0) printf "%d", n }' || true)
  download "$dl_url" "$dl_output" &
  dl_pid=$!
  dl_i=0
  while kill -0 "$dl_pid" 2>/dev/null; do
    dl_i=$((dl_i + 1))
    dl_text="$dl_label"
    if [ -n "$dl_length" ] && [ -f "$dl_output" ]; then
      dl_size=$(wc -c < "$dl_output" 2>/dev/null || printf '0')
      dl_pct=$(awk -v s="$dl_size" -v l="$dl_length" \
        'BEGIN { p = int(s * 100 / l); if (p > 100) p = 100; print p }')
      dl_text="$dl_label ${dl_pct}%"
    fi
    printf '\r\033[K%s %s' "$(spin_glyph "$dl_i")" "$dl_text"
    sleep 0.08
  done
  printf '\r\033[K'
  wait "$dl_pid"
}

validate_version() {
  case "$1" in
    '' | *[!0-9A-Za-z.-]*) fail "Invalid version: $1" ;;
  esac
}

resolve_release() {
  requested_version="$1"
  if [ -n "$requested_version" ]; then
    validate_version "$requested_version"
    VERSION="$requested_version"
    TAG="antiburn-v${VERSION}"
    return
  fi

  info "Sniffing out the latest release"
  effective_url=$(curl --fail --silent --show-error --location \
    --proto '=https' --proto-redir '=https' \
    --retry 3 --retry-delay 1 \
    --output /dev/null --write-out '%{url_effective}' \
    "${GITHUB_URL}/releases/latest") || fail "Could not resolve the latest release."
  TAG=${effective_url##*/}
  case "$TAG" in
    antiburn-v*) VERSION=${TAG#antiburn-v} ;;
    *) fail "GitHub returned an invalid release tag: $TAG" ;;
  esac
  validate_version "$VERSION"
  [ "$TAG" = "antiburn-v${VERSION}" ] || fail "GitHub returned an invalid release tag: $TAG"
}

expected_checksum() {
  checksum_file="$1"
  asset_name="$2"
  matches=$(awk -v name="$asset_name" '
    $2 == name || $2 == "*" name {
      if ($1 ~ /^[0-9A-Fa-f]{64}$/) print tolower($1)
    }
  ' "$checksum_file")
  count=$(printf '%s\n' "$matches" | awk 'NF { count++ } END { print count + 0 }')
  [ "$count" -eq 1 ] || fail "SHA256SUMS must contain exactly one valid entry for ${asset_name}."
  printf '%s\n' "$matches"
}

actual_checksum() {
  file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{ print tolower($1) }'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{ print tolower($1) }'
  else
    fail "sha256sum or shasum is required to verify the download."
  fi
}

verify_checksum() {
  file="$1"
  checksum_file="$2"
  asset_name=${file##*/}
  expected=$(expected_checksum "$checksum_file" "$asset_name")
  actual=$(actual_checksum "$file")
  [ "$actual" = "$expected" ] || fail "Checksum verification failed for ${asset_name}."
  status_done "Verified SHA-256 for ${asset_name}. Exactly the bytes we published"
}

verify_attestation_if_requested() {
  file="$1"
  if [ "${ANTIBURN_VERIFY_ATTESTATION:-0}" != "1" ]; then
    return
  fi
  require_command gh
  gh release verify-asset "$TAG" "$file" --repo "$REPOSITORY" >/dev/null \
    || fail "GitHub could not verify the release attestation for ${file##*/}."
  info "Verified the GitHub release attestation"
}

verify_macos_app() {
  app="$1"
  [ -d "$app" ] || fail "The DMG does not contain antiburn.app."
  codesign --verify --deep --strict "$app" >/dev/null 2>&1 \
    || fail "The antiburn application signature is invalid."
  spctl --assess --type execute "$app" >/dev/null 2>&1 \
    || fail "Gatekeeper did not accept antiburn.app."
  bundle_id=$(defaults read "$app/Contents/Info" CFBundleIdentifier 2>/dev/null || true)
  [ "$bundle_id" = "ai.antiburn.desktop" ] \
    || fail "The application has an unexpected bundle identifier: ${bundle_id:-missing}."
}

install_macos() {
  arch="$1"
  require_command hdiutil
  require_command codesign
  require_command spctl
  require_command defaults
  require_command ditto
  require_command id
  require_command sw_vers

  macos_major=$(sw_vers -productVersion | awk -F. '{ print $1 }')
  case "$macos_major" in
    '' | *[!0-9]*) fail "Could not determine the macOS version." ;;
  esac
  [ "$macos_major" -ge 13 ] || fail "antiburn requires macOS 13 or later."

  case "$arch" in
    arm64) arch_label="aarch64" ;;
    x86_64) arch_label="x64" ;;
    *) fail "Unsupported macOS architecture: $arch" ;;
  esac

  asset_name="antiburn_${VERSION}_${arch_label}.dmg"
  asset_path="${TMP_DIR}/${asset_name}"
  download_release_asset "$asset_name" "$asset_path"
  hdiutil verify "$asset_path" >/dev/null || fail "The DMG failed its internal verification."

  MOUNT_POINT="${TMP_DIR}/mount"
  mkdir "$MOUNT_POINT"
  hdiutil attach "$asset_path" -readonly -nobrowse -mountpoint "$MOUNT_POINT" >/dev/null \
    || fail "Could not mount the DMG."
  source_app="${MOUNT_POINT}/antiburn.app"
  verify_macos_app "$source_app"

  MACOS_DESTINATION="/Applications/antiburn.app"
  if [ "$(id -u)" -ne 0 ]; then
    info "macOS wants your password to move antiburn into /Applications"
  fi
  MACOS_STAGING_ROOT=$(as_root mktemp -d "/Applications/.antiburn-install.XXXXXX") \
    || fail "Could not create a staging directory in /Applications."
  as_root chmod 755 "$MACOS_STAGING_ROOT"
  staged="${MACOS_STAGING_ROOT}/antiburn.app"
  MACOS_BACKUP="${MACOS_STAGING_ROOT}/previous.app"
  info "Moving antiburn into ${MACOS_DESTINATION}"
  as_root ditto "$source_app" "$staged"
  verify_macos_app "$staged"
  if [ -e "$MACOS_DESTINATION" ]; then
    as_root mv "$MACOS_DESTINATION" "$MACOS_BACKUP"
  fi
  if ! as_root mv "$staged" "$MACOS_DESTINATION"; then
    if [ -e "$MACOS_BACKUP" ]; then
      as_root mv "$MACOS_BACKUP" "$MACOS_DESTINATION" || true
    fi
    fail "Could not replace ${MACOS_DESTINATION}."
  fi
  as_root rm -rf "$MACOS_BACKUP"
  MACOS_BACKUP=""
  as_root rm -rf "$MACOS_STAGING_ROOT"
  MACOS_STAGING_ROOT=""
  status_done "Moved in. antiburn ${VERSION} is in /Applications"
}

install_deb() {
  require_command id
  asset_name="antiburn_${VERSION}_amd64.deb"
  asset_path="${TMP_DIR}/${asset_name}"
  download_release_asset "$asset_name" "$asset_path"

  package_name=$(dpkg-deb -f "$asset_path" Package)
  package_arch=$(dpkg-deb -f "$asset_path" Architecture)
  package_version=$(dpkg-deb -f "$asset_path" Version)
  [ "$package_name" = "antiburn" ] || fail "The Debian package has an unexpected name: $package_name."
  [ "$package_arch" = "amd64" ] || fail "The Debian package has an unexpected architecture: $package_arch."
  [ "$package_version" = "$VERSION" ] || fail "The Debian package has an unexpected version: $package_version."

  info "Installing the Debian package"
  # Debian compares a hyphenated prerelease as newer than the stable version.
  # The selected local package is already pinned to the requested release and verified.
  as_root apt-get install --yes --allow-downgrades "$asset_path"
  info "Installed antiburn ${VERSION} with APT"
}

install_appimage() {
  [ -n "${HOME:-}" ] || fail "HOME is required for an AppImage installation."
  asset_name="antiburn_${VERSION}_amd64.AppImage"
  asset_path="${TMP_DIR}/${asset_name}"
  download_release_asset "$asset_name" "$asset_path"

  applications_dir="${HOME}/Applications"
  bin_dir="${HOME}/.local/bin"
  destination="${applications_dir}/antiburn.AppImage"
  APPIMAGE_DESTINATION="$destination"
  APPIMAGE_STAGED="${applications_dir}/.antiburn.AppImage.$$"
  APPIMAGE_BACKUP="${applications_dir}/.antiburn-backup.$$"
  mkdir -p "$applications_dir" "$bin_dir"
  chmod 755 "$asset_path"
  mv "$asset_path" "$APPIMAGE_STAGED"
  if [ -e "$destination" ]; then
    mv "$destination" "$APPIMAGE_BACKUP"
  else
    APPIMAGE_BACKUP=""
  fi
  APPIMAGE_NEW_INSTALLED="1"
  mv -f "$APPIMAGE_STAGED" "$destination"
  APPIMAGE_STAGED=""
  APPIMAGE_SWAP_COMPLETE="1"
  if [ -n "$APPIMAGE_BACKUP" ]; then
    rm -f "$APPIMAGE_BACKUP"
    APPIMAGE_BACKUP=""
  fi
  link_path="${bin_dir}/antiburn"
  if [ -L "$link_path" ]; then
    require_command readlink
    if [ "$(readlink "$link_path")" != "$destination" ]; then
      info "Not replacing the existing link at ${link_path}."
    fi
  elif [ -e "$link_path" ]; then
    info "Not replacing the existing path at ${link_path}."
  elif ! ln -s "$destination" "$link_path"; then
    info "Could not create the optional command link at ${link_path}."
  fi
  info "Installed antiburn ${VERSION} to ${destination}"
  case ":${PATH:-}:" in
    *":${bin_dir}:"*) ;;
    *) info "Add ${bin_dir} to PATH to run antiburn from a terminal." ;;
  esac
}

install_linux() {
  arch="$1"
  case "$arch" in
    x86_64 | amd64) ;;
    *) fail "Unsupported Linux architecture: $arch" ;;
  esac

  if command -v apt-get >/dev/null 2>&1 && command -v dpkg-deb >/dev/null 2>&1; then
    install_deb
  else
    install_appimage
  fi
}

download_release_asset() {
  asset_name="$1"
  asset_path="$2"
  release_url="${GITHUB_URL}/releases/download/${TAG}"
  checksum_path="${TMP_DIR}/SHA256SUMS"
  if [ ! -f "$checksum_path" ]; then
    download "${release_url}/SHA256SUMS" "$checksum_path" \
      || fail "Could not download SHA256SUMS for ${TAG}."
  fi
  download_with_progress "${release_url}/${asset_name}" "$asset_path" \
    "Downloading ${asset_name}" \
    || fail "Could not download ${asset_name}."
  verify_checksum "$asset_path" "$checksum_path"
  verify_attestation_if_requested "$asset_path"
}

parse_args() {
  VERSION_REQUESTED="${ANTIBURN_VERSION:-}"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --version)
        [ "$#" -ge 2 ] || fail "--version requires a value."
        VERSION_REQUESTED="$2"
        shift 2
        ;;
      --help)
        printf '%s\n' "Usage: install.sh [--version VERSION]"
        exit 0
        ;;
      *) fail "Unknown argument: $1" ;;
    esac
  done
}

install_antiburn() {
  parse_args "$@"
  require_command curl
  require_command awk
  require_command mktemp
  require_command uname
  # The traps come before the banner, so an interrupt during the fire
  # still restores the cursor through cleanup.
  trap cleanup EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM
  banner
  TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/antiburn-install.XXXXXX") \
    || fail "Could not create a temporary directory."

  resolve_release "$VERSION_REQUESTED"
  os=$(uname -s)
  arch=$(uname -m)
  case "${os}/${arch}" in
    Darwin/arm64) build_target="your Mac (Apple Silicon)" ;;
    Darwin/*) build_target="your Mac (Intel)" ;;
    *) build_target="${os}/${arch}" ;;
  esac
  info "Found ${VERSION}. Built for ${build_target}"
  case "$os" in
    Darwin) install_macos "$arch" ;;
    Linux) install_linux "$arch" ;;
    *) fail "Unsupported operating system: $os" ;;
  esac
  info "antiburn lives in your menu bar, up by the clock."
}

install_antiburn "$@"
