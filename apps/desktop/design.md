# Desktop design guide

This guide helps contributors make design decisions for antiburn's desktop app.
It describes visual intent, shared rules, and exceptions that are difficult to
infer from one component. These are requirements for contributors and reviewers.
CSS owns exact token values; shared components implement the rules below. The
source map points to those owners. Existing code or a screenshot can contain a
violation; neither overrides a written rule. Change a rule explicitly, with its
rationale, rather than treating an implementation difference as permission.

Use this guide when adding a surface, changing visual hierarchy, or deciding
whether a pattern belongs in the shared system. Update it when a design rule
or its rationale changes. A CSS value change does not need a transcription here.

## What the interface should do

antiburn is a compact desktop instrument for understanding local AI-agent
activity. Show the reading, its scope, and the next useful action. Keep chrome
quiet enough that changes in usage, checks, and session evidence stand out.
Prefer a clear label or a small explanation over decoration that asks the user
to infer meaning.

Build hierarchy with placement, type, weight, and ink before adding cards,
colour, or motion. A card should group related information or identify an
interactive row; it should not be the default wrapper for every section.
Avoid headings that repeat the content immediately below them. Communicate
when a reading is estimated or evidence is incomplete without crowding each
figure with a permanent qualifier.

Write interface copy in sentence case. Name the state plainly and make action
labels describe what pressing them does. Keep explanations short and specific
about the evidence, limit, or next step; avoid vague success and failure copy.
Use active voice and present tense. Do not use marketing superlatives,
exclamation marks, or promises the app cannot keep. Spell `antiburn` lowercase.
Keep accessible names short; do not repeat an entire message in its action label.

Settings owns stored preferences, the popover reads, onboarding owns first run,
and notifications alert. Do not put settings controls in the popover or turn a
notification into a reading pane. Deep links must name and reach their destination.

The menu-bar popover and HUD favor glanceable readings; the main window gives
evidence room to breathe. Reuse meanings and primitives across surfaces
without forcing every surface into the same density.

## Where to find the source of truth

| Concern                                                   | Owner                                                                                                                                                        |
| --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Semantic colours, spacing, radii, shadows, theme branches | [tokens.css](src/styles/tokens.css)                                                                                                                          |
| Base text and cursor behavior                             | [base.css](src/styles/base.css)                                                                                                                              |
| Type steps                                                | [typography.css](src/styles/typography.css)                                                                                                                  |
| Focus and reduced motion                                  | [focus.css](src/styles/focus.css), [motion.css](src/styles/motion.css)                                                                                       |
| Shared controls and platform details                      | [controls.css](src/styles/controls.css), [platform-controls.css](src/styles/platform-controls.css), [UI components](src/components/ui/)                      |
| Main window, sessions, checks, HUD, and charts            | Their feature components and styles in [views](src/views/), [components](src/components/), and [styles](src/styles/)                                         |
| Native window geometry and interface scale                | Native window owners, [interface-scale.css](src/styles/interface-scale.css), and the [interface scale QA runbook](../../docs/runbooks/interface-scale-qa.md) |

Use existing CSS variables and components before introducing a new value.
If a value represents a recurring meaning, add a semantic token in the relevant
palette. The existing exception for one-off visualization or geometry values
requires a local comment explaining the constraint; it does not permit ad-hoc
shared chrome. Token definitions in the owning CSS palette and package-sourced
vendor artwork are also
exceptions to the feature-code literal ban. A token name must explain its job
and define theme behavior where relevant.

## Colour and surfaces

**No raw colour in feature code:** no hex literals, ad-hoc `rgb()` or `hsl()`,
or stock Tailwind colours such as `bg-blue-500` or `text-slate-400`. Use semantic
utilities such as `bg-surface-window`, `text-label-secondary`, and
`border-separator`, subject only to the exceptions above. Do not add manual
`dark:` colour overrides; semantic tokens own both theme values. The palette in
`tokens.css` resolves System, Light, and Dark. Check a
colour on the surface where it appears, including translucent popovers over
uncontrolled desktop backgrounds. System must match the corresponding explicit
Light or Dark palette. Reduced transparency must make window and popover
surfaces solid and legible in every System, Light, and Dark branch.

Choose a token for its meaning rather than the nearest-looking hue:

- `surface-window` is the opaque app canvas; `surface` is the menu-bar material.
  Use `surface-overlay` for opaque floating dialogs. Use `surface-card` to group
  content, `surface-hover` for transient row hover, and `surface-selected` for
  persistent list selection. Keep selection visible on hover.
- `label`, `label-secondary`, and `label-tertiary` form the reading hierarchy.
  Tertiary ink still needs enough contrast for information a user must read.
