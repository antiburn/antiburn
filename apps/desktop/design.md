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
  - src/styles/session-analysis-colors.css
  - src/styles/session-rows.css
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
  brand: # antiburn orange for text and small glyphs
    light: "hsl(18 92% 39%)"
    dark: "hsl(17.6 100% 58.6%)"
  brand-tint: # antiburn orange for large fills
    light: "hsl(17.6 100% 58.6%)"
    dark: "hsl(17.6 100% 58.6%)"
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
  system-red:
    light: "hsl(354 100% 42.1%)"
    dark: "hsl(3 100% 69%)"
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
  led-off: # unlit LED segment; one grey for both themes, because the HUD paints no surface
    light: "hsl(0 0% 0% / 0.22)"
    dark: "hsl(0 0% 0% / 0.22)"
  # Session-analysis sub-palette only (src/styles/session-analysis-colors.css)
  context-fill-top:
    light: "hsl(217.2 91% 59.8% / 0.6)"
    dark: "hsl(213 94% 67.8% / 0.75)"
  context-fill-base:
    light: "hsl(217.2 91% 59.8% / 0.2)"
    dark: "hsl(213 94% 67.8% / 0.25)"
  token-in:
    light: "hsl(217.2 91% 59.8%)"
    dark: "hsl(213 94% 67.8%)"
  token-out:
    light: "hsl(255 91% 76.2%)"
    dark: "hsl(253 94% 85%)"
  token-subagent:
    light: "hsl(161.3 93% 30.4%)"
    dark: "hsl(158 64% 51.56%)"
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
rounded:
  small: 4px
  control: 5px
  popover: 10px # outer corner for macOS floating popover and notification surfaces
  full: 9999px
shadow:
  popover: "0 4px 12px rgb(0 0 0 / 0.15), 0 1px 3px rgb(0 0 0 / 0.08)"
  tooltip: "0 2px 8px rgb(0 0 0 / 0.12), 0 0.5px 2px rgb(0 0 0 / 0.06)"
  raised: "0 1px 2px rgb(0 0 0 / 0.15), 0 0 0 0.5px rgb(0 0 0 / 0.04)"
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
  package records. Marks are never drawn inline; they come from the `renderAgentIcon` slot.
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
  resting meaning — the title paints as plain primary text.
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
