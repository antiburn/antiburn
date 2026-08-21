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
  - src/styles/session-analytics-colors.css
  - src/styles/session-rows.css
  - src/components/ui/text-roll.css
colors:
  # name → Tailwind utility via bg-/text-/border-<name>
  # Both values are the explicit [data-theme="light"|"dark"] palettes.
  # A `# @media <theme>: <value>` note states the system-preference value where it
  # differs. The drift check reads those notes: a difference it cannot find a note
  # for is a failure, and so is a note that no longer differs.
  surface:
    light: "rgb(255 255 255 / 0.58)" # reduced-transparency: rgb(255 255 255)
    dark: "rgb(30 30 30 / 0.92)" # @media dark: rgb(30 30 30 / 0.40)
  surface-secondary:
    light: "rgb(0 0 0 / 0.08)"
    dark: "rgb(255 255 255 / 0.12)"
  surface-tertiary:
    light: "rgb(0 0 0 / 0.12)"
    dark: "rgb(255 255 255 / 0.18)"
  surface-card:
    light: "rgb(0 0 0 / 0.04)"
    dark: "rgb(255 255 255 / 0.08)"
  surface-hover: # stays clear of surface-selected, so a hover never reads as a selection
    light: "rgb(0 0 0 / 0.04)"
    dark: "rgb(255 255 255 / 0.04)" # @media dark: rgb(255 255 255 / 0.07)
  surface-window: # standard decorated window
    light: "rgb(246 246 246)" # @media light: rgb(246 246 246 / 0.80)
    dark: "rgb(32 32 32)" # @media dark: rgb(40 40 40 / 0.80)
  surface-sidebar: # source-list / sidebar material
    light: "rgb(0 0 0 / 0.03)"
    dark: "rgb(255 255 255 / 0.04)"
  surface-selected: # selected row in a list or source list (accent-fill stays for controls)
    light: "rgb(0 0 0 / 0.09)"
    dark: "rgb(255 255 255 / 0.14)"
  input-fill:
    light: "rgb(255 255 255)" # @media light: rgb(255 255 255 / 0.50)
    dark: "rgb(58 58 60)" # @media dark: rgb(255 255 255 / 0.08)
  label: # live system label token where available
    light: "rgb(0 0 0 / 0.85)"
    dark: "rgb(255 255 255 / 0.92)"
  label-secondary:
    light: "rgb(60 60 67 / 0.85)"
    dark: "rgb(235 235 245 / 0.72)"
  label-tertiary: # 4.5:1 on a card, the lightest surface it sits on
    light: "rgb(60 60 67 / 0.75)"
    dark: "rgb(235 235 245 / 0.55)"
  separator: # live system separator token where available
    light: "rgb(0 0 0 / 0.15)"
    dark: "rgb(255 255 255 / 0.18)"
  accent: # live system accent token where available
    light: "rgb(0 122 255)"
    dark: "rgb(10 132 255)"
  accent-hover: # darker than accent-fill, because white text sits on it
    light: "rgb(0 98 199)"
    dark: "rgb(10 96 205)"
  accent-fill: # concrete fill; use bg-accent-fill for backgrounds
    light: "rgb(0 113 227)"
    dark: "rgb(10 110 235)"
  system-green:
    light: "rgb(36 138 61)"
    dark: "rgb(48 219 91)"
  system-orange:
    light: "rgb(179 81 0)"
    dark: "rgb(255 179 64)"
  system-orange-tint:
    light: "rgb(255 149 0)"
    dark: "rgb(255 159 10)"
  system-yellow:
    light: "rgb(160 90 0)"
    dark: "rgb(255 212 38)"
  system-red:
    light: "rgb(215 0 21)"
    dark: "rgb(255 105 97)"
  system-red-text:
    light: "rgb(190 0 20)"
    dark: "rgb(255 138 128)"
  system-blue:
    light: "rgb(0 122 255)"
    dark: "rgb(10 132 255)"
  system-indigo:
    light: "rgb(88 86 214)"
    dark: "rgb(94 92 230)"
  system-indigo-text:
    light: "rgb(88 86 214)"
    dark: "rgb(150 148 255)"
  system-gold:
    light: "rgb(202 138 4)"
    dark: "rgb(255 204 0)"
  system-gold-text:
    light: "rgb(146 100 0)"
    dark: "rgb(245 203 92)"
  agent-mark: # vendor brand-mark ink; see the Vendor brand marks note below
    light: "rgb(38 37 30)"
    dark: "rgb(247 247 244)"
  # Floating-HUD sub-palette only (src/styles/hud.css)
  burn:
    light: "rgb(255 77 0)"
    dark: "rgb(255 106 0)"
  burn-muted:
    light: "rgb(240 88 22)"
    dark: "rgb(242 113 34)"
  bg-hud:
    light: "rgb(246 246 246)"
    dark: "rgb(32 32 32)" # @media dark: rgb(40 40 40)
  # Session-analytics sub-palette only (src/styles/session-analytics-colors.css)
  mode-implementing:
    light: "rgb(29 78 216)"
    dark: "rgb(37 99 235)"
  mode-testing:
    light: "rgb(5 150 105)"
    dark: "rgb(52 211 153)"
  mode-exploring:
    light: "rgb(59 130 246)"
    dark: "rgb(96 165 250)"
  mode-thinking:
    light: "rgb(6 182 212)"
    dark: "rgb(34 211 238)"
  mode-disruption:
    light: "rgb(234 88 12)"
    dark: "rgb(255 149 10)"
  context-fill-top:
    light: "rgb(59 130 246 / 0.55)"
    dark: "rgb(96 165 250 / 0.6)"
  context-fill-base:
    light: "rgb(59 130 246 / 0.08)"
    dark: "rgb(96 165 250 / 0.1)"
  context-fixed:
    light: "rgb(100 116 139)"
    dark: "rgb(148 163 184)"
  context-system:
    light: "rgb(124 58 237)"
    dark: "rgb(167 139 250)"
  token-in:
    light: "rgb(59 130 246)"
    dark: "rgb(96 165 250)"
  token-out:
    light: "rgb(167 139 250)"
    dark: "rgb(196 181 253)"
  pattern-drifting:
    light: "rgb(217 119 6)"
    dark: "rgb(255 159 10)"
