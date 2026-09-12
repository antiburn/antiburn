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
  - src/styles/session-analysis-colors.css
  - src/styles/session-rows.css
  - src/styles/session-detail.css
  - src/components/ui/text-roll.css
  - src/components/burn-checks/burn-check-summary.css
colors:
  # Concrete token colors use modern HSL function syntax.
  # Use the shortest value that keeps the same 8-bit RGB channels.
  # Hue uses at most one decimal. Saturation and lightness use at most two decimals.
  # Alpha uses at most three decimals. Remove trailing zeros. Achromatic colors use hue 0.
  # name → Tailwind utility via bg-/text-/border-<name>
  # Both values are the explicit [data-theme="light"|"dark"] palettes.
  # A `# @media <theme>: <value>` note states the system-preference value where it
  # differs. The drift check reads those notes: a difference it cannot find a note
  # for is a failure, and so is a note that no longer differs.
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
  surface-header: # the quiet band at the head of the menu-bar popover; fainter than a card
    light: "hsl(0 0% 0% / 0.025)"
    dark: "hsl(0 0% 100% / 0.03)"
  surface-hover: # stays clear of surface-selected, so a hover never reads as a selection
    light: "hsl(0 0% 0% / 0.04)"
    dark: "hsl(0 0% 100% / 0.04)" # @media dark: hsl(0 0% 100% / 0.07)
  surface-window: # standard decorated window
    light: "hsl(0 0% 96.4%)" # @media light: hsl(0 0% 96.4% / 0.8)
    dark: "hsl(0 0% 12.5%)" # @media dark: hsl(0 0% 15.6% / 0.8)
  surface-sidebar: # source-list / sidebar material
    light: "hsl(0 0% 0% / 0.03)"
    dark: "hsl(0 0% 100% / 0.04)"
  surface-selected: # selected row in a list or source list (accent-fill stays for controls)
    light: "hsl(0 0% 0% / 0.09)"
    dark: "hsl(0 0% 100% / 0.14)"
  input-fill:
    light: "hsl(0 0% 100%)" # @media light: hsl(0 0% 100% / 0.5)
    dark: "hsl(240 1.6% 23%)" # @media dark: hsl(0 0% 100% / 0.08)
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
    dark: "hsl(17.6 100% 58.6%)"
  brand-tint: # antiburn orange for large fills
    light: "hsl(17.6 100% 58.6%)"
    dark: "hsl(17.6 100% 58.6%)"
  brand-unlit: # an unlit meter segment: the brand tint, a quarter less saturated
    light: "hsl(17.7 75% 58.6%)"
    dark: "hsl(17.7 75% 58.6%)"
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
    light: "hsl(17.6 100% 58.6%)"
    dark: "hsl(17.6 100% 58.6%)"
  burn-check-failure-text: # failure wording on session and summary surfaces
    light: "hsl(18 100% 36.4%)"
    dark: "hsl(18 100% 68%)"
  burn-check-pass-fill: # pass arcs and terminal marks
    light: "hsl(191.5 83% 36.8%)"
    dark: "hsl(192 63% 47.6%)"
  burn-check-neutral: # unassessed arcs and neutral lifecycle marks
    light: "hsl(209 6% 73.7%)"
    dark: "hsl(210 3% 50.5%)"
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
  hud-control-ink: # the close control's glyph; no alpha, so it stays solid on the hover surface
    light: "hsl(0 0% 32%)"
    dark: "hsl(0 0% 78%)"
  hud-control-edge: # the close control's edge; no alpha, for the same reason
    light: "hsl(0 0% 80%)"
    dark: "hsl(0 0% 32%)"
  led-off: # unlit LED segment; one mid grey for both themes, because the HUD paints no surface and floats over any background
    light: "hsl(0 0% 50% / 0.45)"
    dark: "hsl(0 0% 50% / 0.45)"
  led-notch: # the dark line of the linear-use notch; one value for both themes, for the same reason as led-off
    light: "hsl(0 0% 20% / 0.95)"
    dark: "hsl(0 0% 20% / 0.95)"
  led-notch-highlight: # the light line beside it; the pair reads on a light document and on a dark one
    light: "hsl(0 0% 100% / 0.85)"
    dark: "hsl(0 0% 100% / 0.85)"
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
fonts:
  sans: "-apple-system, BlinkMacSystemFont, SF Pro Text, system-ui, sans-serif"
  mono: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" # via `font-mono`
