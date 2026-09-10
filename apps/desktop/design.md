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
  led-sweep: "4s loop in src/styles/hud.css; a gleam crosses the lit segments of each live meter in about 1.25s, rows 100ms apart, and runs the ring's lit arc once; unlit segments do not move; reduced motion stops the loop and holds the brand tint on the next segment to light"
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
    rounded: 3px
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
  scroll:
    className: "ui-scrollbar + ui-scrollbar-thumb"
    width: 6px
    topEdgeFade: "ScrollPane topEdgeFade opt-in; activates above scrollTop 1px; alpha 25% at 0px, 75% at 5px, 100% at 12px"
---

# antiburn Desktop — Design System

The token reference is the YAML front matter above. Light and Dark live in one file:
every `colors` entry carries both values, and only those values differ between themes.
Notes for what isn't expressible as a token:

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
  These local geometry rules use the spacing tokens in `main-window.css`; other source lists
  retain their current density. Sessions is the main sidebar section. A Settings action at the bottom opens the existing Settings window. Command+, (Control+, on Windows and Linux) also opens Settings without changing the selected section.
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
existing title truncation rather than a responsive layout change. The detail toolbar shows the
session title; Back appears only for related-session history. Embedded shortcuts stay inside
the detail pane. Hidden panes pause hygiene reads, relative-time clocks, and active-row motion.
The menu-bar list keeps its existing navigation and presentation defaults.

The unselected Sessions detail uses a centered, quiet empty state: a decorative 24px
`MessagesSquare` icon on a soft circular surface, a `type-title-2` heading, and a short
`type-body` description in secondary text. Keep the heading balanced and the description
pretty-wrapped. Use no entry animation or extra action; selecting a list row is the action.
The main-window collection slot owns this presentation; menu-bar empty states remain unchanged.

#### Session card state treatment

| State                 | Fill                             | Behavior                                                        |
| --------------------- | -------------------------------- | --------------------------------------------------------------- |
| Rest                  | `surface-card/50`                | Quiet background; text and badges retain their normal contrast. |
| Hover or open tooltip | `surface-secondary/50`           | Applies only to an unselected card.                             |
| Selected              | `surface-selected/60`            | Persists through hover and open tooltips.                       |
| Keyboard focus        | Existing focus-visible treatment | Remains independent of the selected fill.                       |

Apply this treatment to main-window session selection. Do not change the shared color
tokens or the menu-bar list's default appearance to achieve it.

- **Wide Session Detail** — the fixed toolbar holds the compact session summary and
  callout-size section picker. The repo uses `font-mono`. The toolbar uses surface at
  80% opacity, 16px backdrop blur, and 120% saturation. Reduced transparency uses a
  solid surface-window with no blur. Rounded controls have no outline; the active
  tab uses surface and the raised shadow. Tab labels use regular weight.
  The section picker and action group are both 32px tall, with 24px inner controls.
  Typography matches the session list: row names use body-large (13.5px), figures and
  table rows use body (13px), and descriptions use callout (12px). Section headings
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