- `accent` identifies interactive emphasis. Use `accent-fill` for an accent
  background: the live system accent token does not resolve reliably as a CSS
  background colour. Selected rows use a neutral fill.
- `brand` is text and small-glyph orange; `brand-tint` is for larger fills.
  Status and chart palettes have their own meanings. Do not turn ordinary
  metadata orange to make it look important.

Status colours express conditions, never decoration. Use the corresponding
`-text` token where provided for status text; a fill token may fail text contrast.
Use tinted status backgrounds on translucent surfaces, not saturated bands.
Do not layer a feature's own blur or opaque fill over the native material.

Status must remain understandable from text, shape, or position when colour
is unavailable. Burn Check failure, pass, and unassessed states use the
feature palette in [tokens.css](src/styles/tokens.css) and the shared
[BurnCheckIndicator](src/components/burn-checks/BurnCheckIndicator.tsx).
Reserve stronger fills for a measured state or action; leave explanatory prose
in ordinary ink.

Vendor marks are trademarks rather than Lucide icons. Render them through
[renderAgentIcon](src/lib/agentIcon.tsx), including its neutral and brand-colour
treatment. Do not hand-code a vendor hex into a component. Decorative marks
and watermarks must not replace a visible source name when identity matters.

## Type, controls, and interaction

**No hard-coded type sizes:** do not use raw `font-size`, `text-[13px]`, or
framework size utilities such as `text-sm` in feature code. Use the `type-*`
scale in `typography.css`; type steps inherit the base
line height from `base.css`. Use one body-sized data step per view; reserve
other steps for hero figures, guidance, headings, and footnotes. Set
hierarchy with weight and contrast before making type larger. Pair figures
that users compare with tabular numerals, and use monospace where the reading
behaves like an instrument. Product prose uses the system sans stack; monospace
is for machine text and the documented instrument/metric roles. The scale is
unlayered CSS, so a deliberate weight
override on a `type-*` class uses a Tailwind important modifier such as
`font-medium!`. Weight modifiers must keep their `type-*` size. Italics are
reserved for a placeholder sentence in an otherwise empty list.

Settings uses `Pane` → `SectionGroup` → `Row` for its descending type hierarchy.
Only the pane title is semibold; group and row hierarchy uses size and contrast.
Do not rebuild that ladder per pane.

Use [shared controls](src/components/ui/) for buttons, rows, tabs,
disclosures, menus, tooltips, and scrolling. They provide hit areas, states,
focus, and accessibility behavior. Do not hand-roll their chrome. Control heights
come from shared tokens and primitives. **No ad-hoc radii:** use named radii,
not `rounded-md`, `rounded-lg`, or arbitrary pixel radii, except for the documented
local-geometry cases above. Use `rounded-control` for controls.

Use `gap-*` between children, not `space-x-*` or `space-y-*`. Do not rebuild
menu, tooltip, or scroll-fade material in feature CSS. Headless control states
must use `[data-state]` and `[data-highlighted]`, not rely on `:hover`: passive
notification windows may receive no pointer hover events. Use `InlineLink` for
outbound prose links so a normal `href` cannot navigate the app webview.
A `Banner` is one line, dismissible, has at most one action, and uses a polite
status region. Permission notices are a separate, non-dismissible state.
Buttons use the arrow cursor by default; full-row disclosures may use a pointer.

Use Lucide with `currentColor` for ordinary interface icons. No emoji in product
chrome: lists, labels, buttons, panes, or notifications. Give a meaningful icon an accessible
name through its control or adjacent label; hide a decorative icon from
assistive technology. Do not use an icon, colour, or tooltip as the only way
to learn a critical result or action. Check text and icon contrast against the
composited surface: at least 4.5:1 for ordinary text and 3:1 for large text and
meaningful controls or graphics. Give controls a usable hit area, including coarse pointers,
and provide a keyboard equivalent for every pointer interaction. A tooltip
can explain a figure's method or reveal truncated context, but the main
reading stays visible without hover.

The keyboard-only focus treatment in [focus.css](src/styles/focus.css) uses
`html[data-keyboard]`, which [focusModality.ts](src/lib/focusModality.ts)
updates after Tab and pointer input. Preserve it on custom controls. Keep
selection distinct from focus. Dialogs, drawers, and menus need predictable
initial focus, Escape dismissal, and focus restoration. Navigation by search
may focus a destination; it must not silently change a setting or run an action.
A popover surface change must focus its meaningful heading rather than leave
focus on the body. Preserve roving keyboard navigation and valid `aria-controls`,
`aria-labelledby`, and tab/panel roles. Do not suppress the keyboard focus ring.
Escape dismisses transient popovers, not the nonmodal Settings window.
Command-W or Control-W closes a decorated window.

