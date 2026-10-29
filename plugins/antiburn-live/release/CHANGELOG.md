# Changelog

## 0.2.5 — mod composition and responsive Desktop usage

- Preserve unrelated AbovePrompt mods in either stacking order.
- Call downstream rendering once and retain its UI if quota rendering fails.
- Keep surveys, hidden bands, usage spacing and host row budgets unchanged.
- Remove the optional development-plugin probe from public rendering.
- Fill the host-allocated Desktop meter slots without assuming a fixed cell size.
- Keep glyphs and dots at their existing size as the window changes.
- Preserve Terminal rendering and the existing quota behavior.
- Add a browser geometry regression gate across sizes, breakpoints and readings.

## 0.2.4 — listing metadata

- Add 17 discovery keywords for usage, limits, reset times and pace.
- Remove the decorative usage bar from the README introduction only.
- Preserve the plugin description and runtime behavior.

## 0.2.3 — directory validation fixes

- Keep registration, command hooks and quota capability helpers in one source module.
- Spell out fixed browser-opening commands with bounded execution time.
- Add a public privacy policy URL and document capabilities for review.
- Remove the unused manifest types field; retain the TypeScript declarations.
- Use the existing 512px brand icon without metadata, below 256 KiB.
- Separate the SVG namespace constant from dynamic local image markup.
- Preserve usage behavior, plugin identity, and the existing shared core.

## 0.2.2 — marketplace preparation

- Update the listing description, Antiburn publisher and plugin homepage.
- Replace testing-branch instructions with the permanent installation route.
- Add public Desktop Code and Terminal previews with presentation captions.
- Preserve the 0.2.1 usage runtime and embedded shared core unchanged.
- Prepare the package for directory review; submission and publication remain separate.

## 0.2.1 — testing candidate

- Show usage automatically without an install-time configuration setting.
- Ignore the retired public usage display setting from earlier installations.

## 0.2.0 — local unified candidate

- Show usage with `/antiburn`, keep help to two lines, and confirm refresh briefly.
- Keep technical diagnostics and display flags in the development command.

- Register canonical `/antiburn` directly; remove alternate names, legacy aliases and the separate help skill.
- Replace the owned standalone findings skill; keep findings and CLI access in local development.

- Uses one `antiburn` installation and native command router for help and usage.
- Preserves the 0.1.4 native quota UI and authenticated Desktop fallback.
- Adds optional usage UI and explicit namespaced help when mods alone are blocked.
- Separates local `antiburn-dev` checks, previews and CLI discovery from public bytes.
- Prepares isolated identity migration and independent recovery; remains unpublished.

## 0.1.4 — testing candidate

- Reduces desktop labels to 14px, dots to 7px, and branding to 13px.
- Softens filled dots, delays stacking until 480px, and tightens stacked and status layouts.
- Shows whole-number desktop percentages without rounding into exhaustion.
- Uses steady red dots and pace markers at 100% usage and above.
- Uses solid pace markers with a subtle right-edge shadow drawn once per meter.
- Keeps narrow desktop accessibility labels consistent with visible percentages.
- Preserves exact source readings and terminal formatting.

## 0.1.3 — previous testing candidate

- Keeps meter graphics stable during unrelated redraws by using pixel-aligned pace markers.
- Adds 4 px of space above Desktop usage and one row above terminal usage.
- Omits the extra gap in bands below eight rows or 24 columns to preserve content.
- Targets the testing branch marketplace without a GitHub Release.

## 0.1.2 — previous testing candidate

- Automatically fetches account usage after Desktop attaches when native data is empty or unavailable.
- Uses the host's opaque bearer handle; credentials stay on the host and no request body is sent.
- Keeps valid native readings primary, including partial windows, without account fill.
- Shares one request, reuses successful readings for 60 seconds, and backs off failures with HTTP 429 Retry-After support.
- Preserves prior readings on failure and rejects late results after native events, reset, or detach.
- Shows Checking usage... during a read without numeric usage, then Usage unavailable with a refresh icon if no reading is found.
- Shows one loading status without a duplicate pending control and deduplicates repeated clicks.
- Shows account availability in the runtime timezone with an explicit zone and UTC fallback; combines retry timing and session-refresh guidance in one wrapping line.
- Uses dim Bitcount secondary descriptions on Desktop while retaining native Refresh accessibility and terminal font support.
- Keeps terminal-only usage native; adds no prompt/model calls, usage-fetch or retry timers, polling, or cache/file access.
- Animates pending dots with Desktop CSS honoring reduced motion and a terminal client timer scoped to the mounted indicator.
- Distributes through an immutable source in the testing branch catalog.

## 0.1.1 — previous candidate

- Uses the display name antiburn (Mod), with a separate local dev label.
- Re-reads native usage after a Desktop client attaches.
- Adds an unavailable-state Refresh action and `/antiburn-mod refresh`.
- Shows `[refresh]` in the command hint and retains the `details` argument.
- Makes no prompt or model request to refresh usage; missing host data stays unknown.

## 0.1.0 — initial candidate

- Introduces the `antiburn-mod` identity and `/antiburn-mod` command.
- Shows native five-hour and weekly account usage, reset times, and pace.
- Preserves logo website links, quota hover reveals, and keyboard activation.
- Keeps missing data and stale readings distinct from fresh quota measurements.
- Distributes only the usage implementation, with an included MIT license,
  Bitcount attribution, and the existing antiburn app icon.

The entries above retain their historical testing status. Current candidate
checks and remaining directory gates are recorded in the repository
[submission handoff](https://github.com/antiburn/antiburn/blob/main/docs/plugins/marketplace-submission.md).
A package version does not establish directory acceptance or publication.