typography:
  display: { fontSize: 40px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0.36px" }
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
  led-sweep: "4000ms loop in src/styles/hud.css, the session list's shimmer cycle and its phase: installLivePhase in src/lib/livePhase.ts sets the start time of every live animation from the wall clock, on an animation frame, so a title shimmer and a meter sweep hold the same point of the cycle however late either one starts and whatever a render does; it sets the start time again when an animation starts, when the window comes back, and once each cycle, so a window that stopped painting returns in step; the stylesheets declare no animation-delay, because a delay would move the phase; the sweep is held back 0.2s because a stepped segment snaps on where the soft shimmer fades in; a gleam crosses the lit segments of each live meter in about 2s, rows 100ms apart, and runs the ring's lit arc once; each segment takes one of two brightness levels, off or the peak, instead of a smooth ramp, and the band is about three segments wide; it peaks at 0.56 in the popover and 0.245 on the floating HUD; the gleam takes a shade of the segment's own colour, white above a dark segment and dark above a light one; unlit segments do not move; a meter that is scoped to one model sweeps only while a live session runs that model; reduced motion stops the loop and holds the brand tint on the next segment to light"
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
    typography: "{typography.footnote}"
    height: 32px
    padding: "0 12px"
    motion: "100ms color and underline opacity crossfade; no moving indicator"
  scroll:
    className: "ui-scrollbar + ui-scrollbar-thumb"
    width: 6px
    topEdgeFade: "ScrollPane topEdgeFade opt-in; activates above scrollTop 1px; alpha 25% at 0px, 75% at 5px, 100% at 12px"
---

# antiburn Desktop — Design System

The token reference is the YAML front matter above. Light and Dark live in one file:
every `colors` entry carries both values, and only those values differ between themes.
Notes for what isn't expressible as a token:

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
  with semibold sentence-case `All X passed` wording. Passed counts in mixed results use the same cyan at regular
  weight, so the color maps to the dial segment while failure retains weight priority. Unassessed arcs and
  lifecycle marks use `burn-check-neutral`. A compact session-list verdict uses system monospace with
  `type-footnote tabular-nums text-label-secondary`, aligns its text to the card's 8px content
  gap, and renders each outcome and middle dot as separate elements. The dot uses 2px CSS margins on
  each side instead of monospace space characters. The verdict omits unassessed counts and the repeated “Burn Checks” noun
  from assessed counts. Failure wording uses semibold weight. Compact Lucide indicators use a
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
- **Session Detail style rules** — the detail view matches the home screen's density of styles.
  One data size per tab: every figure, row, and data label is `type-body`; hierarchy comes from ink
  and weight, and size changes are reserved for the hero title (`type-title-3`), guidance prose
  (`type-callout`), footnotes, and the wide Cost card's component table (`type-callout`, so it
  sits beside the total at the minimum window width). No heading that restates its content, and no caption label over
  a self-evident value — identification that is genuinely needed uses an icon with a tooltip, the
  session-row fork-glyph pattern. Every horizontal bar uses the usage meter (`SegmentedMeter`)
  silhouette; judgment is carried by the band word's ink, never by a multi-color bar. Color only
  where it means a category: blue for context, the token series colors for in/out, yellow and
  pink for cache marks, brand orange for a compaction, and the `measure` blue wherever the view
  draws a reading — the real-work run of the efficiency composition and the measure on the cost
  scale. In that composition rewrite waste takes the mid neutral and carry takes the brand
  orange, so the same orange means a compaction in the chart and carry in the bar below it; the
  two never share a shape, and the legend beside each names it. The Tools tab reports its wasted
  tokens in `waste-warn`, and in `system-red-text` when the waste is both a large share of the
  startup context and large in absolute terms. `waste-warn` is its own token because `brand` is
  too dark on the light surface to read as orange beside that red. Everything else stays greyscale
  until the pointer names a layer. The wide Cost tab is a query
  container, and its burn checks answer their own pane width. Each check is a card, which is what
  groups its name with its verdict; the verdict is the mark alone, with the word kept for a screen
  reader, and the card itself is the affordance that opens the explanation. Two cards to a row, and
  three from 48rem where each card also shows its summary sentence. The tooltip holds the evidence
  and the advice at every width.
