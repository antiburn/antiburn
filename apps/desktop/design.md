---
version: alpha
name: antiburn Desktop
description: "antiburn desktop design system (native-feeling, Tauri; ships macOS/Windows/Linux). Light and Dark are documented together: every colors entry carries both values. Use semantic Tailwind utilities for shared product chrome; promote recurring feature values to tokens and keep one-off visualization or geometry values local and documented."
sources:
  - src/styles/tokens.css
  - src/styles/base.css
  - src/styles/typography.css
  - src/styles/focus.css
  - src/styles/controls.css
  - src/styles/motion.css
  - src/styles/platform-controls.css
  - src/styles/hud.css
  - src/styles/main-window.css
  - src/styles/burn-checks-report.css
  - src/styles/interface-scale.css
  - src/styles/session-analysis-colors.css
  - src/styles/session-rows.css
  - src/styles/session-detail.css
  - src/components/ui/text-roll.css
  - src/components/burn-checks/burn-check-summary.css
  - src/views/main-window/overview/overview.css
  - src/views/main-window/quota/quota.css
colors:
  # Concrete token colors use modern HSL function syntax.
  # Use the shortest value that keeps the same 8-bit RGB channels.
  # Hue uses at most one decimal. Saturation and lightness use at most two decimals.
  # Alpha uses at most three decimals. Remove trailing zeros. Achromatic colors use hue 0.
  # name → Tailwind utility via bg-/text-/border-<name>
  # System selects the same palette as explicit Light or Dark from the OS preference.
  # The drift check requires the system and explicit palette values to match.
  surface: # the menu-bar popover, which sits on the window material
    light: "hsl(0 0% 100% / 0.85)" # reduced-transparency: hsl(0 0% 100%)
    dark: "hsl(0 0% 11.7% / 0.92)"
  surface-secondary:
    light: "hsl(0 0% 0% / 0.08)"
    dark: "hsl(0 0% 100% / 0.12)"
  surface-tertiary:
    light: "hsl(0 0% 0% / 0.12)"
    dark: "hsl(0 0% 100% / 0.18)"
  surface-card:
    light: "hsl(0 0% 0% / 0.04)"
    dark: "hsl(0 0% 100% / 0.08)"
  stats-edge:
    light: "hsl(0 0% 0% / 0.04)"
    dark: "hsl(0 0% 100% / 0.05)"
  session-card: # quiet session-list rest fill; dark mode needs less lift than generic cards
    light: "hsl(0 0% 0% / 0.02)"
    dark: "hsl(0 0% 100% / 0.03)"
  surface-key: # a translucent white pill over a chart; the same in both themes
    light: "hsl(0 0% 100% / 0.2)"
    dark: "hsl(0 0% 100% / 0.2)"
  surface-header: # the quiet band at the head of the menu-bar popover; fainter than a card
    light: "hsl(0 0% 0% / 0.025)"
    dark: "hsl(0 0% 100% / 0.03)"
  surface-hover: # stays clear of surface-selected, so a hover never reads as a selection
    light: "hsl(0 0% 0% / 0.04)"
    dark: "hsl(0 0% 100% / 0.04)"
  surface-window: # standard decorated window
    light: "hsl(0 0% 96.4%)"
    dark: "hsl(0 0% 12.5%)"
  surface-overlay: # opaque search dialog and other floating surfaces
    light: "hsl(0 0% 96.4%)"
    dark: "hsl(0 0% 12.5%)"
  command-palette-scrim:
    light: "hsl(0 0% 0% / 0.04)"
    dark: "hsl(0 0% 0% / 0.12)"
  surface-sidebar: # source-list / sidebar material
    light: "hsl(0 0% 0% / 0.03)"
    dark: "hsl(0 0% 100% / 0.04)"
  surface-selected: # selected row in a list or source list (accent-fill stays for controls)
    light: "hsl(0 0% 0% / 0.09)"
    dark: "hsl(0 0% 100% / 0.14)"
  input-fill:
    light: "hsl(0 0% 100%)"
    dark: "hsl(240 1.6% 23%)"
  label: # live system label token where available
    light: "hsl(0 0% 0% / 0.85)"
    dark: "hsl(0 0% 100% / 0.92)"
  label-secondary:
    light: "hsl(240 5.5% 25% / 0.85)"
    dark: "hsl(240 33% 94% / 0.72)"
  label-tertiary: # 4.5:1 on a card on the popover, over any desktop behind it
    light: "hsl(240 5.5% 25% / 0.79)"
    dark: "hsl(240 33% 94% / 0.62)"
  separator: # live system separator token where available
    light: "hsl(0 0% 0% / 0.15)"
    dark: "hsl(0 0% 100% / 0.18)"
  accent: # live system accent token where available
    light: "hsl(211.2 100% 50%)"
    dark: "hsl(210 100% 51.9%)"
  accent-hover: # darker than accent-fill, because white text sits on it
    light: "hsl(210.4 100% 39%)"
    dark: "hsl(213.5 91% 42%)"
  accent-fill: # concrete fill; use bg-accent-fill for backgrounds
    light: "hsl(210 100% 44.5%)"
    dark: "hsl(213.3 92% 48%)"
  selected-fill: # the solid chip of a view picker; the neutral furthest from the track
    light: "hsl(240 6% 26%)"
    dark: "hsl(240 6% 84%)"
  selected-ink: # the ink on that chip
    light: "hsl(0 0% 100%)"
    dark: "hsl(240 4% 12%)"
  brand: # antiburn orange for text and small glyphs
    light: "hsl(18 92% 39%)"
    dark: "hsl(17.6 100% 54%)"
  brand-tint: # antiburn orange for large fills
    light: "hsl(17.6 100% 54%)"
    dark: "hsl(17.6 100% 54%)"
  brand-unlit: # an unlit meter segment: the brand tint, a quarter less saturated
    light: "hsl(17.7 75% 54%)"
    dark: "hsl(17.7 75% 54%)"
  system-green:
    light: "hsl(135 59% 34%)"
    dark: "hsl(135 70% 52.3%)"
  system-orange:
    light: "hsl(27 100% 35.1%)"
    dark: "hsl(36 100% 62.5%)"
  system-orange-tint:
    light: "hsl(35 100% 50%)"
    dark: "hsl(36.4 100% 52%)"
  system-yellow:
    light: "hsl(34 100% 31.3%)"
    dark: "hsl(48 100% 57.4%)"
  system-yellow-tint: # vivid yellow for fills (a category, not a warning)
    light: "hsl(45 100% 50%)"
    dark: "hsl(48 100% 57.4%)"
  system-yellow-unlit: # the yellow fill, a quarter less saturated
    light: "hsl(45 75% 50%)"
    dark: "hsl(48 75% 57.4%)"
  system-red:
    light: "hsl(354 100% 42.1%)"
    dark: "hsl(3 100% 69%)"
  system-red-tint: # vivid red for fills (a meter's critical zone)
    light: "hsl(357 91% 52%)"
    dark: "hsl(357 100% 65%)"
  system-red-unlit: # an unlit critical segment
    light: "hsl(357 68.2% 52%)"
    dark: "hsl(357 75% 65%)"
  system-red-text:
    light: "hsl(353.6 100% 37.2%)"
    dark: "hsl(5 100% 75%)"
  system-blue:
    light: "hsl(211.2 100% 50%)"
    dark: "hsl(210 100% 51.9%)"
  system-indigo:
    light: "hsl(241 61% 58.8%)"
    dark: "hsl(241 73% 63%)"
  system-indigo-text:
    light: "hsl(241 61% 58.8%)"
    dark: "hsl(241 100% 79%)"
  system-gold:
    light: "hsl(40.6 96% 40.4%)"
    dark: "hsl(48 100% 50%)"
  shimmer: # the running-session title sweep. One value for both themes: white
    # lifts the near-white glyphs in dark mode and washes out the near-black
    # ones in light mode. Below 4.5:1 on a light row on purpose; the band is
    # transient and the text under it is legible at rest.
    light: "hsl(0 0% 100%)"
    dark: "hsl(0 0% 100%)"
  system-gold-text:
    light: "hsl(41 100% 28.6%)"
    dark: "hsl(43.5 88% 66%)"
  agent-mark: # vendor brand-mark ink; see the Vendor brand marks note below
    light: "hsl(52 11% 13.3%)"
    dark: "hsl(60 15% 96.2%)"
  burn-check-failure-fill: # failure arcs and terminal marks
    light: "hsl(17.6 100% 54%)"
    dark: "hsl(17.6 100% 54%)"
  burn-check-failure-text: # failure wording on session and summary surfaces
    light: "hsl(18 100% 36.4%)"
    dark: "hsl(18 100% 68%)"
  burn-check-pass-fill: # pass arcs and terminal marks
    light: "hsl(191.5 83% 36.8%)"
    dark: "hsl(192 63% 47.6%)"
  burn-check-neutral: # unassessed arcs and neutral lifecycle marks
    light: "hsl(209 6% 73.7%)"
    dark: "hsl(210 3% 50.5%)"
  # Main Burn Checks category icons; each circle derives its tint from the icon.
  check-tools:
    light: "hsl(27.5 89.4% 51.7%)"
    dark: "hsl(28.7 100% 59%)"
  check-mcp:
    light: "hsl(196.2 100% 42.7%)"
    dark: "hsl(196 86.4% 53.5%)"
  check-overthinking:
    light: "hsl(263 88% 64.4%)"
    dark: "hsl(260 100% 73.3%)"
  check-skills:
    light: "hsl(47 100% 44.9%)"
    dark: "hsl(47.6 100% 58.2%)"
  check-subagents:
    light: "hsl(341 88% 59.4%)"
    dark: "hsl(342.8 100% 65%)"
  check-old-model:
    light: "hsl(40 100% 47%)"
    dark: "hsl(38.3 100% 58.6%)"
  check-fast-mode:
    light: "hsl(47 100% 30.5%)"
    dark: "hsl(57.6 91% 55.5%)"
  check-cache:
    light: "hsl(175.3 100% 35.5%)"
    dark: "hsl(174.3 74% 47.6%)"
  check-depth:
    light: "hsl(158 89% 39.6%)"
    dark: "hsl(156 75% 47.2%)"
  # Floating-HUD sub-palette only (src/styles/hud.css)
  burn:
    light: "hsl(18 100% 50%)"
    dark: "hsl(25 100% 50%)"
  burn-muted:
    light: "hsl(18.1 88% 51.4%)"
    dark: "hsl(23 88.8% 54%)"
  bg-hud:
    light: "hsl(0 0% 96.4%)"
    dark: "hsl(0 0% 12.5%)"
  bg-hud-hover: # the HUD surface on hover; the desktop stays visible through it
    light: "hsl(0 0% 96.4% / 0.9)"
    dark: "hsl(0 0% 12.5% / 0.9)"
  hud-frame: # the HUD frame at rest and on hover, 70% white in light and 70% black in dark, used as `bg-hud-frame`; reduced-transparency: hsl(0 0% 100%) in light and hsl(0 0% 0%) in dark
    light: "hsl(0 0% 100% / 0.7)"
    dark: "hsl(0 0% 0% / 0.7)"
  hud-stroke-top: # top of the HUD frame's one-pixel gradient stroke; softer on the dark frame
    light: "hsl(0 0% 100% / 0.9)"
    dark: "hsl(0 0% 100% / 0.3)"
  hud-stroke-bottom: # bottom of that stroke
    light: "hsl(0 0% 0% / 0.22)"
    dark: "hsl(0 0% 0% / 0.22)"
  hud-control-ink: # the close control's glyph; no alpha, so it stays solid on the hover surface
    light: "hsl(0 0% 32%)"
    dark: "hsl(0 0% 78%)"
  hud-control-edge: # the close control's edge; no alpha, for the same reason
    light: "hsl(0 0% 80%)"
    dark: "hsl(0 0% 32%)"
  hud-island: # the notch island's panel, used as `bg-hud-island`; pure black in both themes, so it merges with the notch
    light: "hsl(0 0% 0%)"
    dark: "hsl(0 0% 0%)"
  hud-island-ink: # the captions on the island; one light ink, because the island is black in both themes
    light: "hsl(0 0% 92%)"
    dark: "hsl(0 0% 92%)"
  led-off: # unlit LED segment; one mid grey for both themes, because the desktop tints the translucent HUD frame
    light: "hsl(0 0% 50% / 0.45)"
    dark: "hsl(0 0% 50% / 0.45)"
  led-notch: # the dark line of the linear-use notch; one value for both themes, for the same reason as led-off
    light: "hsl(0 0% 20% / 0.95)"
    dark: "hsl(0 0% 20% / 0.95)"
  led-notch-highlight: # the light line beside it; the pair reads on a light document and on a dark one
    light: "hsl(0 0% 100% / 0.85)"
    dark: "hsl(0 0% 100% / 0.85)"
  # Token-map work modes: one colour per kind of work a turn did. They are
  # separate tokens so the map can re-tune without moving product chrome.
  # None of them is orange, coral, black, or white: orange and coral mean
  # burn, and black and white vanish on the desktop. All seven are bold and
  # fully opaque so a single dot reads on the glass.
  mode-looking: # read + search; mirrors system-blue
    light: "hsl(215 100% 48%)"
    dark: "hsl(215 100% 60%)"
  mode-running: # shell + tests; mirrors system-green
    light: "hsl(140 80% 36%)"
    dark: "hsl(140 90% 50%)"
  mode-changing: # edits; the system purple
    light: "hsl(280 85% 55%)"
    dark: "hsl(280 100% 70%)"
  mode-delegating: # sub-agent spawns and sub-agent turns; mirrors system-indigo
    light: "hsl(245 90% 60%)"
    dark: "hsl(245 100% 72%)"
  mode-thinking: # extended thinking with no tool; mirrors system-gold
    light: "hsl(45 100% 42%)"
    dark: "hsl(52 100% 55%)"
  mode-talking: # plain assistant text; a hot pink, well away from burn orange
    light: "hsl(330 90% 55%)"
    dark: "hsl(330 100% 68%)"
  mode-other: # MCP, skills, web; the system teal
    light: "hsl(192 100% 38%)"
    dark: "hsl(190 100% 55%)"
  # Session-analysis sub-palette only (src/styles/session-analysis-colors.css)
  context-stroke: # the context line; a cool blue, lit at rest
    light: "hsl(221.2 83% 53.3%)"
    dark: "hsl(221 89% 59.8%)"
  context-fill-top: # under the line, in the same blue
    light: "hsl(221.2 83% 53.3% / 0.5)"
    dark: "hsl(221 89% 59.8% / 0.42)"
  context-fill-base:
    light: "hsl(221.2 83% 53.3% / 0.12)"
    dark: "hsl(221 89% 59.8% / 0.1)"
  context-rest-top: # the same fill in grey, while another layer is lit
    light: "hsl(240 5.5% 25% / 0.16)"
    dark: "hsl(240 33% 94% / 0.24)"
  context-rest-base:
    light: "hsl(240 5.5% 25% / 0.03)"
    dark: "hsl(240 33% 94% / 0.05)"
  chart-rest-strong: # resting grey, strongest step; parent output
    light: "hsl(240 5.5% 25% / 0.3)"
    dark: "hsl(240 33% 94% / 0.28)"
  chart-rest: # resting grey, middle step; parent input
    light: "hsl(240 5.5% 25% / 0.18)"
    dark: "hsl(240 33% 94% / 0.16)"
  chart-rest-faint: # resting grey, faintest step; sub-agent tokens
    light: "hsl(240 5.5% 25% / 0.09)"
    dark: "hsl(240 33% 94% / 0.08)"
  chart-rest-fainter: # resting grey, fourth step; the cost burnup chart's cache-write layer
    light: "hsl(240 5.5% 25% / 0.045)"
    dark: "hsl(240 33% 94% / 0.04)"
  chart-rest-mark: # resting grey for a hairline mark over the plot
    light: "hsl(240 5.5% 25% / 0.55)"
    dark: "hsl(240 33% 94% / 0.5)"
  chart-label-pill: # behind a label inside the plot; the opposite of the surface
    light: "hsl(0 0% 100% / 0.5)"
    dark: "hsl(0 0% 0% / 0.5)"
  context-rewrite: # the rewrite bar over the plot; white in both themes
    light: "hsl(0 0% 100%)"
    dark: "hsl(0 0% 100%)"
  token-in: # parent input; cyan
    light: "hsl(189 86% 53.3%)"
    dark: "hsl(188.7 87% 58.2%)"
  token-out: # parent output; violet
    light: "hsl(258.3 89% 66.2%)"
    dark: "hsl(258 89% 69.8%)"
  token-subagent: # quiet label-family neutral
    light: "hsl(240 5.5% 25% / 0.45)"
    dark: "hsl(240 33% 94% / 0.4)"
  cost-cache-read: # cost burnup chart, cache-read layer; muted blue-green
    light: "hsl(165 45% 40%)"
    dark: "hsl(165 50% 62%)"
  cost-cache-write: # cost burnup chart, cache-write layer; muted green
    light: "hsl(140 40% 38%)"
    dark: "hsl(140 45% 64%)"
  mark-rehydration: # a cache rehydration mark, lit; yellow
    light: "hsl(45.5 96% 56.2%)"
    dark: "hsl(45.2 96% 60.1%)"
  mark-routing-miss: # a provider routing-miss mark, lit; pink
    light: "hsl(330.3 81% 60.3%)"
    dark: "hsl(330.6 82% 64.5%)"
  mark-compaction: # a compaction mark, lit; the brand tint in both themes
    light: "hsl(17.6 100% 58.6%)"
    dark: "hsl(17.6 100% 58.6%)"
  measure: # the reading itself: the real-work run and the cost-scale measure; a calm blue
    light: "hsl(191.5 83% 36.8%)"
    dark: "hsl(192 63% 47.6%)"
  share-work: # efficiency composition, real work; teal, for fills
    light: "hsl(173.4 80% 40%)"
    dark: "hsl(173.5 78% 46.4%)"
  share-work-text: # the same teal as ink for words; darker on light
    light: "hsl(174 80% 25.4%)"
    dark: "hsl(173.5 78% 46.4%)"
  share-waste: # efficiency composition, rewrite waste; hot red, for fills
    light: "hsl(347.9 100% 56%)"
    dark: "hsl(348 100% 60.5%)"
  share-waste-text: # the same red as ink for words; darker on light
    light: "hsl(348 100% 34.5%)"
    dark: "hsl(348 100% 60.5%)"
  share-carry: # efficiency composition, carry; the mid neutral
    light: "hsl(240 5% 52%)"
    dark: "hsl(240 6% 62%)"
  waste-warn: # a wasted-token figure that is bad, below the red for a severe one
    light: "hsl(24 100% 45%)"
    dark: "hsl(17.6 100% 58.6%)"
  context-warning:
    light: "hsl(32 95% 43.72%)"
    dark: "hsl(36.4 100% 52%)"
  context-critical:
    light: "hsl(0 72% 50.5%)"
    dark: "hsl(0 90% 70.7%)"
  # Quota sub-palette only (src/views/main-window/quota/quota.css). The five
  # session steps are one lightness ramp of the meter blue, darkest for the top
  # spender. The ramp repeats for additional sessions and is safe for every colour-vision
  # deficiency. Grouped bands ("other", "unattributed") use chart-rest-strong
  # and chart-rest-faint, the same resting greys as the session-analysis charts.
  quota-meter: # the provider's own meter reading; matches the context line's blue
    light: "hsl(221.2 83% 53.3%)"
    dark: "hsl(221 89% 59.8%)"
  quota-session-1: # ramp step 1, the top spender
    light: "hsl(221 75% 45%)"
    dark: "hsl(221 89% 66%)"
  quota-session-2: # ramp step 2
    light: "hsl(221 70% 56%)"
    dark: "hsl(221 75% 56%)"
  quota-session-3: # ramp step 3
    light: "hsl(221 65% 67%)"
    dark: "hsl(221 60% 46%)"
  quota-session-4: # ramp step 4
    light: "hsl(221 60% 77%)"
    dark: "hsl(221 45% 38%)"
  quota-session-5: # ramp step 5, the fifth spender
    light: "hsl(221 54% 86%)"
    dark: "hsl(221 34% 30%)"
  quota-unexplained: # meter spend no local reading explains; hatch stroke, reuses the meter blue at low opacity so it needs no new hue
    light: "hsl(221.2 83% 53.3% / 0.5)"
    dark: "hsl(221 89% 59.8% / 0.5)"
  quota-pace: # even spend through a window: 0% at the start, 100% at the reset
    light: "hsl(240 5.5% 25% / 0.7)"
    dark: "hsl(240 33% 94% / 0.7)"
  quota-reset: # a reset boundary line, lighter than the axis/grid text; also draws the chart's 25/50/75/100% gridlines
    light: "hsl(240 5.5% 25% / 0.15)"
    dark: "hsl(240 33% 94% / 0.32)"