fonts:
  sans: "-apple-system, BlinkMacSystemFont, SF Pro Text, system-ui, sans-serif"
  mono: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" # via `font-mono`
typography:
  # class .type-<name> · [fontSize, fontWeight, lineHeight, letterSpacing] · family = fonts.sans
  large-title: { fontSize: 26px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0.36px" }
  title-1: { fontSize: 22px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0.35px" }
  title-2: { fontSize: 17px, fontWeight: 600, lineHeight: 1.4, letterSpacing: "-0.43px" }
  title-3: { fontSize: 15px, fontWeight: 600, lineHeight: 1.4, letterSpacing: "-0.23px" }
  headline: { fontSize: 13px, fontWeight: 600, lineHeight: 1.4, letterSpacing: "-0.08px" }
  body: { fontSize: 13px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "-0.08px" }
  callout: { fontSize: 12px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0" }
  footnote: { fontSize: 11px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0.12px" }
  caption: { fontSize: 10px, fontWeight: 400, lineHeight: 1.4, letterSpacing: "0.06px" }
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
  popover: 10px
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
- **Type scale** — `.type-*` classes, declared outside any `@layer` in `src/styles/typography.css`.
  Unlayered CSS outranks Tailwind's `utilities` layer, so overriding a baked-in weight needs the
  important modifier (`font-normal!`), as `SectionGroup` does.
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
  the arrow cursor everywhere.
- **Motion** — `prefers-reduced-motion: reduce` clamps every animation and transition globally
  (`src/styles/motion.css`). A surface that still needs a hint of movement re-states a short
  duration there, with the reason; today the only such exception is the segmented control's
  reduced-motion fill, which crossfades over 60ms instead of swapping instantly. An ambient loop
  stops instead of shortening: no duration makes a loop acceptable, so the activity-row pulse and
  title shimmer in `src/styles/session-rows.css` set `animation: none` and each keeps its resting
  meaning — the pulse settles at a fixed tint, and the title paints as plain primary text.
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