- **Main window** — the retained main window opens at 1100 × 600 logical pixels with a normal
  minimum of 1000 × 560. The initial outer frame uses at most 85% of each usable display dimension,
  including native chrome. A smaller work area takes precedence over the normal minimum. Saved
  user sizes can exceed the initial cap and remain constrained to the usable work area. It paints the opaque
  `surface-window` canvas. On macOS its 40px overlay drag region spans only the sidebar and leaves the
  native traffic lights visible. The collection and detail panes start at the top of the window,
  without titlebar clearance. On macOS the list header and detail toolbar supply drag regions;
  their controls remain interactive. The empty detail uses a 40px drag region without layout clearance. Double-clicking this strip toggles maximize and restore through
  Tauri's drag-region handler. Windows and Linux retain their native bars, so this surface adds no
  top strip there. Multi-pane content keeps the documented 220px sidebar visible at every size.
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
  retain their current density. Burn checks and Sessions are the main sidebar sections. Burn checks
  is the default section, uses the 14px Lucide `Flame` mark, and opens from the checks summary in
  the menu-bar popover. A
  Settings action at the bottom opens the existing Settings window. Command+, (Control+, on Windows
  and Linux) also opens Settings without changing the selected section.
  The first sidebar row starts at 48px on macOS, clear of the drag strip. The content region scrolls
  independently of the title strip. A view switch is immediate: the window does not animate navigation. Use the
  documented type scale and keyboard-only focus treatment. Hidden or minimized main windows suspend
  presentation work; blur alone does not suspend it. Native close hides this renderer for reuse.

### Main window collection and detail architecture

The 220px navigation sidebar, 340px collection pane, and flexible detail pane remain visible
at every supported window size. Each pane owns its scroll viewport. Generic pane labels are visually hidden;
the session detail owns its toolbar and scroll area. At the 1000px minimum window width,
the detail retains 440px; at the 1100px default width, it receives 540px.
Selection is immediate, with no navigation animation. The generic collection does not auto-select.
Sessions initially selects the newest active session, or the newest session from today in the
local timezone. Older sessions leave the detail empty. Refreshes preserve the user’s selection;
clearing or deleting a selection does not trigger another automatic selection.
The default collection uses 40px minimum rows, semantic selected fills, and the shared
keyboard-only focus treatment. Arrow keys, Home, and End select rows; Enter focuses the detail
region. Visited sections retain their state and scroll position while hidden.

`MainWindowLayout` owns chrome and columns. `CollectionDetailPane` owns selection and detail
slots; a custom collection slot owns its own viewport, including any virtualization. These
components do not load data or subscribe to events. Sessions supplies the existing virtualized
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
The detail toolbar shows the session title; Back appears
only for related-session history. Embedded shortcuts stay inside the detail pane. Hidden panes pause
hygiene reads, relative-time clocks, and active-row motion. The menu-bar list shares this card presentation
while keeping its existing navigation behavior.

