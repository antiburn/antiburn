# App icons

Owner-provided artwork (2026-08-13): a dot-grid mark on a dark plate. The
1024px master is `icon.png`; `512x512.png`, `128x128@2x.png` (256px),
`128x128.png`, and `32x32.png` are derived from it with `sips -Z`.
`tray.png` (44px, 22pt @2x) is the menu-bar glyph, loaded with
`icon_as_template(true)` on macOS so the system recolors it for the menu
bar; only its alpha channel is significant there.

A separate 256px copy ships in the frontend at `src/assets/app-icon.png`
for in-app use (onboarding Welcome step, About).

`icon.icns` and `icon.ico` are intentionally not checked in — `tauri icon`
produces them at packaging time from these PNGs.

`scripts/generate-icons.mjs` produced the previous placeholder mark and is
retained only as provenance for git history; it is no longer the source of
these files.