fonts:
  sans: "-apple-system, BlinkMacSystemFont, SF Pro Text, system-ui, sans-serif"
  mono: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" # via `font-mono`
typography:
  display: { fontSize: 40px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0.36px" }
  hero-figure: { fontSize: 32px, fontWeight: 800, lineHeight: 1.4, letterSpacing: "-0.96px" } # a headline number; pair with font-mono
  # class .type-<name> · [fontSize, fontWeight, lineHeight, letterSpacing] · family = fonts.sans
  large-title: { fontSize: 26px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0.36px" }
  title-1: { fontSize: 22px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0.35px" }
  title-2: { fontSize: 17px, fontWeight: 600, lineHeight: 1.4, letterSpacing: "-0.43px" }
  title-3: { fontSize: 15px, fontWeight: 600, lineHeight: 1.4, letterSpacing: "-0.23px" }
  body-large: { fontSize: 13.5px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "-0.15px" } # a list's primary line
  headline: { fontSize: 13px, fontWeight: 600, lineHeight: 1.4, letterSpacing: "-0.08px" }
  body: { fontSize: 13px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "-0.08px" }
  callout: { fontSize: 12px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0" }
  footnote: { fontSize: 11px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0.12px" }
  caption: { fontSize: 11px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0.06px" }
  metadata: { fontSize: 10.5px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0.08px" } # transient metadata in a constrained list row
spacing:
  1: 4px
  2: 8px
  3: 12px
  4: 16px
  5: 20px
  6: 24px
  base: 4px
sizes:
  # Raw CSS geometry vars in src/styles/tokens.css :root. Not @theme-registered, so
  # consume them as arbitrary values, e.g. w-[var(--sidebar-width)].
  --sidebar-width: 220px # source-list / sidebar width for multi-pane windows (36px rows)
  --control-height-regular: 22px # ui-push-button, dropdown triggers, inputs
  --control-height-small: 17px # compact control variant
  # Spacing rhythm vars (multiples of 4); mirrors `spacing` above.
  --space-xs: 4px
  --space-sm: 8px
  --space-md: 12px
  --space-lg: 16px
  --space-xl: 20px
  --space-2xl: 24px # group separation in a settings-style pane
  # Local geometry that stays in its component. The wide Session Detail
  # (src/components/session/SessionDetailPresentation.tsx)
  # lets the context chart fill the tab, with a min-h-48 floor.
rounded:
  small: 4px
  control: 5px
  popover: 10px # outer corner for macOS floating popover and notification surfaces
  full: 9999px
shadow:
  stats-card: "inset 0 0 0 1px var(--color-stats-edge)"
  popover: "0 4px 12px rgb(0 0 0 / 0.15), 0 1px 3px rgb(0 0 0 / 0.08)"
  tooltip: "0 2px 8px rgb(0 0 0 / 0.12), 0 0.5px 2px rgb(0 0 0 / 0.06)"
  raised: "0 1px 2px rgb(0 0 0 / 0.15), 0 0 0 0.5px rgb(0 0 0 / 0.04)" # @theme-registered; consume as the `shadow-raised` utility
motion:
  # Transition tokens. Durations are plain :root vars in src/styles/tokens.css,
  # because Tailwind has no --duration-* theme namespace; consume one as
  # duration-[var(--duration-fast)]. The easing is @theme-registered, so it is
  # the `ease-out-quart` utility. Plain `ease-out` is the default elsewhere.
  --duration-quick: 100ms # a crossfade that leads the movement it accompanies
  --duration-fast: 120ms # the default control, hover, and disclosure transition
  --duration-medium: 180ms # a paired visibility crossfade
  --duration-slow: 300ms # a meter or bar that fills
  --ease-out-quart: cubic-bezier(0.23, 1, 0.32, 1)
  # Recipes, for the timings the tokens above do not carry. Animation timings
  # stay with the keyframes that own them.
  button: "transform 80ms / opacity 120ms ease-out; :active scale(0.98) opacity 0.85"
  menu-in: "120ms ease-out from trigger origin"
  tooltip-in: "100ms"
  switch: "180ms ease-out track + thumb"
  progress-pulse: "1.5s loop"
  segmented-indicator: "120ms ease-out slide; reduced motion swaps to a 60ms per-segment crossfade"
  anchored-content: "100ms opacity-only crossfade after native geometry commits; reduced motion uses 60ms"
  text-roll: "300ms overshoot per character, 45ms stagger; retune with --text-roll-duration / --text-roll-stagger / --text-roll-ease"
  tray-usage-meter: "launch: 1.5s column-by-column depletion; later changes: 300ms column-by-column"
  hud-led-blink: "steps(1) loop; --led-period 300ms to 3s on eight geometric rungs, set per segment from the spend rate; 300ms is the flash-safety cap for a 6px dot"
  led-sweep: "4000ms loop in src/styles/hud.css, the session list's shimmer cycle and its phase: installLivePhase in src/lib/livePhase.ts sets the start time of every live animation from the wall clock, on an animation frame, so a title shimmer and a meter sweep hold the same point of the cycle however late either one starts and whatever a render does; it sets the start time again when an animation starts, when the window comes back, and once each cycle, so a window that stopped painting returns in step; the stylesheets declare no animation-delay, because a delay would move the phase; the sweep is held back 0.2s because a stepped segment snaps on where the soft shimmer fades in; a gleam crosses the lit segments of each live meter in about 2s, rows 100ms apart, and runs the ring's lit arc once; each segment takes one of two brightness levels, off or the peak, instead of a smooth ramp, and the band is about three segments wide; it peaks at 0.56 in the popover; the floating HUD does not sweep, because it shows the spend-rate blink alone; the gleam takes a shade of the segment's own colour, white above a dark segment and dark above a light one; unlit segments do not move; a meter that is scoped to one model sweeps only while a live session runs that model; reduced motion stops the loop and holds the brand tint on the next segment to light"
components:
  button-secondary:
    className: ui-push-button
    backgroundColor: "{colors.surface-secondary}"
    textColor: "{colors.label}"
    borderColor: "{colors.separator}"
    typography: "{typography.callout}"
    rounded: "{rounded.control}"
    padding: "0 10px"
    height: 22px
  button-primary:
    className: "ui-push-button bg-accent-fill text-white border-transparent"
    backgroundColor: "{colors.accent}"
    textColor: "#ffffff"
    typography: "{typography.callout}"
    rounded: "{rounded.control}"
    padding: "0 10px"
    height: 22px
  button-cta:
    backgroundColor: "{colors.accent-hover}"
    textColor: "#ffffff"
    fontWeight: 500
    typography: "{typography.body}"
    rounded: "{rounded.control}"
    padding: "10px 16px"
  input:
    backgroundColor: "{colors.input-fill}"
    textColor: "{colors.label}"
    borderColor: "{colors.separator}"
    typography: "{typography.body}"
    rounded: "{rounded.control}"
    padding: "0 8px"
    height: 22px
  menu:
    className: ui-menu
    backgroundColor: { light: "rgb(235 235 235 / 0.92)", dark: "rgb(50 50 50 / 0.96)" }
    backdropFilter: "blur(20px) saturate(180%)"
    borderColor: "{colors.separator}"
    rounded: "{rounded.control}"
    shadow: "{shadow.popover}"
  tooltip:
    className: ui-tooltip
    backgroundColor: { light: "rgb(235 235 235 / 0.92)", dark: "rgb(50 50 50 / 0.96)" }
    backdropFilter: "blur(20px) saturate(180%)"
    textColor: "{colors.label}"
    rounded: "{rounded.control}"
    padding: 8px
    shadow: "{shadow.tooltip}"
    maxWidth: 250px
  switch:
    className: "ui-switch + ui-switch-thumb"
    size: "32x20px"
    checkedColor: "{colors.accent}"
  radio:
    className: "ui-radio-indicator + ui-radio-dot"
    size: "12px ring / 6px dot"
    checkedColor: "{colors.accent}"
  progress:
    className: "ui-progress + ui-progress-indicator"
    height: 6px
    indicatorColor: "{colors.accent}"
  tab-bar:
    className: "SegmentedControl variant=raised-tabs"
    trackColor: "{colors.surface-secondary}"
    segmentColor: "{colors.input-fill}" # the selected segment, raised on a solid fill
    selectedInk: "{colors.brand}" # the active tab label carries the brand at 80% alpha, platform tab-bar style
    typography: "{typography.callout}" # font-semibold! selected / font-medium! rest
    rounded: "7px track, {rounded.control} segment" # 5px + 2px track padding keeps the curves concentric
    shadow: "{shadow.raised}"
  detail-tabs:
    className: "SegmentedControl variant=native-tabs (ui-segmented-native)"
    trackColor: "{colors.surface-secondary}"
    segmentColor: { light: "{colors.surface}", dark: "{colors.surface-tertiary}" } # the selected segment, raised like the macOS control
    selectedInk: "{colors.label}"
    typography: "{typography.caption}" # font-semibold! selected / font-medium! rest
    rounded: "7px track, {rounded.control} segment"
    shadow: "{shadow.raised}"
    separator: "{colors.separator} hairline between two unselected neighbours"
  list-display-toolbar:
    className: "ListDisplayToolbar + SegmentedControl variant=text-tabs"
    selectedInk: "{colors.accent}"
    indicatorColor: "{colors.label}"
    typography: "{typography.footnote}" # size="regular" (default): 24px row, 12px gap
    height: 32px
    padding: "0 12px"
    motion: "100ms color and underline opacity crossfade; no moving indicator"
    largeSize: 'size="large": {typography.body}, 28px row, 16px gap; used by the Overview usage unit control'
  scroll:
    className: "ui-scrollbar + ui-scrollbar-thumb"
    width: 6px
    topEdgeFade: "ScrollPane topEdgeFade opt-in; activates above scrollTop 1px; alpha 25% at 0px, 75% at 5px, 100% at 12px"
    bottomEdgeFade: "ScrollPane bottomEdgeFade opt-in; activates with more than 1px below the viewport; mirrors the top fade"
---

# antiburn Desktop — Design System

The token reference is the YAML front matter above. Light and Dark live in one file:
every `colors` entry carries both values, and only those values differ between themes.
System follows changes to the OS appearance and uses the same Light or Dark palette,
including its opacity and native system colours. Reduced transparency applies equally
to System and the corresponding explicit theme.
Notes for what isn't expressible as a token:

- **Overview panels** — the usage meters use a card background and inset
  outline. The checks and sessions section fills the remaining column without an
  outer card. Both retain 16px internal padding. A unit control that heads a whole block uses
  the large text-tabs size; a toolbar control uses the regular one. Individual burn findings match
  compact session cards: `bg-session-card`,
  `--radius-popover` corners, 12px horizontal and 8px vertical padding, 6px gaps,
  and `hover:bg-surface-secondary/50` with the shared `session-card` transition.
  Finding rows have no separator lines. “Recent” sits at the left
  of the sessions header, on the same baseline as “All sessions”, with both using
  `type-caption text-label-secondary`. Rows that share columns are one grid with subgrid rows,
  never a stack of flex rows. A list that ranks sessions by a figure draws every figure the
  same: plain text, with no pill, flame, or high-cost mark at any magnitude, because the order
  already says which is large. A missing figure leaves its cell empty rather than showing a
  zero. When the page narrows, a secondary column hides and its grid track goes with it, so
  the remaining columns keep their places; when the page shortens, the list yields rows from
  the end, so the chart above it keeps a usable plot height. Both answer a container query, not
  the window. Every cost figure in the app is an estimate at list rates, never a bill, and its
  tooltip says so in one muted line. The provider limits card is the one side panel: only the
  Overview shows it, every other section keeps its full workspace width. It takes its width
  from the window with a clamp — the page needs the width more at small sizes and less at
  large — keeps the full window height between its margins, and scrolls alone when the accounts
  outgrow it. It uses the popover corner, the opaque `surface-window` fill under the
  `surface-sidebar` tint, and `shadow-raised` over the `shadow-stats-card` outline.

- **Overview cost chart** — show daily estimated cost as stacked areas, one per source
  agent, for the latest 30 days. Keep agent colors stable and name each agent in the
  legend. Do not show the previous period. Leave gaps for unknown costs and mark
  incomplete days; tooltips distinguish unpriced usage from known subtotals.

- **Hero figures** — every row of headline figures in the app is one `HeroFigures`: a
  `type-callout text-label-secondary` label over a `type-hero-figure font-mono` number in the
  `measure` ink, with a `type-caption text-label-tertiary` line under it, cells parted by a
  hairline, stacking when their container narrows. A figure's method goes in its tooltip.
  A new headline number joins this row rather than drawing its own.

- **Limits page** — the scope picker is a pill that floats over the bottom centre of
  the page, in the shape of the session detail's section picker and in its selected
  chip colours for its whole length: `rounded-full`, `bg-selected-fill`,
  `text-selected-ink`, `shadow-raised`, 2px track padding, and `type-callout` labels.
  Account › lane › range are its segments, each a `rounded-full` menu button at least
  24px tall with 12px inline padding, washed with `selected-ink` at 10% on hover and 15%
  while its menu is open; a `›` at 50% opacity sits between segments. The chevron shows
  only on hover, focus, or while the menu is open (`quota-jump-chevron`); a level with
  one choice is plain text. The range menu lists every preset and greys one that ends
  before the lane's first reading at 40% opacity, with "· no readings" after its label.
  The range reads in the lane's own words ("Last 3 weeks" on a weekly lane, "Last 3
  windows" on a 5-hour lane); the "last reading" note at the top right is
  `type-footnote text-label-tertiary`. The chart always spans the whole window, start
  to reset, with the open window's data ending at "now" and the pace line ending at the
  same point; local midnights (or whole hours on a 5-hour lane) label the axis. The
  pace-line toggle is a small `type-footnote` push button under the chart's right edge,
  24px tall on `bg-surface-secondary` in secondary ink with `rounded-control` and 8px
  inline padding, reading "Show pace" or "Hide pace" for what a press does. A range with nothing to show renders a centred `Gauge`
  icon in tertiary ink over a `type-body` title and a `type-callout` caption that says
  when readings began. Session rows follow the session-card recipe (`bg-session-card`,
  `rounded-control`, 12px by 8px padding, 6px gaps, `hover:bg-surface-secondary/50`)
  with the agent icon, a regular-weight `type-body` title, and at the right the colour
  swatch beside the percent, then the dollars in secondary ink. The swatch sits with the
  figure it explains, not at the row's edge, so the row starts with the agent icon like
  a session card; the "other", "unattributed" and "unexplained" rows use `surface-card/50` and
  secondary ink. The list sizes to its rows up to 45% of the column and the chart takes
  the rest (`quota-session-list` in `quota.css`). Sessions above the percentage
  threshold remain individually visible without a count limit. Hover identifies each
  session across the chart and list, including sessions that reuse a shade.

- **Popover spend summary** — one shared `surface-card` card uses `rounded-control`,
  a 12px top inset, 8px side insets, 12px horizontal and 8px vertical internal padding, and three equal columns with 8px gaps.
  The following component owns the gap below the card; the summary adds no bottom padding.
  Every cost uses system sans with `type-title-3 font-semibold!`; each secondary line
  uses `type-footnote text-label-secondary` without an added top gap. `SegmentFigure` supplies
  tabular numerals. The secondary line keeps the token count in `label-secondary`, then
  renders the middle dot and period in `label-tertiary` with 4px side margins around the dot:
  `1.35B · 7 days`. Periods read Today, 7 days, and 30 days,
  while accessible labels retain Last 7 days and Last 30 days.
  Token units remain accessible but visually implicit. If cost is unavailable, the
  token count becomes the primary figure and only the period appears below it.
  The summary has no column dividers, individual card shadows, or entrance animation.
  The shared card uses `shadow-stats-card`: a uniform 1px inset outline that follows
  its rounded corners. `stats-edge` uses pure black at 4% in light mode and pure white
  at 5% in dark mode, so no single edge reads as a divider.
  The collapsed provider row uses a 16px leading inset, 10px top padding, 6px bottom padding,
  and 4px horizontal radial padding, so the first ring aligns with the summary's 20px content
  inset. The expanded row uses 4px bottom padding. The disclosure keeps its visual size and
  has a centered 40px square hit area in both states. A 12px gap separates its slot from the
  provider group.

- **Utilities** — every `colors` key is a Tailwind utility via `bg-/text-/border-<name>` (e.g.
  `bg-surface`, `text-label`, `text-system-green`). Use `bg-accent-fill` for accent backgrounds; the
  live system accent token resolves incorrectly when used as a `background-color`.
- **Floating surfaces** — use the shared `.ui-menu` and `.ui-tooltip` chrome. Feature code must not
  recreate those materials in an independently positioned panel; use an existing primitive or add a
  documented shared component.
- **Type scale** — `.type-*` classes, declared outside any `@layer` in `src/styles/typography.css`.
  Unlayered CSS outranks Tailwind's `utilities` layer, so overriding a baked-in weight needs the
  important modifier: `font-normal!` (as `SectionGroup` does) to soften a heading step, or
  `font-medium!` / `font-semibold!` to make a body step carry a small label or a short alert
  title. Pair the modifier with a `type-*` class; it changes weight only, never size. `italic` is
  the one permitted style variant, for a placeholder sentence in an otherwise empty list.
- **Icons** — `lucide-react`, inherits `currentColor`. `size` 12 (footnote) / 14–16 (default) / 24
  (feature); color with `text-*`; `strokeWidth` 2 (2.5–3 tiny marks, 1.5 large/chart); `shrink-0` in
  flex; decorative → `aria-hidden`.
- **Vendor brand marks** — the one exception to the icon rule, and not interchangeable with it: a
  vendor's mark is its trademark, so its shape and colour are the vendor's to define. Marks are
  filled paths, not stroked glyphs, and take `--color-agent-mark` rather than a `text-*` label
  colour — a deliberately firmer ink, because a shape has no letterforms to carry it at 18px. A mark
  whose identity _is_ its colour keeps that colour in both themes, taken from the value its source
  package records. Marks are never drawn inline; they come from the `renderAgentIcon` slot. The
  same rule covers the one other place a vendor colour appears: the HUD draws Anthropic's usage bar
  in the hex `simple-icons` records for the Claude mark, read from the package rather than written
  into the palette. The bar raises the saturation of that hex and keeps its hue and lightness. A
  published brand value is made for a filled mark at 18px, and a row of 6px dots on an uncontrolled
  desktop needs more chroma to read as the same colour. The lift is a factor applied to the package
  value, so the source stays the package.
  Session cards omit the mark from the model line. Instead, each card places a decorative 40px neutral
  mark at 5% opacity in light mode and 6% in dark mode, cropped 6px beyond the lower-right edge. The
  denser Antigravity mark scales to 90% inside the same box for equal optical weight. The card clips
  overflow, and the mark ignores pointer input and assistive technology. The model tooltip keeps the
  vendor mark and source name for explicit identification. It does not repeat repository metadata,
  which remains in the card's trailing context row.
- **Burn Checks** — `BurnCheckIndicator` owns the feature palette and iconography. Failure uses
  `burn-check-failure-fill` for arcs or marks and `burn-check-failure-text` for wording. Pass arcs
  and terminal ticks use `burn-check-pass-fill`. A passing session-card verdict uses the same cyan
  with semibold sentence-case `X/Y passed` wording, counting assessed checks only. A failing
  verdict uses semibold failure ink with `X/Y failed` wording instead; the two never appear
  together, so the verdict is always a single phrase. Unassessed arcs and
  lifecycle marks use `burn-check-neutral`. A compact session-list verdict uses system monospace with
  `type-footnote tabular-nums text-label-secondary` and aligns its text to the card's 8px content
  gap. The verdict omits unassessed checks entirely, counting them out of both the numerator and the
  denominator. Failure wording uses semibold weight. Compact Lucide indicators use a
  15px visual size. Compact segmented dials remain 14px with a 1.5px optical stroke and 14-degree requested
  gaps. A text-bearing indicator shifts down 1px for optical alignment with the monospace verdict.
  Session-card rows use a 2px interline gap inside unchanged 12px vertical card padding. A zero-failure result with at least one assessed check uses an outlined ring and tick, even
  when some checks are not assessed. Session cards keep the passing verdict visible above the title.
  Failed and non-result states keep their explicit verdict wording. Session cards use
  fill alone in the default state, without inset top or bottom edges; their existing hover, tooltip-open, and selected fills
  remain unchanged. Their interaction transitions only the background fill and opts out of the generic
  role-button scale and opacity feedback, so thin Burn Check icons remain raster-stable. Cards within one date group use a 12px gap; date headings retain their existing
  8px separation from the next group. Hover-only repository and time metadata and the decorative
  vendor watermark form a paired opacity crossfade. Both wait for `--duration-fast`, then transition
  linearly over `--duration-medium`; metadata fades in while the watermark fades out. Pointer exit
  reverses the transition without a delay. The same state persists while a card tooltip is open.
  The Burn Check tooltip uses system sans and groups details as Failed, Passed, then Not assessed.
  Its `type-callout` title uses semibold weight; group labels remain medium.
  It reuses the circular Burn Check alert, pass, and neutral icons in a leading 14px column. The
  Failed heading uses the same tertiary ink as other group labels; failed row text uses
  `burn-check-failure-text`, and the failed icon uses `burn-check-failure-fill`. Not-assessed rows omit their repeated “not assessed” suffix. The group
  heading remains the visible and accessible status, so row icons stay decorative. The tooltip has
  no repeated result summary or “Open the session” footer.
  Session-detail check cards and expandable rows share the tooltip's individual-check marks:
  a 14px `CircleAlert` with a 2.5px stroke and `burn-check-failure-fill` for failures,
  and a 14px `CircleCheck` with a 2px stroke and `burn-check-pass-fill` for passes.
  Failure wording and evidence use `burn-check-failure-text`, including card tooltips
  and expanded guidance. Passing row wording stays neutral. Unassessed checks remain
  omitted from the detail cards and rows.
  The popover summary uses a stable “All burn checks” heading without a dial, reserving circular indicators
  for provider usage and individual sessions. It uses `--space-md` (12px) horizontal padding and a 12px text-to-meter gap.
  The text block uses `--space-sm` (8px) top and bottom padding, matching the spend card.
  Failed and passed counts use `font-mono type-footnote tabular-nums`, matching session verdicts.
  Failed counts use semibold failure ink; passed counts use regular cyan `burn-check-pass-fill`.
  The headline uses system sans with `type-headline font-semibold! text-label`. A regular
  `type-footnote text-label-tertiary` “30 days” label shares its baseline with an 8px gap.
  Counts carry the visible verdict;
  the accessible description and companion retain the full result and evidence status.
  When no counts exist, the second line shows the loading or unavailable status.
  Remaining context stays in system sans. The summary omits visible “Evidence incomplete” wording;
  its accessible description retains the evidence status.
  The summary is the first card in the collapsing overview header, above spend and provider limits.
  Its wrapper uses twelve pixels of top padding and `--space-sm` (8px) horizontal padding.
  The wrapper and inner padding use the same tokens as the spend card, so their text starts on the same line.
  Provider rings retain their existing optical indent.
  A single separator after
  the provider limits is inset twelve pixels from both popover edges. The session toolbar starts eight
  pixels below that rule.
  The session list uses the remaining popover height without a persistent footer.
  A static one-pixel edge sits one pixel inside the card. Its state color starts at 40% opacity and fades
  to 10% at the 45% stop, then into a faint separator at the right. Findings use
  `burn-check-failure-fill`; results with passes and no findings use `burn-check-pass-fill`;
  loading, unavailable, and unassessed states use `burn-check-neutral`. Failure takes precedence over pass.
  The fill stays neutral at 50% of `surface-card`, so the semantic state remains in the edge and content.
  The surface uses `rounded-control` (5px), matching the spend summary card. The twelve-pixel gap between these cards matches the top inset. Hover uses 35% of `surface-hover`, and the active preview uses 40% of
  `surface-selected`, while the keyboard-focus treatment remains unchanged.
  Increased contrast uses a solid edge in the resolved state color.
  The trailing estimate uses four filled Lucide flame silhouettes, masked over a neutral track.
  Each full flame represents 25% token burn and fills from the bottom, in order from left to right.
  The 78 × 18px meter uses local SVG geometry. Values below five percent use a yellow-to-brand
  gradient; higher values use brand-to-red, matching the existing token-burn threshold.
  The exact estimate stays in the accessible summary, meter value, app tooltip, and companion
  panel. The flame tooltip opens on hover or keyboard focus and explains the estimate and scale.
  The summary button opens Burn checks in the main window. The meter is a separate focus target
  beside the button, so inspecting it does not open the main window.
  Its estimate uses semibold `font-mono type-callout tabular-nums` in `burn-check-failure-text`,
  or cyan `burn-check-pass-fill` for zero burn. The explanation uses secondary callout text,
  followed by the scale in tertiary footnote text. Color identifies the result, not the explanatory prose.
  Its target is at least 40px high. Inspecting the flames dismisses the companion, so only one
  explanation opens at a time. Unknown estimates omit the meter. Flames remain static when reports update.
- **Themes** — three sources, in cascade order. The system light/dark preference is the default. A
  platform whose webview exposes live system label/separator/accent tokens picks those up through
  `@supports`, so text and chrome track the OS exactly. A platform without them takes an explicit
  `<html data-theme="light|dark">` palette, which is deliberately more opaque because there is no
  window material behind it. `prefers-reduced-transparency` makes the window and popover surfaces
  solid in every branch.
- **Platforms** — `<html data-platform>` is set once at startup (`src/lib/platform.ts`); the few
  genuinely platform-specific rules key off that attribute rather than branching in TypeScript. Only
  the design foundation is allowed to read it, so a component never asks what platform it is on.
- **Focus** — the focus ring is keyboard-only on every platform: it paints under
  `html[data-keyboard]`, which `src/lib/focusModality.ts` sets on Tab and clears on any pointer
  press. This is deliberate — webviews paint `:focus-visible` for programmatic and
  window-activation focus too, which would put a ring on a window that simply reopened. Buttons use
  the arrow cursor by default. A full-row disclosure can use `cursor-pointer!` as a click affordance.
- **Motion** — `prefers-reduced-motion: reduce` clamps every animation and transition globally
  (`src/styles/motion.css`). A surface that still needs a hint of movement re-states a short
  duration there, with the reason. The segmented control's reduced-motion fill and the anchored
  content presenter's opacity-only handoff crossfade over 60ms instead of swapping instantly. An
  ambient loop stops instead of shortening: no duration makes a loop acceptable, so the
  activity-row title shimmer in `src/styles/session-rows.css` sets `animation: none` and keeps its
  resting meaning — the title paints as plain primary text. The live-session sweep in
  `src/styles/hud.css` stops the same way and holds a steady brand-tint mark on the next segment
  to light: a colour, not movement.
- **State** — style the headless control primitives via `[data-state]` / `[data-highlighted]`, not
  `:hover`.
- **Scroll edges** — use the shared `ScrollPane` `topEdgeFade` prop when scrolling content needs to
  dissolve into a fixed top boundary. It masks only the viewport contents after `scrollTop > 1`;
  opt into `bottomEdgeFade` to mirror that mask while more content remains below. The bottom
  boundary updates on scroll and content or viewport resize. Keep scrollbars outside both masks;
  keep fixed labels outside `ScrollPane`. Do not recreate the effect with an overlay, fill, backdrop
  blur, or feature-specific gradient.
- **Settings type ladder** — one descending step per level, set by the `ui/` primitives rather than
  per-pane: pane title `type-title-2` (`Pane`) → group header `type-title-3 font-normal!`
  (`SectionGroup`) → row label `type-body` (`Row`) → row description
  `type-footnote text-label-secondary`. Only the pane title is semibold; below it size and contrast
  carry the hierarchy. Hand-rolled rows must match `Row`'s label type.
- **Window chrome** — a window that hides its native title bar owns the drag strip and the matching
  top clearance in the webview; a window that keeps native decorations must not reserve that space.
  Keep that decision in the window's own layout, not in the shared primitives.
- **Data views** — a view that shows figures (Session Detail, the Overview) keeps the home
  screen's density of styles. One data size per view: every figure, row, and data label is
  `type-body`; hierarchy comes from ink and weight, and size changes are reserved for a hero
  title or figure, guidance prose (`type-callout`), and footnotes. No heading that restates its
  content, and no caption label over a self-evident value — identification that is genuinely
  needed uses an icon with a tooltip. A figure's method belongs in its tooltip, not beside it: a
  hero figure has room for a name and no room for a method. Every horizontal bar uses the usage
  meter (`SegmentedMeter`) silhouette; judgment is carried by ink, never by a multi-color bar.
- **Chart color** — color only where it means a category: blue for context, the token series
  colors for in/out, `cost-cache-read` and `cost-cache-write` for the cache layers, yellow and
  pink for cache marks, brand orange for a compaction or carry, and the `measure` blue wherever
  the view draws a reading. One meaning per color across a view: when two marks share a color
  they never share a shape, and a legend names each. Layers rest in the `chart-rest` greys and
  take their color when the key or the pointer names them. A chart with several layers on one
  category stays on one hue and separates the layers by weight and opacity, so a further layer
  never needs a second hue to stay legible. Layer styling is Tailwind utilities on the SVG
  elements, not stylesheet rules. `waste-warn` is its own token because `brand` is too dark on
  the light surface to read as orange beside `system-red-text`.
- **Verdict cards** — a card groups a name with its verdict; the verdict is the mark alone, with
  the word kept for a screen reader, and the card itself is the affordance that opens the
  explanation. The tooltip holds the evidence and the advice at every width. A pane of cards is a
  query container, and its cards answer their own pane width, not the window's.
- **Main window** — the retained main window opens at 1100 × 600 logical pixels with a normal
  minimum of 1000 × 560. The initial outer frame uses at most 85% of each usable display dimension,
  including native chrome. A smaller work area takes precedence over the normal minimum. Saved
  user sizes can exceed the initial cap and remain constrained to the usable work area. It paints the opaque
  `surface-window` canvas. A shared titlebar spans the window, 40px high at 100%. macOS reserves 78 native logical pixels
  for native traffic lights, matching the 32px center spacing of adjacent toolbar icons. Native button centers target 20 logical pixels below the
  window top, using AppKit coordinate conversion. Horizontal native positions remain unchanged.
  Resize, display-scale, focus, and fullscreen-exit notifications align the buttons synchronously
  on the native main thread. Creation and reveal also align them. Resize correction does not
  enter the asynchronous dispatch queue. Fullscreen leaves the system-owned controls alone.
  Window destruction removes the native notification observers.
  Windows and Linux replace native decorations with this same toolbar and right-aligned
  Minimize, Maximize/Restore, and Close controls. Caption controls use 44 × 40px targets;
  Close uses `system-red-text` over `surface-selected` on hover. Maximize state follows
  native resize events. Linux adds 4px resize edges and 8px corners, hidden when maximized.
  Windows retains native resize hit testing.
  Action errors appear below the controls on `surface-overlay`.
  Back, Forward, and Search use adjacent 32px-wide, 40px-high desktop targets with
  centered 28px-square hover fills. The toolbar overlays the sidebar's top edge.
  macOS views reach the top window edge without an empty row.
  The top 40px of view content also accepts window dragging and double-click maximize
  on non-interactive content. Buttons, links, inputs, tabs, editable text, scrollbars,
  charts, and explicit `data-no-window-drag` regions keep their own input behavior.
  The overlay passes pointer input through to the view; there is no blocking drag sheet.
  Windows and Linux reserve 40px above the detail pane, Overview, and Limits content for right-side caption controls.
  The sidebar stays visible at 220 CSS pixels when the CSS viewport is at least 720px wide.
  Narrower viewports use a modal navigation drawer; its trigger sits below the shared toolbar.
  Navigation starts below the toolbar
  without a brand header or Search row. Search stays after Forward. Command+K / Control+K
  open the palette. Previously saved collapsed preferences do not change this layout.
  Search opens an immediate, top-centered
  command palette on the opaque `surface-overlay` token, with a standalone 40px-square
  close target, labeled result groups and keyboard focus management.
  The Features, Settings, and Checks groups omit repeated group names beneath results.
  Distinct setting paths remain visible.
  The `command-palette-scrim` gently dims the background with 4% black in light mode
  and 12% black in dark mode. The palette opens immediately without background blur.
  Main navigation uses 28px rows, 2px vertical gaps, 14px icons, and 8px icon-to-label gaps.
  `main-window.css` sets this density over `SidebarNav`'s own 36px rows, 8px gaps, 16px icons,
  and 12px icon gaps, which Settings keeps. A top-level item can nest child rows one level deep,
  right below it in the same tablist. A child row needs no icon of its own and carries
  `data-nested`. In the main window its label starts at the parent label column, 30px from the
  row edge (`--main-window-nav-indent`: 8px padding, the 14px icon, and the 8px gap). A child's
  own hairline separator carries `data-nested` too and keeps that same indent. In `SidebarNav`'s
  default density a child row is 32px tall with a 32px `pl-8` indent and an `ml-8` separator.
  Any row's optional trailing count uses the shared `CountPill`
  (`src/components/ui/CountPill.tsx`): the same 16px-high, borderless, `surface-tertiary/40`,
  tertiary-ink, `font-mono type-metadata tabular-nums` pill documented below for a session
  card's `+N` model count. A row with a count sets its accessible name to its label alone, so the
  count digits stay out of the announced name.
  These local geometry rules use the spacing tokens in `main-window.css`; other source lists
  retain their current density. Overview is the default section; Sessions and Burn checks are
  its peers. Burn checks uses the 14px Lucide `Flame` mark and opens from the checks summary
  in the menu-bar popover. A
  Settings action at the bottom opens the existing Settings window. Command+, (Control+, on Windows
  and Linux) also opens Settings without changing the selected section.
  The sidebar material and divider reach the top window edge behind the toolbar controls.
  The sidebar navigation starts below the shared titlebar. The content region scrolls
  independently of the sidebar controls. A view switch is immediate: the window does not animate navigation. Use the
  documented type scale and keyboard-only focus treatment. Hidden or minimized main windows suspend
  presentation work; blur alone does not suspend it. Native close keeps the existing policy: hide for reuse, or quit on Windows/Linux when the tray icon is disabled.

### Main-window search

The palette opens with Command+K or Control+K, focuses its input, and supports arrows, Enter,
and Escape. It has no entrance, exit, or result motion. Dismissal restores the visible Search
trigger. Successful navigation can focus a destination control. Catalog results use Best match,
Features, Settings, and Checks, limited to five per category. Search is navigation only:
Settings opens its separate window at a stable control ID, and a check opens its disclosure.
Neither path executes an action or changes a setting. Queries stay local and are not persisted.

### Interface scaling

Interface size is a reader preference, not a display-resolution heuristic. The operating
system owns display DPI; the shell applies one native webview zoom factor on top of it.
The supported percentages live in `interface-scale.json`: 90, 100, 110, 125, 150, 175, and 200. The default and Actual Size action use 100%. Do not animate zoom or multiply the
type, spacing, radius, or icon tokens by that factor in CSS.

All app-owned web surfaces follow the same saved preference: main window, Settings,
onboarding, popover, previews, HUD, HUD detail, and nudges. Native menus, traffic lights,
and operating-system notifications retain native sizing. Before revealing a new surface,
the native window owner applies zoom and supplies its geometry. Existing main-window
bounds remain under user control. Preferred utility-window bounds grow with interface
size but fit inside the current monitor's work area.
Settings remains resizable, with a minimum content size of 721 × 480 CSS pixels
multiplied by the interface scale. The extra pixel protects its 720px navigation
breakpoint from native rounding. The minimum updates when scale or monitor changes
and is capped by the available work area after native chrome. Compact Settings
navigation remains a fallback only when the display cannot fit the scaled minimum.
Onboarding also resizes around its current center during live scale changes,
clamps to its current monitor's work area, and preserves its renderer and step.

Layout responds to the resulting **CSS viewport**, not physical pixels, screen labels,
or `devicePixelRatio`. Below 720px, main and Settings navigation use a modal drawer with
Escape dismissal and focus restoration. Arrow keys select tabs without closing the
drawer; activation closes it. Settings rows stack controls at a 360px container width.
Onboarding source columns stack below 720px and retain scrolling. Constrained surfaces
must reflow or scroll; reducing the chosen zoom to fit is not allowed.
Below 720px, Overview stacks usage and provider limits in one full-width column.
Each pane retains its own bounded vertical scroll area; the wide layout stays unchanged.

`styles/interface-scale.css` owns these adaptations. The shell supplies
`--interface-scale` only to preserve native chrome geometry. On macOS,
`--native-titlebar-clearance` is `40px / --interface-scale`, preserving a 40 native
logical pixel band at every preset, including 90%. Settings and onboarding keep
their content below it; the main toolbar shares it outside the traffic-light inset.
The main toolbar reserves 78 native logical pixels horizontally for traffic lights.
Its web controls scale horizontally; hover fills stay inside the fixed native-height band.
Windows and Linux have web-owned titlebars: their 40 CSS pixel height and caption controls
scale with the interface. Compact main navigation reserves the toolbar once above its
trigger, rather than adding a second inset inside the workspace. Drawer sidebars use normal
flow, not the wide layout's absolute positioning. Overview's compact grid has two content
rows, with no obsolete titlebar spacer. Search keeps its input and footer fixed while the
results shrink and scroll inside the viewport-bounded dialog.
Below 720 CSS pixels, Checks stacks its collection and detail in two flexible scroll
regions, so the fixed desktop collection width cannot push controls outside the window.

The Interface size control is a presentational primitive. A Settings-owned search adapter
exposes its stable `interfaceSize` target without changing the value on navigation. Its Settings owner invokes
the dedicated scale command; general settings updates cannot change this preference.
The shell owns serialization, persistence, all-surface propagation, geometry conversion,
and consent-gated change analytics. Reusable native window crates accept values and
geometry only; they must not import app settings, commands, or analytics.

Release validation and its native-platform gates are documented in the
[interface-scale QA runbook](../../docs/runbooks/interface-scale-qa.md). Browser fixtures
exercise layout and interactions; they do not prove native zoom, monitor transitions,
traffic-light clearance, or native preview hit testing.

### Main window collection and detail architecture

The 220px navigation sidebar, 340px collection pane, and flexible detail pane remain visible
when the CSS viewport is at least 900px wide. Below 900px, the collection and detail share
one pane. Activating a row opens detail; Back restores the collection's focus and scroll.
Each pane owns its scroll viewport. Generic pane labels are visually hidden;
the session detail owns its toolbar and scroll area. At the 1000px minimum window width,
the detail retains 440px; at the 1100px default width, it receives 540px.
Selection is immediate, with no navigation animation. The generic collection does not auto-select.
Sessions initially selects the newest active session, or the newest session from today in the
local timezone. Older sessions leave the detail empty. Refreshes preserve the user’s selection;
clearing or deleting a selection does not trigger another automatic selection.
An explicit session target from another window reveals and focuses the compact detail pane.
A newer target request can reopen the same session after Back. The session boundary passes
only a reveal revision to the generic pane; ordinary selection and refresh do not create one.
The default collection uses 40px minimum rows, semantic selected fills, and the shared
keyboard-only focus treatment. Arrow keys, Home, and End select rows; Enter focuses the detail
region. Visited sections retain their state and scroll position while hidden.

`MainWindowLayout` owns chrome and columns. `CollectionDetailPane` owns selection and detail
slots; a custom collection slot owns its own viewport, including any virtualization. These
components do not load feature data or subscribe to domain events. The window boundary observes
the CSS viewport through a ref-counted resize subscription. Sessions supplies the existing virtualized
`SessionList` and shared session detail in embedded mode. Selected session rows use `surface-selected/60` for a softer fill in both themes;
hover and tooltip states retain that fill. This yields a 5.4% black tint in light mode and
an 8.4% white tint in dark mode, without reducing text or badge opacity. Sidebar and generic
collection selections keep their full-strength token;
row density, grouping, badges, and tooltips match the menu-bar list. The 340px collection uses
existing title truncation rather than a responsive layout change. Session cards lead structurally with the
token-burn verdict, while the unique title uses `type-body font-medium! text-label` as the primary row identifier.
Titles stay on one line, truncate at rest, and reveal their overflow at about 45px per second with an
interruptible horizontal transition after a deliberate card hover. Reduced motion keeps the truncated resting
title and disables the reveal.
Routine verdict text uses secondary ink; failure wording retains its status colour. One context row
shows the complete first model in semibold secondary ink, keeps its same-size thinking suffix tertiary, adds a separator-free
`+N` count as a 16px-high, borderless muted pill with `surface-tertiary/40`, tertiary ink, and
`font-mono type-metadata tabular-nums`, and exposes the complete source and model context
in a tooltip. The tooltip leads with the vendor icon and name without a redundant Source label,
groups full model identifiers under one Models label, and gives model values primary contrast.
The first model never truncates. Repository moves to the trailing metadata zone
beside the hover timestamp, appears only while the card is hovered, and exists only when the list spans multiple
repositories or it is the row's only context. The trailing repository does not open a second tooltip. On hover,
repository and timestamp anchor to the card's right edge on the model baseline. An 8px inter-group gap
protects the model cluster, including its `+N` pill, from a long truncated repository. Repository and time form one
`font-mono type-metadata tabular-nums` group with a compact 2px gap around their middle
dot; the timestamp never wraps. When the repository name exceeds
18 monospace characters, the visible timestamp drops “ago”; its accessible label remains complete. The model line does
not reserve inline space for the vendor mark. Group labels use sentence case. A
shared `ListDisplayToolbar` places the pinned activity label and the right-aligned `text-tabs` badge metric control on one row with the labels Cost,
Week %, and 5h %. Its accessible group name replaces redundant visible labels. The selected choice uses accent ink and a
primary-label hairline underline. The control crossfades only color and underline opacity over `--duration-quick`; it never slides a moving indicator.
The detail toolbar shows the session title. Shared Back and Forward own main-window history;
the popover retains its related-session Back behavior. Embedded adjacent-session shortcuts stay inside the detail pane. Hidden panes pause
hygiene reads, relative-time clocks, and active-row motion. The menu-bar list shares this card presentation
while keeping its existing navigation behavior.

Burn checks keeps a screen-reader-only page heading and a compact collection header.
Both panes use `CollectionToolbar`: `pt-2`, `h-8`, `mb-1`, and `px-3` share Tailwind’s
rem-based geometry. At the 13px root these resolve to 6.5px, 26px, 3.25px, and 9.75px.
Burn checks adds no first-card top inset. Labels use `type-caption`. Passed and Snoozed
separators use 8px spacing on each side; collapsed headings have no bottom margin.
Their disclosure controls retain a 40px minimum hit area.
Resource cards use `session-card` fill, `rounded-control`, 16px padding, and 16px separation.
Project context appears once below the title and actions, using the full text-column width.
Keep it on one line, truncating overflow while retaining the inline folder control.
The folder hover panel reveals the full recorded local path and supports open/copy actions
on pointer hover or keyboard focus.
Failed sessions use the shared session cards without a separate heading, count badge, or disclosure.
Show all available cards. Lists longer than five cards scroll within the measured height of the
first five cards. Counts and dates use tabular numerals.
The project row keeps a bare 14px folder icon in a 20px target and a `mt-1` count gap.
Resource titles use a 16px vendor column and an 8px gap. Center each vendor against the
first title line. Metadata, affected counts, and session cards share the title text column.
Leave 8px before the session list.
Resource titles and generic finding copy center their first line within a 40px action row.
The actions align with that first line and wrap without negative vertical offsets.
Every finding explanation appears below the header metrics.
All check actions sit at the header’s right edge. Named resource cards contain evidence only.
Do not repeat explanations or check-level actions in the body.
The collection and detail panes start at the top of the workspace. The collection docks directly to
the sidebar and uses the same `--main-window-collection-width` geometry as Sessions. Its 340px width
does not change by breakpoint. The detail pane remains flexible, and both panes own independent scroll
viewports. Padding belongs inside the panes; no centered report wrapper or outer horizontal
gutter separates the collection from the sidebar. Both surfaces use the same check names, icons, assessed-session
counts, order, token-burn percentages, summaries, and semantic status colors. This parity comes from
shared presentation helpers. Do not copy labels or calculate percentages in either surface. Do not
sum category percentages. Use color only for the compact status icon and metric. Other text and
surfaces stay neutral. The main view shows failed and passed groups. It hides not-assessed rows;
settled historical coverage gaps use a not-assessed count, not a processing state. Category rows use
separate `session-card` rounded controls and accessible selection buttons. The selected detail uses one short, check-specific
finding sentence below the heading. The prompt action sits at the heading’s right edge. Do not show internal target identities,
repeated observations, repeated guidance, or detail refresh and bounded-list notices. Unused MCP servers, skills, and built-in tools show
named resource rows. Show bounded failed-session lists directly. Opening a card selects it in the
standard Sessions collection and detail layout. Returning to Burn checks preserves the check
selection. A single-target `Fix` opens a small modal that shows the effect, scope, and one
current-to-new value. Multiple targets open a chooser grouped by agent and scope. State a shared
disable action once above the list, not in every row, and identify built-in tool choices as optional. Use the standard `PushButton` and the same
custom checkbox treatment as notification milestones. Selection starts empty, and the primary
review action stays visibly disabled until the reader selects a target. The modal traps focus,
focuses Cancel first, and closes from Cancel, Escape, or the backdrop. At narrow widths, summaries, details, and actions stack without horizontal
scrolling. At constrained detail widths, prose and actions wrap without horizontal scrolling;
the collection-and-detail structure remains unchanged. The cold loading state uses one busy region,
one screen-reader status, the compact collection header, three shaped collection-row skeletons, and one detail skeleton.
A selected check uses the same one-region,
one-status rule with a compact body, action, and sample skeleton. It must not announce each skeleton. The quiet
`Your savings` disclosure appears only when at least one supported estimate exists. Place it below
the check groups in the collection viewport. Its neutral vertical list supports any number of
contributing checks and collapses into the total. Show token and dollar savings together only when
they cover the same scope and period.
Clipboard success replaces `Copy fix prompt` with a disabled `Copied` button for three seconds.
Applied fixes replace `Fix` with a disabled `Change applied` button for three seconds. A current
finding then restores the enabled action. Do not add separate success text below the actions.

The unselected Sessions detail uses a centered, quiet empty state: a decorative 24px
`MessagesSquare` icon on a soft circular surface, a `type-title-2` heading, and a short
`type-body` description in secondary text. Keep the heading balanced and the description
pretty-wrapped. Use no entry animation or extra action; selecting a list row is the action.
The main-window collection slot owns this presentation; menu-bar empty states remain unchanged.

#### Session card state treatment

| State                 | Fill                             | Behavior                                                    |
| --------------------- | -------------------------------- | ----------------------------------------------------------- |
| Rest                  | `session-card`                   | Dark mode is quieter; light mode retains the existing fill. |
| Hover or open tooltip | `surface-secondary/50`           | Applies only to an unselected card.                         |
| Selected              | `surface-selected/60`            | Persists through hover and open tooltips.                   |
| Keyboard focus        | Existing focus-visible treatment | Remains independent of the selected fill.                   |

Apply this treatment to main-window session selection. Do not change the shared color
tokens or the menu-bar list's default appearance to achieve it.

- **Wide Session Detail** — the fixed toolbar holds the compact session summary and
  callout-size section picker. The repo uses `font-mono`. The toolbar uses surface at
  80% opacity, 16px backdrop blur, and 120% saturation. Reduced transparency uses a
  solid surface-window with no blur. Rounded controls have no outline; the active
  tab uses surface and the raised shadow. Tab labels use regular weight.
  The section picker and action group are both 32px tall, with 24px inner controls.
  Within the detail, check row names use body-large (13.5px), figures and table rows use
  body (13px), and descriptions use callout (12px). Section headings
  are screen-reader-only; caption is reserved for compact toolbar metadata.
  There are no inherited size overrides. Content has 40px side padding.
  Cost composition closes the Context tab, below the plot and its key.
  The context plot fills available height with a 192px minimum. Both efficiency tracks
  are 24px tall. Scale labels use the primary data size. The chart key fits as many 8rem columns as the pane allows,
  then shares the remaining width evenly. Columns shrink below 8rem when necessary. The composition legend stacks three rows beneath the bar, with
  names on the left and percentages and ratings aligned on the right.
  Cost, Checks, and Efficiency stack vertically. Checks use two equal columns when
  the content reaches 40rem, and one column below that width. The top cost block pairs
  a large-title total with a component table; it stacks below 40rem. The card fills
  the content width, while its table caps at 640px. A shared surface-card/50 background,
  rounded-popover corners, and 16px padding group the total and table. The table
  label column fits its text up to 12rem, with 12px gaps before the numeric columns.
  The gap between the total and the component table is 48px.
  The table columns sit together at the card’s right edge. The whole cost card uses
  font-mono, including the total, captions, row labels, and figures.
  Checks show documentation as callout
  subtext by default, with findings first and the status beside each name.
  Efficiency guidance uses the full content width. Tools and efficiency figures use
  display without a background. Tools uses brand orange; efficiency uses label ink
  for good and ok readings, and brand orange only for bad readings. Tool cells use
  16px padding.
  These rules apply only to `.session-detail-wide`.

  The Context chart updates geometry without animation when its measured size changes.
  New bucket data can still animate together, without the entrance stagger.

  The Cost tab's burnup chart sits between the cost card and the checks, and shares the
  Context chart's minimum height and resize behavior.

  Efficiency sits at the bottom of the Cost pane when content fits, and follows the
  checks in normal scroll order otherwise. The total appears once in the top cost
  block.

### Main Burn Checks layout

The collection and detail panes fill the workspace height at every supported breakpoint.
Use the shared 340px collection width beside the flexible detail pane. Keep the
screen-reader page heading, but omit a full-width overview, aggregate outcome dial,
and summary card. Put the failed-check heading and count badge on the left of the collection header,
with “30 days” on the right. Use secondary `type-callout` for the period.
Use 8px top, 12px horizontal, and 4px bottom padding from the spacing tokens.
Use a 32px toolbar row to match Sessions, with centered text.
Keep the first card’s 8px viewport inset, matching the Sessions list. The first card begins at 52px.
Keep the screen-reader page heading. Do not give the header a card surface.

The header has no assessment info icon, tooltip, or coverage navigation action.

Do not show processing or not-assessed status in the collection header.
Settled reports without coverage gaps need no permanent assessment sentence.
Keep savings, dollar estimates, verification state, retry paths, and report-backed actions.
Verified savings appear below the check groups in the collection viewport.

Use quiet `type-footnote` section headings with counts in muted `surface-card`
circle badges. Badges have a 16px minimum width and height, tabular numerals,
and a radius of half `space-lg`; larger counts can expand horizontally.
Prefix failed checks with `CircleAlert` and passed checks with `CircleCheck`.
Use `burn-check-failure-fill` for failed icons and `burn-check-pass-fill` for passed icons. The passed group retains its disclosure chevron and keyboard behavior.
Prefix Awaiting verification with a static 14px `Hourglass` in `system-orange`.
Keep its count and separator. Omit the collection helper sentence; use the standard
section-heading margin before the cards.
Place a collapsed Snoozed group below Passed checks with a neutral Clock and a current count badge.
Separate adjacent collection groups with a semantic separator aligned to the card edges,
with 16px above and below the line.
Snoozed rows retain their check card and show a 13px Clock in secondary ink beside the
Snoozed-until or Snoozed-forever label. The selected detail heading provides the direct
BellRing Unsnooze action. A newly snoozed row enters once over `--duration-medium`;
reduced motion disables that movement through the shared motion rule.
Category rows use `rounded-popover`, a 10px gap derived from half `space-xl`,
and the Session-card state recipe: `session-card` at rest,
`surface-secondary/50` on unselected hover, and `surface-selected/60` when selected.
The selected fill persists on hover; keyboard focus remains independent. Omit
selected leading stripes and category-row chevrons.

Main-view check category icons use 16px glyphs with a 1.9px stroke inside
28px circles. Keep the existing 32px grid column. The circle diameter uses
`--space-xl` plus `--space-sm`; the icon and loading skeleton share this geometry.
The skeleton uses a neutral circular fill. `BurnCheckCategoryIcon` maps each detector
to a static `text-check-*` utility. Each category keeps its color for failed, passed, and
snoozed results. These decorative icons do not replace the visible check names.

Circle backgrounds mix `currentColor` with transparent using the shared
`--burn-check-category-tint`: 10% in light mode and 14% in dark mode. Define the
percentage in every palette branch with the category colors. Keep the existing
card surface visible through the tint; do not add a white base or tint the card.
The bright light-mode palette is approved for decorative category icons. Fast mode
uses a deeper gold to distinguish its thin gauge strokes from the circle tint.
The main view uses `BookOpen` for skills; other surfaces keep their existing icons.

| Detector ID            | Icon       | Category token       |
| ---------------------- | ---------- | -------------------- |
| `unusedBuiltInTools`   | `Wrench`   | `check-tools`        |
| `unusedMcpServers`     | `Server`   | `check-mcp`          |
| `modelOverthinking`    | `Brain`    | `check-overthinking` |
| `unusedSkills`         | `BookOpen` | `check-skills`       |
| `overpoweredSubagents` | `Bot`      | `check-subagents`    |
| `oldModelUsage`        | `History`  | `check-old-model`    |
| `overuseOfFastMode`    | `Gauge`    | `check-fast-mode`    |
| `cacheChurn`           | `Database` | `check-cache`        |
| `sessionsOverDepth`    | `Layers3`  | `check-depth`        |

Place neutral 40px vendor watermarks at the bottom-right of category cards, behind the text.
Reuse `session-vendor-watermark` opacity (5% light, 6% dark), with 4px separation for distinct marks.
Use the report’s full-cohort finding agents, or clean agents for a passing category.
Normalize the evidence agent `claude` to the presentation identity `claude-code`.
Deduplicate icon identities, omit unknown agents, and never infer vendors from bounded samples.
Titles use medium `type-body`; outcomes use monospaced, tabular `type-footnote`.
Failed counts use semibold failure ink. Passed counts in failed rows use secondary ink.
A passing row can retain cyan. Keep the shared gradient flame and exact burn estimate
below the counts. Zero remains unlit; small positive estimates retain “<1%”.

Select the first failed category initially, or the first passed category when no
failure exists. Preserve selection while its category exists. Keep details mounted
so session lists, prepared prompts, fix dialogs, and transient action state survive
selection and resizing. Up, Down, Home, and End select categories; Enter focuses the detail.

Keep detail headings above their independent scroll viewports. The heading and body
fill the available detail pane width with 24px horizontal padding on each side.
The divider spans the pane. Failed-session lists remain left-aligned and cap at 960px,
shrinking to the available width on smaller windows. Use shared session-card navigation
and an 8px gap between cards within the Burn Checks report.
Use one reading column at every breakpoint: description, actions when needed,
then sample sessions. Do not split guidance and samples into parallel columns.
The header title uses a left-aligned 960px maximum-width container; its divider stays full width.
Place actions beside each finding in the body, at the reading column’s right edge.
For a generic finding, align actions with its description. For named resources, align them with the resource title.
Constrain each complete resource list to 960px, including titles, actions, descriptions, samples, and dividers.
Separate resources with a quiet divider, 24px space above, and 24px below. Do not nest resource cards around sample cards.
Actions wrap at constrained widths. Heading and body start at the same left inset.

On detail panes at least 760px wide, a decorative 280px vector antiburn dot-grid “a”
can occupy the bottom-right corner. Increase it to 360px when the detail pane reaches 1200px.
Inset the artwork 12px from the right and crop only 12px at the bottom, preserving its right-hand stroke.
Use the existing dot layout in `antiburn-mark.svg`, with its viewBox fitted to the visible dots
and a diagonal transparent-to-7.5% label-color gradient.
Show it only when unused space below the content is at least the current mark size.
Observe viewport and content sizes so expanded samples cannot overlap the mark.
Keep it static, hidden from accessibility, and transparent to pointer input.

Place a noninteractive status pill before the shared metrics, below the detail title.
Use the collection's existing state: a snooze takes precedence over awaiting verification.
Snoozed checks show a 12px `Clock` and the existing localized Snoozed-until or
Snoozed-forever label. Awaiting checks show a static 12px `Hourglass` in `system-orange`
and “Awaiting verification”. Other checks show no status pill.
Use `rounded-full`, `surface-card`, secondary `type-footnote` ink, 8px horizontal
padding, 4px vertical padding, and a 4px icon gap. Keep each pill on one line and let
its metadata row wrap. The icons are decorative. Add no hover, press, or loading motion.
Action buttons retain `rounded-control`; status pills do not use action-button styles.

Detail headings repeat the card’s failed/passed counts and available burn and cost
metadata through the same presentation component. They also show “N session(s) affected”
from the report’s per-session finding count.
Named MCP and skill checks append “affected resource” counts, with “shown” for truncated lists.
Lead with affected sessions, then the shared check metadata. Explain the named check once in its header.
Show each resource’s sanitized project name when available. The local folder panel and
its explicit actions use the full recorded project path; other display labels remain sanitized.
Do not infer configuration filenames.
Show its distinct affected-session count from the backend, never the occurrence or sample count.
Keep generic recommendations outside resource rows; retain cost, availability, and verification details.
Show session cards directly without a failed-session heading or disclosure.
Keep lists of five or fewer cards fully visible. Longer lists scroll within the
measured height of their first five cards, including gaps. Recalculate that height
when content or width changes. Preserve keyboard access to the list and its cards.
Use `ScrollPane` for these longer lists, with its shared scrollbar and both edge fades.
Reserve `pr-3` inside the viewport so the scrollbar stays beside the cards, matching the session pane.
Never sum target occurrences or bounded sample counts to derive affected sessions.
Named findings retain provider marks, names, scopes, and resource-header actions.
Put each check description in a full-width row below the header metrics. Actions follow in a
separate row, so they do not reduce the description width. Order check and resource actions as
Snooze or Unsnooze, Fix when available, then Copy fix prompt. Batch selectable prompt targets.
The check-level action can use generic text for those targets, but it returns no prompt without a target.
Add a quiet “Snooze” action for each finding, including single findings.
It opens the shared menu material with one week, one month, and forever choices.
Its face stays transparent. The same `RemindLaterAction` becomes Unsnooze for a snoozed check;
do not add a separate Unsnooze control.
Named-resource actions remain in their resource headers. Preserve their status and copy feedback.
Loaded details use no enclosing card. Loading, retry, empty, and passed states keep
contained cards. Named findings use quiet separators with no additional horizontal inset.

### Burn Checks action buttons

The report-only `burn-check-action` variant in `burn-checks-report.css` styles
copy, fix, retry, and change-selection controls. Its visible face is 28px high,
with a 40px hit area and a 44px hit area for coarse pointers. Use `type-callout`
labels, 12px icons, 10px horizontal face padding, and `rounded-control`. The
resting face uses a borderless `surface-card` face and `label`. Hover uses neutral
`surface-hover`; press uses `surface-selected`. Do not use accent or brand color
for the resting or hover face. Keep the shared keyboard focus ring. Disabled
and completed states stay neutral. Success icons use `token-in`.

Use natural-width controls in check and named-target details. Keep the copied
state in the same slot.

Failed sessions use the full shared `SessionRow` cards: `session-card` fill,
popover corners, real Burn Check status, title, available model and cost data,
and the neutral vendor watermark. Add a visible source-agent label beside the
model on this surface; wrap the metadata when space is narrow. Retain the shared
hover and focus treatments without scaling or extra entrance motion. Respect
snoozed detectors as the session lists do. Show all available failed sessions from
the bounded backend result in one mixed-agent list, newest first. Deduplicate
sessions by their full source identity. Each card's status describes all checks
assessed for that session.
Empty available samples use neutral explanatory text.

The menu-bar Burn Checks summary uses `surface-card/50` at rest and
`surface-secondary/70` on hover or focus within, with a `duration-fast` colour
transition. Its summary button uses a pointer cursor. Hover does not open the
checks companion; clicking opens Burn Checks in the main window.

The shell shares the top 40px with view content beside the fixed sidebar. Non-interactive
view content accepts dragging there; controls keep their own interactions. Platform caption controls reserve their necessary clearance.
Their info and action buttons retain their normal interactions.

### Floating HUD frame

The HUD paints a 70% frame, white in light and black in dark (`hud-frame`, as `bg-hud-frame`), inside a one-pixel
vertical gradient stroke (`hud-stroke-top` to `hud-stroke-bottom`, drawn by
`.hud-frame::before`), the same at rest and under the pointer. The content sits 10px inside the
stroke on every side, and the window keeps an 8px transparent margin at the
sides. The frame is white in light and
black in dark, so it reads as the system's own
material on either desktop. LEDs and token-map dots
sit on it without rings or shadows; the frame is what holds them apart from
the desktop. The token map draws in HUD pixels on the LED grid, so a dot is
the size of an LED and a sub-agent dot is smaller.

The live LED blinks at the spend rate (`hud-led-blink` under `motion`), and
its lit half takes `mode-<mode>` of the session with the newest turn, so one
animated dot says both how fast the machine spends and what it is doing.
Reduced motion stops the loop; the detail window states the rate in words.

### Notch island

The native HUD owner reports a revisioned layout in native logical points and
its applied WebView scale. The renderer converts the hardware-bound header to
CSS pixels once. The notch gap, header height and fillets stay fixed on
the display. Each wing starts at 30 native points at 100% and grows with interface
size, capped equally by the available space on both sides of the camera gap.
When expanded, the header fills the native-resolved body width. Its wings extend
to the body's side edges, inside the fixed fillet gutters. They can differ in
width when the body is clamped to a display edge; the camera gap stays anchored.
Body padding still uses half the compact wing width, not the expanded wings.
Header marks scale uniformly, with their height capped to leave `space-xs` above
and below them. Their shared size retains the existing 2.5-spacing-unit mark,
1.4-times-wide and 0.4-times-high live LED; no horizontal stretching applies.
The expanded body uses its own
native-resolved width, grows with interface size below the header, and stays
inside the notch display's usable bounds. Long body content scrolls below the
fixed header. The transparent side gutters and header offset come from the same
native layout, not a second renderer placement calculation. Drag preview retains
the floating frame. Scale requests during a drag apply to the HUD after the drop;
other app windows apply the saved preference immediately.

On a display with a notch, the HUD can sit in it. The island is pure black
(`hud-island`, as `bg-hud-island`) in both themes, so it merges with the
notch, and its captions take one light ink (`hud-island-ink`). Monochrome usage
LEDs use that same ink on the island so they stay visible in both themes;
provider accents and floating HUD colors stay unchanged. Collapsed, it
is the notch row alone: a scale-aware wing either side of the notch, with its
bottom corners at a 14px radius and 6px fillets curving out into the bezel
(`.hud-island`, `.hud-island-fillets`). Expanded, the HUD content hangs
below the row, the corners open to 24px and the fillets to 19px
(`.hud-island-open`), over `duration-fast`. The window is the notch plus the
wings plus a transparent gutter either side for the fillets. The left wing
holds the live LED, blinking at the spend rate as the floating frame's LED
does, and the right wing shows the spend rate as a four-character figure
(`$12`, `$1.2`, `$.05`) in `type-footnote`, or the top bar's LED when the
rate is below half a cent a minute. The bars do not blink on the island; the
wing LED is the one animated dot. The token map and bars keep their 20
columns and spread over the island's width.

### Project folder actions

Session and Burn Check project rows have a bare 14px folder icon in a 20px target. The folder
panel opens after 300ms of pointer hover or immediately on keyboard focus.
A 200ms leave delay lets the pointer cross the 8px gap into the panel. It stays
open while the pointer or focus is inside. Escape, outside press, window blur,
viewport resize, and surrounding scroll dismiss it. Hover never moves focus.

The shared `.ui-menu` surface uses an opaque `surface-window`, `shadow-popover`,
and `rounded-popover`, with no entry animation. The body portal uses layer 100
and favors the detail pane when its 390px maximum width fits. It clamps to the
window with 8px clearance and flips above the trigger when needed. Content size
changes update its placement, including inline errors. The panel
scrolls internally when its contents exceed the available window height.

The header holds the Project folder label and bare open-folder and copy icons.
Both use 14px glyphs, 20px targets, 8px separation, and color-only hover feedback.
These compact desktop targets follow the approved bare-icon design. Keep button
semantics and keyboard focus indicators without visible button chrome or press
scaling. Action tooltips name the host file manager and Copy path. Copy success
replaces its icon with a check for two seconds and announces the result. Errors
stay inline. The selectable monospace path prefers directory-boundary wraps and
gives the final directory primary ink. No path, folder name, or error text enters
analytics. Unknown project paths hide the control; deleted paths remain copyable.