Burn checks uses the workspace as one flexible pane instead of adding a collection pane. It has no
visual page header. The scroll viewport starts with the report summary and keeps a screen-reader-only
page heading. The viewport uses the shared top-edge fade and centers a single-column overview. The summary and grouped check
rows extend the anchored preview. Both surfaces use the same check names, icons, assessed-session
counts, order, token-burn percentages, summaries, and semantic status colors. This parity comes from
shared presentation helpers. Do not copy labels or calculate percentages in either surface. Do not
sum category percentages. Use color only for the compact status icon and metric. Other text and
surfaces stay neutral. The main view shows failed and passed groups. It hides not-assessed rows;
the summary never presents incomplete historical evidence as a pending product state. Groups use `surface-card/50`, a subtle `border-separator/40` outline, `rounded-control`,
internal row separators, and accessible disclosure buttons. Expanded failures use one short, check-specific
finding sentence, followed by the available actions. Do not show internal target identities,
repeated observations, repeated guidance, or detail refresh and bounded-list notices. Only unused MCP servers and unused skills show
named resource rows. A separate nested disclosure lists bounded sample sessions. Opening a sample selects it in the
standard Sessions collection and detail layout. Returning to Burn checks preserves the check and
sample disclosure state. `Fix` opens a small modal that shows the effect, scope, and one
current-to-new value. The modal traps focus, focuses Cancel first, and closes from Cancel, Escape,
or the backdrop. At narrow widths, summaries, details, and actions stack without horizontal
scrolling. The cold loading state uses one busy region, one screen-reader status, an uncontained summary,
a group label, and three shaped row skeletons. An expanded check uses the same one-region,
one-status rule with a compact body, action, and sample skeleton. It must not announce each skeleton. The quiet
`Your savings` disclosure appears only when at least one supported estimate exists. Place it below
the report summary and before failed checks. Its neutral vertical list supports any number of
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

  Efficiency sits at the bottom of the Cost pane when content fits, and follows the
  checks in normal scroll order otherwise. The total appears once in the top cost
  block.

### Main Burn Checks cosmetic polish

The main report keeps Zack’s failed/passed grouping, ordering, disclosure states,
target loading, sample navigation, and actions. The summary has no card. Use
Marty’s `SegmentedRadialDial` at 88px diameter with an 8px stroke and 100% opacity. Use an
explicit 88px grid column, a flexible text column, and a 24px gap. The ring
has no center icon. Put a grey 12px `Flame` before the `type-callout`
“Estimated burn” label, matching its height. The dial uses `brand-tint` for
avoidable usage and `measure` darker cyan for the remainder. Positive burn has a
4px minimum arc length at the stroke centerline, about 1.59% of this ring,
so tiny issues remain visible. Larger values retain their actual proportions.
The remainder uses the display share so the ring totals 100%. Zero burn has no
orange arc, and unknown estimates show a neutral ring. Keep zero gaps and flat
endpoints. The visible text and accessible name retain the exact supplied value
or its existing display formatting; the arc can overstate values below the floor.
This display floor belongs only to the hero call site. Other uses retain the
shared component’s existing proportions and rounded endpoints.

Use neutral `type-large-title` with `font-semibold!` for the complete percentage. Write “Less than 1%”
for a positive estimate below 1%; do not add decorative decimals. Use `type-body`
for “Of assessed usage could be avoided.” and `type-callout` for the check count,
with no extra paragraph margins. The text column has an 88px minimum height and
distributes its lines to align with the circle. It can grow for wrapped text or
processing status. The failed count has a small `share-waste-text` dot; its words
stay neutral. Use the documented line heights and 32px vertical hero padding.
Use explicit 88px wrapper geometry to match the SVG; rem-based spacing utilities
do not match it with the app’s 13px root font.
The page fills the workspace with 32px horizontal padding and no centered
maximum-width column. The cold skeleton follows this hierarchy.

Use `type-title-2` group headings with 32px space above and 12px below. Check
rows use 16px horizontal and 12px vertical padding, `type-title-3` titles,
`type-body` summaries, and `font-mono` percentage figures. Metric qualifiers and
“burn” labels use neutral sans-serif text, with 6px gaps between the pieces. Omit “token” from the main
view’s displayed metrics; shared percentage calculation and popover copy stay
unchanged. Check category icons are bare 15px glyphs, with no tinted container. Use
`label-secondary` for all category icons, including passed checks. Keep their
existing grid alignment. Failed-session counts use `share-waste-text`; other
values, savings, and status text below the hero use neutral label colours. Provider logos retain their
brand colours. Action-success glyphs use `token-in` cyan.

