#!/usr/bin/env bash
# Regenerate the DMG background renders from background.svg.
# The script runs on macOS only. Commit the output files.
# Requires: rsvg-convert (brew install librsvg) or Google Chrome, and tiffutil.
set -euo pipefail

DMG_DIR="$(cd "$(dirname "$0")/../src-tauri/dmg" && pwd)"
SVG="$DMG_DIR/background.svg"
PNG_1X="$DMG_DIR/background.png"
PNG_2X="$DMG_DIR/background@2x.png"
TIFF="$DMG_DIR/background.tiff"

WIDTH=660
HEIGHT=440

CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

render() {
  # $1 = scale factor, $2 = output path
  if command -v rsvg-convert >/dev/null 2>&1; then
    rsvg-convert -w $((WIDTH * $1)) -h $((HEIGHT * $1)) "$SVG" -o "$2"
  elif [ -x "$CHROME" ]; then
    # Chrome screenshots the SVG page at the exact window size.
    "$CHROME" --headless --disable-gpu --hide-scrollbars \
      --window-size="$WIDTH,$HEIGHT" --force-device-scale-factor="$1" \
      --screenshot="$2" "file://$SVG" 2>/dev/null
  else
    echo "error: need rsvg-convert or Google Chrome to render the SVG" >&2
    exit 1
  fi
}

render 1 "$PNG_1X"
render 2 "$PNG_2X"

# Combine 1x and 2x into one TIFF so Finder picks the 2x on Retina displays.
tiffutil -cathidpicheck "$PNG_1X" "$PNG_2X" -out "$TIFF"

echo "wrote:"
ls -la "$PNG_1X" "$PNG_2X" "$TIFF"
