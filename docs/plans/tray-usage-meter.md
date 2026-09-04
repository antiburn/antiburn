# Tray usage meter

## Goal

Make the antiburn tray icon show the remaining provider allowance without
opening the popover. Each vertical dot column in the existing mark is one meter
segment.

## Behavior

- The tray starts with all five columns bright at application launch.
- It then moves to the cached usage state over at most 1.5 seconds.
- Columns deplete from right to left. Every dot in a column changes together,
  so the meter reads as a bar.
- A column stays bright while it represents remaining allowance. A depleted
  column stays visible at low alpha, so the tray target remains discoverable at
  0%.
- Remaining allowance rounds to the nearest whole column.
- Later provider updates animate between states over the existing 300ms meter
  duration. A newer update cancels an older transition.

## Usage selection

The meter shows the lowest remaining allowance from every provider account and
window that the Usage surface would show. This is the highest valid
`used_percent` after the existing rules for visible supplemental windows,
Antigravity primary windows, and the ten-minute failed-refresh grace period.

The normal full mark is neutral, not a claim that usage is available. It stays
full when onboarding is unfinished, live usage is disabled, all readings are
unknown, or all readings have failed past their grace period. The meter never
uses transcript cost estimates because they have no allowance denominator.

## Design and implementation

- Keep `icons/tray.png` as the owner-provided source image.
- Derive runtime RGBA frames in the native shell. The tray remains a macOS
  template image, so AppKit still tints it for light, dark, and pressed states.
- Use Tauri's atomic `set_icon_with_as_template` update to avoid macOS
  repaint flicker. Windows and Linux use the same generated frame through
  Tauri's fallback.
- Update the target after the persisted snapshot is restored at launch and
  after every `refresh_publish_and_evaluate` publication.
- Do not add polling or provider traffic for this feature.

## Tests

- Test usage selection, grace handling, and unknown-data fallback.
- Test percentage quantization and the right-to-left dot order.
- Test frame alpha changes without changing the source RGB channels.
- Test immediate and animated state transitions without a native tray.
- Manually verify native rendering on macOS light, dark, pressed, and
  highlighted states, plus Windows and Linux tray visibility.
