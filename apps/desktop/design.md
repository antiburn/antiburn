# Desktop design guide

This guide helps contributors make design decisions for antiburn's desktop app.
It describes visual intent, shared rules, and exceptions that are difficult to
infer from one component. CSS and components own exact values and behavior; the
source map below points to them. Treat a screenshot or an old rule as evidence
of one implementation, then check the current code and user outcome before
reusing it.

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
palette. Keep one-off geometry or visualization values with the feature that
uses them. A new shared token needs a name that explains its job, theme behavior
where relevant, and a real second use or a clear system role.

## Colour and surfaces

Use semantic Tailwind utilities such as `bg-surface-window`, `text-label-secondary`,
and `border-separator`. The palette in `tokens.css` resolves System, Light, and
Dark themes; components should not choose their own theme branch. Check a
colour on the surface where it appears, including translucent popovers over
uncontrolled desktop backgrounds. Reduced transparency must leave content
legible on a solid surface.

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

Use the `type-*` scale in `typography.css`; type steps inherit the base
line height from `base.css`. Most data, rows, and labels use body-sized text;
titles and hero figures are deliberate exceptions. Set
hierarchy with weight and contrast before making type larger. Pair figures
that users compare with tabular numerals, and use monospace where the reading
behaves like an instrument. The scale is unlayered CSS, so a deliberate weight
override on a `type-*` class uses a Tailwind important modifier such as
`font-medium!`.

Use [shared controls](src/components/ui/) for buttons, rows, tabs,
disclosures, menus, tooltips, and scrolling. They provide hit areas, states,
focus, and accessibility behavior. Use `rounded-control` for controls and the
semantic radius for larger surfaces. A feature should not rebuild menu or
tooltip material in its own stylesheet. Style headless-control states from
their state attributes as well as pointer hover.

Use Lucide for ordinary interface icons. Give a meaningful icon an accessible
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

The global reduced-motion rule in `motion.css` stops animation and transition
by default. Add motion only when it explains a change or gives useful feedback.
Use the shared duration variables; in Tailwind, write
`duration-[var(--duration-fast)]`, not a copied millisecond value.
Do not animate navigation, resize corrections, or continuously changing
readings merely to make them feel active. Ambient loops must stop under reduced
motion and leave a static indication with the same meaning. Any essential
reduced-motion exception belongs in motion.css with its reason.

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
colour alone. Keep a category's hue stable across a view and distinguish
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