Parent check groups use `surface-card/50` with a subtle `border-separator/40`
outline. Expanded problems and named targets use borderless `surface-card/75`
for slightly stronger grouping. Savings retains borderless `surface-card/50`.
Expanded problems use
`rounded-control` and 16px padding. Keep internal row dividers. Named MCP and skill targets form a responsive grid with a local
18rem minimum card width, 12px gaps, and 16px outer padding. Cards stack when
space is narrow. Long resource names wrap. Regular check details keep one card.
Every action and sample disclosure stays inside its original problem. The hero
follows Keith’s sketch, using the dial from Marty’s `feat/desktop-pr4-burn-check-design`
branch at `9cb51e4f`. The approved 02D refinement moves the grey flame beside the label.

### Burn Checks action buttons

The opt-in `burn-check-action` variant in `main-window.css` styles the existing
copy, fix, and change-selection buttons. Keep `ui-push-button` and its standard
22px control height, 10px horizontal padding, and `rounded-control`. Use regular
`type-callout` labels, 12px icons, and a 4px gap, matching `PushButton`. The resting surface uses `surface-window`, `separator`,
and `label`. Enabled hover uses solid `brand` fill and border with
`selected-ink` text and `shadow-raised`. Press mixes 10% `label` into the
brand fill and removes the shadow. Transitions use `duration-fast`, with `duration-quick`
for press movement. The shared keyboard focus ring stays visible. Disabled and
completed states keep the neutral surface and do not lift or change on hover.
Success icons use `token-in` cyan. No other buttons use this variant.
In named target cards, Copy fix prompt fills the available width up to 24rem
and is horizontally centred. In wide check-level detail panels, actions use
their natural width and align left beneath the description. Keep the copied
state in the same slot. Sample-session disclosure labels use semibold callout
text and the count first, such as “3 Sample sessions”, with no chevron. Hover
uses `surface-secondary/50` and `label` text with the standard fast transition. Preserve their
expanded state, keyboard interaction, and accessible disclosure attributes.

The menu-bar Burn Checks summary uses `surface-card/50` at rest and
`surface-secondary/70` on hover or focus within, with a `duration-fast` colour
transition. Its summary button uses a pointer cursor. Hover does not open the
checks companion; clicking opens Burn Checks in the main window.

On macOS, Burn Checks reserves a fixed 40px drag strip above its scroll area,
using `--main-window-titlebar-height` and `data-tauri-drag-region`. The strip
remains available in loading and error states. Sidebar dragging remains
available; report controls scroll below the strip and stay interactive.
Windows and Linux use their native title bars without this added strip.

### Floating HUD LED rings

The HUD window paints no surface at rest, so its LEDs sit directly on the
desktop. The desktop can be any colour, and it can match a lit segment and hide
it. Each lit segment therefore takes a 1px ring at 75% alpha. A ring holds a 6px
dot better than a blurred shadow, which only softens the edge at that size.
Unlit segments take no ring, so they stay quiet.

A coloured LED rings in its own colour, darkened to 70% in oklab, so the ring
reads as the edge of the LED rather than as a second mark. An LED that takes
`label` has no colour of its own: it is near-black in the light theme and
near-white in the dark theme, so it rings in the opposite tone, white on light
and black on dark. `OverlayWindow` chooses between the two with the
`hud-leds-color` and `hud-leds-label` classes and passes the row's colour in
`--hud-led-color`.

Inside the HUD, unlit segments raise `led-off` to full opacity. The token keeps
its 45% alpha everywhere else, because the popover and the detail card paint
their own surfaces to hold it. The HUD has none. These ring and opacity values
are local to `src/styles/hud.css` and are not palette or shadow tokens.
