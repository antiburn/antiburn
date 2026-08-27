# Plan: DMG installer appearance

Give the macOS disc image a designed background — wordmark, dotted drag arrow, brand dark ground — and position the app icon and Applications alias to line up with it. macOS only; Windows NSIS branding is a possible follow-up, Linux has no equivalent surface.

Mockup: `dmg-mockup.html` in the session scratchpad (sent alongside this plan). The mockup's SVG becomes the real background asset.

## What we're changing

**Today**: antiburn ships Tauri's default DMG — plain white Finder window, 660×400, app at (180, 170), Applications at (480, 170), no background.

**After**: dark `#333844` background with the orange wordmark up top, a dotted `#FF6A2C` arrow from the app icon to the Applications folder, and logomark-style corner dot grids. Window grows slightly to 660×440 for wordmark headroom.

**Fixed constraint**: icon size stays 128 px. Tauri's bundler hard-codes `ICON_SIZE=128` in its vendored `bundle_dmg` script and exposes no config for it. 128 is already the large end for DMGs; the designed background does the perceptual work. If it still feels small after this lands, the escape hatch (build `--bundles app` + run `create-dmg` ourselves in CI) is a separate decision — not in this plan.

## Steps

### 1. Background asset — `apps/desktop/src-tauri/dmg/`

- `background.svg` — the source art, adapted from the approved mockup (660×440 design space). Checked in as the editable source of truth.
- `background.png` (660×440) and `background@2x.png` (1320×880) — exported renders.
- `background.tiff` — the two PNGs combined into a multi-resolution TIFF so Finder picks the 2x on Retina:
  ```
  tiffutil -cathidpicheck background.png background@2x.png -out background.tiff
  ```
- `apps/desktop/scripts/generate-dmg-background.sh` — regenerates the PNGs and TIFF from the SVG (rsvg-convert or headless Chrome for the raster step, `tiffutil` for the combine; macOS-only script, outputs are committed so CI needs nothing new).

Rationale for committing generated files: the release workflow's macOS runners then need zero extra tooling, and the bundler just gets handed a file path.

### 2. Config — `apps/desktop/src-tauri/tauri.conf.json`

Add under `bundle.macOS`:

```json
"dmg": {
  "background": "dmg/background.tiff",
  "windowSize": { "width": 660, "height": 440 },
  "appPosition": { "x": 170, "y": 225 },
  "applicationFolderPosition": { "x": 490, "y": 225 }
}
```

The path resolves relative to `src-tauri/` (the bundler joins it to the build cwd). Tauri passes the background file straight through to its DMG script, so the TIFF works even though the config docs only mention png/jpg/gif — **verify this in step 3 and fall back to `background.png` if the schema validator or Finder rejects it**.

### 3. Local verification and calibration

- `pnpm tauri build --bundles dmg` in `apps/desktop` (unsigned local build is fine for this).
- Mount the DMG, screenshot, and check: background not tiled/cropped, icons sitting on the arrow line, labels legible, Retina crispness.
- Calibrate: the position coordinates are Finder icon-view positions and may need a nudge of ±10–20 px against the artwork — adjust config or art, whichever is cheaper, and re-export.
- Also check the mounted window in light-mode Finder (the dark background design should be theme-proof, but confirm label contrast — Finder draws white labels on DMGs with backgrounds set by this script; if labels come out black on any OS version, darken the label strip area in the art).

### 4. Release plumbing check

Nothing expected: `release-app.yml` already builds `app,dmg` on the macOS matrix legs, and the new files ride along in the repo. Confirm the DMG artifact in a CI dry run still uploads and the updater artifacts are untouched (background only affects the `.dmg`, not the `.app.tar.gz` updater bundle).

## Out of scope (possible follow-ups)

- **Windows NSIS branding** — `installerIcon`, `headerImage` (150×57), `sidebarImage` (164×314) under `bundle.windows.nsis`. Same brand pass, different artwork; separate small PR.
- **Linux** — no install-time visual surface exists (AppImage runs directly, deb installs via the package manager). Nothing to do.
- **Icon size > 128 px** — needs replacing Tauri's DMG bundler with a manual `create-dmg` CI step. Only revisit if the shipped result disappoints.

## Risks

| Risk | Handling |
| --- | --- |
| TIFF rejected by config schema or Finder | Fall back to `background.png` (1x only, slightly soft on Retina) |
| Position coordinates off vs artwork | Calibration loop in step 3; art and config live side by side |
| White labels illegible on some Finder theme | Verified in step 3; art adjusts if needed |

## Delivery

One PR, well under the 1k-line cap (art assets aside). Commits signed off (`git commit -s`) from the first commit — DCO fails the whole PR otherwise.