The global reduced-motion rule in `motion.css` stops animation and transition
by default. Add motion only when it explains a change or gives useful feedback.
Use the shared duration variables; in Tailwind, write
`duration-[var(--duration-fast)]`, not a copied millisecond value. Do not use
literal durations, inline easing curves, or bare transition utilities that
silently inherit framework timings. Animation-specific timing stays with its
owning keyframes. Preserve completion callbacks such as `animationend` when
an action depends on them, including under reduced motion.
Main-window view switches are immediate; do not animate navigation. Do not
animate resize corrections or continuously changing readings merely to make
them feel active. Ambient loops must stop under reduced
motion and leave a static indication with the same meaning. Any essential
reduced-motion exception belongs in motion.css with its reason.

## Required states

Every data-loading surface must handle the states that apply to it:

- Empty: show a title and a line explaining why it is empty and what would fill it.
- Loading: use content-shaped `Skeleton` placeholders, not a spinner over a blank
  surface. Avoid layout jumps; mark the region `aria-busy` and placeholders
  `aria-hidden`.
- Error: say what failed and provide a next step.
- Permission blocked: explain the missing access before a user-triggered consent
  prompt. Keep the gap visible; do not treat it as a crash or hide the denied source.
- Freshness: indexed data must show whether it is scanning and when it was last checked.
- Progress and status changes: announce through a polite live region, not assertive.

## Layout and scaling

Design against the available CSS viewport and the width of the component's
own pane. Prefer container queries when a list or card changes inside a
resizable pane. Keep related labels, figures, and actions aligned; when space
shrinks, reflow or scroll before dropping essential information. Each pane
owns its scroll area. Use the shared [ScrollPane](src/components/ui/ScrollPane.tsx)
edge fades when scrollable content meets a fixed header or footer, keeping
fixed labels and scrollbars outside the mask.

Interface size is a reader preference applied by native WebView zoom. Do not
multiply type, spacing, radii, or icons by the scale in CSS. Native chrome
keeps its own geometry. Layout responds to the resulting CSS viewport, not
physical pixels or `devicePixelRatio`. Preserve the chosen size on small
displays by reflowing content; see the [interface scale QA runbook](../../docs/runbooks/interface-scale-qa.md)
for native checks that browser fixtures cannot prove.

Use shared main-window navigation and collection/detail primitives for their
keyboard and history behavior. Keep the sidebar, collection, and detail as
separate reading and scrolling regions where space allows. Preserve collection
focus and scroll context when Back returns from detail. Platform caption
controls and drag regions must retain native behavior while interactive
content keeps its input.

Platform differences belong in the design foundation or native window owner.
[platform.ts](src/lib/platform.ts) sets the platform attribute; ordinary
feature components should not branch on the operating system for appearance.
A window with custom chrome owns its drag strip and clearance; a
native-decorated window does not reserve that space.

## Data views and charts

Make a number's period, unit, and evidential status available where the user
reads it; a concise tooltip can carry method or estimate detail. Cost figures
are estimates at list rates, not bills.
Unknown costs remain gaps rather than zeroes, and incomplete periods need an
explicit mark. Never imply that a bounded sample covers every session. The
[session](../../docs/session-coverage.md) and
[check coverage](../../docs/check-coverage.md) documents describe the limits
of parsed evidence.

Use shared [HeroFigures](src/components/ui/HeroFigures.tsx) for comparable
headline readings and [SegmentedMeter](src/components/ui/SegmentedMeter.tsx)
for horizontal usage meters. Keep ranked figures visually consistent: their
order already communicates magnitude. A chart needs direct names or a legend,
units, readable axes, and a way to inspect exact values without relying on
colour alone. Chart series must resolve colours through CSS tokens. Keep feature
chart palettes within their feature; do not borrow them for general product chrome.
Comment new chart tokens with their meaning. Keep a category's hue stable across a view and distinguish
layers with weight, opacity, or shape. Feature chart palettes live with their
styles, including [session-analysis-colors.css](src/styles/session-analysis-colors.css)
and [quota.css](src/views/main-window/quota/quota.css).

Show the state and scope of a Burn Check before offering a fix. Use shared
result presentation so a collection row, session card, tooltip, and detail
agree on counts and wording. An unassessed check is not a pass. For named
resources, distinguish affected sessions from occurrences or bounded samples.
Put actions beside the evidence they affect, and keep a prompt action inert
until an exact target is selected.

## Review a change

Before calling a visual change done, inspect the affected surface in Light,
Dark, and System themes; reduced transparency and reduced motion where they
matter; keyboard focus and screen-reader names; and the narrowest supported
CSS viewport. For data views, inspect loading, empty, partial, failure, and
large-value states. Check native chrome and scale on each supported platform
when the change touches geometry or input. Change this guide only when the
review establishes a new reusable rule or changes a rule above.
