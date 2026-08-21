<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

# antiburn design principles — the design-review rulebook

This is the rulebook the design-review skill grades a window against.

`apps/desktop/design.md` is the token contract, and it wins on every value it
states. This file adds what a token cannot state: which window owns what, which
state a surface must have, how the copy sounds, and where the accessibility line
sits. Every rule below carries a number so a finding can cite it.

Paths in this file are relative to `apps/desktop/`.

## 0. Source of truth and how to cite a finding

- `design.md` is the contract. Its YAML front matter lists every colour, type
  role, spacing step, radius, shadow, and motion value.
- The CSS under `src/styles/` is where those values live: `tokens.css`,
  `base.css`, `typography.css`, `focus.css`, `controls.css`, `motion.css`,
  `platform-controls.css`, `session-analytics-colors.css`, `session-rows.css`.
  A component-scoped stylesheet is imported by the component that owns it, not
  from `src/styles.css`.
- The shared primitives live in `src/components/ui/`. Each one carries a doc
  comment that states what it is for. Read that comment before you claim a
  component is wrong.
- `scripts/check-design-drift.mjs` keeps the contract and those stylesheets
  equal. So a value in `design.md` that disagrees with the CSS is a CI failure,
  not a design finding. Report it, and stop there.
- `docs/deviations.md` records every knowing difference from the ratified
  feature matrix. Check it before you report something as a mistake. A recorded
  deviation is a decision, not a finding.
- Cite a rule number from this file, plus the token, class, or file it breaks.
  A finding with no citation is an opinion.

## 1. North star (what an antiburn window should feel like)

- Native, not web. Each window should read like part of the operating system.
  It borrows the system's palette, its type sizes, and its control geometry.
- Quiet. The app watches work that is already happening. It reports; it does not
  perform. Nothing pulses for attention without a real signal behind it.
- One glance per surface. A surface answers one question. If a reader has to
  scroll a fixed-height window to get the answer, the surface is doing too much.
- Local and honest. The app says what it knows, when it last looked, and what it
  cannot see. A gap is stated, never hidden.

## 2. Windows and surfaces (information architecture)

There is no router. Each window opens with a URL fragment and keeps one route
for its whole life (`src/lib/route.ts`).

| Route | Window | Size | What it owns |
|---|---|---|---|
| default | Tray popover | 380 wide; 700 tall, 780 on Usage | The reading surfaces: what happened, and what it cost |
| `#/settings` | Settings | 960 × 680 | Every choice the reader makes, and every explanation |
| `#/onboarding` | Onboarding | 680 × 480 | First run only |
| `#/nudge` | Notification | 344 wide | One alert, always on top |

**RULE 2.1 — put content in the window that owns it.** The popover reads. The
settings window decides and explains. The notification alerts. A control that
changes stored settings does not belong in the popover, and a paragraph of
explanation does not belong in the notification. **[High]**

**RULE 2.2 — the popover has three surfaces, and only three.** They are
`activity`, `session`, and `usage` (`src/lib/popoverHeight.ts`). `activity` is
the list. `session` is one session's analytics, reached from the list. `usage`
sits over the list rather than in the session stack, because it is a second way
to read the same activity. Do not add a fourth surface without a decision
record. **[High]**

**RULE 2.3 — respect the popover height contract.** 700 is the height the
window is created at and rests at. 780 is a ceiling that exactly one surface
uses, and that use is a recorded deviation (D-22). A new surface that asks for
780 is a finding until a deviation records it. **[High]**

**RULE 2.4 — the settings pane order is deliberate.** General, Privacy,
Notifications, Usage, Sources, Appearance, About (`src/views/SettingsView.tsx`).
Everyday panes come first and provenance comes last. About closes the list and
carries software update. The pane ids live in `src/lib/settingsPanes.ts`, so a
renamed pane is a type error at every call site. **[Medium]**

**RULE 2.5 — a deep link must land somewhere real.** The popover's attention
banners open the settings window at the pane that can fix what they report
(`src/lib/attention.ts` → `openSettingsWindow(pane)`). The About pane links to
sibling panes the same way. Every link states where it goes. A link that opens
the window at the wrong pane, or at no pane, is a finding. **[High]**

**RULE 2.6 — the notification never becomes a third reading surface.** It
carries a title, one reason line, optional recommendations, and at most a short
action bar. Anything that needs a paragraph belongs in a pane. **[Medium]**

**RULE 2.7 — window chrome belongs to the window.** A window that hides its
native title bar owns the drag strip and the matching top clearance. A window
that keeps native decorations must not reserve that space. Keep that decision in
the window's own layout, never in a shared primitive. **[Medium]**

## 3. Colour and tokens

Every `colors` key in `design.md` is a Tailwind utility through
`bg-<name>`, `text-<name>`, and `border-<name>`.

**RULE 3.1 — no raw colour in feature code.** No raw hex. No ad-hoc `rgb()`. No
stock Tailwind colours such as `bg-blue-500` or `text-slate-400`. Use the
semantic utilities: `bg-surface`, `bg-surface-card`, `text-label`,
`text-label-secondary`, `border-separator`, `text-system-green`. **[High]**

- Do NOT flag the token layer itself. `src/styles/tokens.css` and the other
  files under `src/styles/` are where the raw values legitimately live.
- Do NOT flag a documented one-off visualization or geometry value. `design.md`
  allows those, and asks that they stay local and carry a comment.

**RULE 3.2 — use `bg-accent-fill` for an accent background.** The live system
accent token resolves incorrectly as a `background-color`, so `bg-accent` is
wrong for a fill. `text-accent` and `border-accent` are fine. **[High]**

**RULE 3.3 — status colour carries meaning, never decoration.** `system-red`,
`system-orange`, `system-yellow`, and `system-green` state a condition. They are
not brand colour and not a way to add interest. **[Medium]**

**RULE 3.4 — use the `-text` variant when a status colour is text.** Some fill
colours drop below AA as text. `tokens.css` says so: the red fill reads about
4.04:1 on a tinted pill in light mode. So `system-red-text`,
`system-indigo-text`, and `system-gold-text` exist. Coloured text that uses the
fill variant instead of the `-text` variant is a contrast finding. **[High]**

**RULE 3.5 — no manual `dark:` colour override.** Every semantic token already
carries both values. `bg-white dark:bg-neutral-900` is a finding; `bg-surface`
is the fix. **[Medium]**

**RULE 3.6 — a tinted fill, not a saturated band.** The popover surfaces are
translucent. A solid status colour across one reads as a separate window. Follow
`Banner`, which tints (`bg-system-orange-tint/12 text-system-orange`).
**[Medium]**

## 4. Typography

- The type ladder is the `.type-*` classes in `src/styles/typography.css`:
  `type-large-title`, `type-title-1`, `type-title-2`, `type-title-3`,
  `type-headline`, `type-body`, `type-callout`, `type-footnote`,
  `type-caption`.
- Those classes sit outside any `@layer`, so unlayered CSS outranks Tailwind's
  `utilities` layer. To override a baked-in weight you need the important
  modifier, as `SectionGroup` does with `font-normal!`.

**RULE 4.1 — no hardcoded type size.** `text-[13px]`, `text-sm`, and a raw
`font-size` are all findings. Use a `type-*` class. **[High]**

**RULE 4.2 — follow the settings type ladder.** One descending step per level,
set by the `ui/` primitives rather than per pane: pane title `type-title-2`
(`Pane`) → group header `type-title-3 font-normal!` (`SectionGroup`) → row label
`type-body` (`Row`) → row description `type-footnote text-label-secondary`. Only
the pane title is semibold. Below it, size and contrast carry the hierarchy. A
hand-rolled row must match `Row`. **[Medium]**

**RULE 4.3 — figures line up.** A number that changes in place, a column of
numbers, and a timer all take `tabular-nums`. Without it the digits jitter.
**[Medium]**

**RULE 4.4 — `font-mono` is for machine text.** Paths, identifiers, and raw
values. Not for product copy and not for figures the reader is meant to compare
at a glance. **[Nitpick]**

## 5. Spacing, geometry, and layout

- Spacing is a 4px rhythm. `design.md` lists both the Tailwind steps (1–6) and
  the `--space-*` variables.
- Geometry variables are raw CSS custom properties in `tokens.css` and are not
  registered with `@theme`. Consume them as arbitrary values, such as
  `w-[var(--sidebar-width)]`.
- Radii: `rounded-small` (4px), `rounded-control` (5px), `rounded-popover`
  (10px), `rounded-full`.

**RULE 5.1 — no ad-hoc radius.** `rounded-md`, `rounded-lg`, and
`rounded-[7px]` are findings. Use a named radius. **[Medium]**

**RULE 5.2 — control height comes from the token.** 22px regular
(`--control-height-regular`), 17px small (`--control-height-small`). The
`ui-push-button` class already supplies it. **[Medium]**

**RULE 5.3 — `gap-*` between children.** Not `space-x-*` or `space-y-*`.
**[Medium]**

**RULE 5.4 — a fixed window must not overflow.** Every window here has a fixed
size. Check that no surface clips its content, that the scroll area scrolls, and
that nothing hides under the drag strip. **[High]**

**RULE 5.5 — use `ScrollPane` for a scroll area.** Its `ui-scroll-viewport`
class is load-bearing, not cosmetic. When scrolling content must dissolve into a
fixed top boundary, use the `topEdgeFade` prop and keep fixed labels outside
`ScrollPane`. Do not rebuild that effect with an overlay, a fill, a backdrop
blur, or a feature-specific gradient. **[High]**

## 6. Primitives — use the real ones

Accessibility, keyboard behaviour, and the type ladder all ride on these. A
hand-rolled copy loses all three. The set in `src/components/ui/` is:

`Banner`, `Card`, `Disclosure`, `InlineLink`, `Pane`, `PushButton`,
`RangeSlider`, `Row`, `ScrollPane`, `SectionGroup`, `SegmentedControl`,
`SidebarNav`, `Skeleton`, `StatusText`, `TextRoll`, `ToggleRow`, `ToggleSwitch`.

**RULE 6.1 — reach for the primitive first.** A pane is a `Pane` with
`SectionGroup` groups and `Card` rows. A setting with a switch is a `ToggleRow`.
A label with a control on the right is a `Row`. A one-line alert is a `Banner`.
A placeholder is a `Skeleton`. A live status line is a `StatusText`. An outbound
link inside a sentence is an `InlineLink`, which is a `button` rather than an
`<a>` because a real `href` in a desktop window is a navigation hazard.
**[High]**

**RULE 6.2 — use the `ui-*` classes for control chrome.** `ui-push-button`,
`ui-menu`, `ui-tooltip`, `ui-switch` with `ui-switch-thumb`,
`ui-radio-indicator` with `ui-radio-dot`, `ui-progress` with
`ui-progress-indicator`, `ui-scrollbar` with `ui-scrollbar-thumb`. They live in
`src/styles/controls.css` and `src/styles/platform-controls.css`. A restyled
`<button>` that re-derives that geometry is a finding. **[High]**

**RULE 6.3 — style headless state with `[data-state]` and `[data-highlighted]`,
not `:hover`.** The headless primitives already report their state. In the
notification window this is not a preference but a requirement: pointer events
do not reach that window while another window holds key status, so CSS `:hover`
never fires there. **[High]**

**RULE 6.4 — a banner is always dismissible, has at most one action, and is one
line.** That shape is what keeps it from becoming noise. It renders as
`role="status"` with a polite live region. **[Medium]**

**RULE 6.5 — the platform attribute stays in the foundation.**
`<html data-platform>` is set once at startup (`src/lib/platform.ts`). Only the
design foundation reads it. A component never asks what platform it is on.
**[Medium]**

## 7. State taxonomy

The token contract says nothing about states, so this section carries the whole
rule. Every surface that loads anything needs all of the states that apply to
it. A missing state is a real finding, not a polish item.

**RULE 7.1 — empty.** A title and one describing line, never a blank box. The
copy names why the surface is empty and what would fill it.
`LocalActivityList` does this: the title is range-aware ("No sessions today", or
"No sessions in the last N days") and the description says that sessions appear
as they are discovered. The usage surface says "No local evidence yet". **[High]**

**RULE 7.2 — loading.** A `Skeleton` in the shape of the content it hides, never
a spinner over a blank surface, and never a layout that jumps when data lands.
Mark the region `aria-busy` and hide decorative placeholders with `aria-hidden`.
The popover's activity skeleton and its lazy session-analytics placeholder both
show the pattern. **[High]**

**RULE 7.3 — error.** Say what failed, in plain words, and offer the way
forward. The onboarding window's failure state is the model: a title, the error
text, and a "Try again" button. The scan status line says "Last scan did not
finish" beside a warning glyph, and keeps the rescan control next to it. An
error with no next step is a finding. **[High]**

**RULE 7.4 — permission-blocked is its own state, and it is not an error.** The
operating system refusing a folder is a normal condition with a known remedy.
`FolderPermissionNotice` explains what the app wants and why BEFORE the system's
consent dialog appears, and that dialog only ever follows a button on the
notice. The notice is not dismissible, because it explains a visible gap in a
list rather than interrupting. A blocked folder must never read as a crash, and
must never silently vanish from the list. **[High]**

**RULE 7.5 — first run.** The onboarding window owns first run: five linear
steps (welcome, sources, repositories, scan, ready), a "Step N of M" text label
beside the step dots, and no step that cannot be reached by keyboard. First run
is not a popover surface, and it must not be rebuilt as one (D-25). **[High]**

**RULE 7.6 — freshness is a state.** A local-first app has to say when it last
looked. `ScanStatusBar` answers two questions on sight: is it looking, and how
old is what I am reading. A surface that reads indexed data and shows no
freshness or scan state is a finding. **[Medium]**

**RULE 7.7 — a state change is announced.** Use a polite live region for
progress and status (`aria-live="polite"`), never assertive. A scan finishing is
worth knowing. It is not worth interrupting what the reader is reading.
**[Medium]**

## 8. Data display and session analytics

**RULE 8.1 — the session-analytics palette stays in its own surface.** The
mode, context, token, and pattern colours in
`src/styles/session-analytics-colors.css` are a sub-palette for that one
surface. Do not borrow them as general product colour, and do not add a new one
without a comment that says what it means. **[Medium]**

**RULE 8.2 — a chart colour resolves through a token.** Chart series read their
colour from a CSS variable, so both themes work with no branch in the chart
code. **[High]**

**RULE 8.3 — no chart junk.** One accent, quiet gridlines, a label on the chart
in place of a busy legend. Thin strokes at chart scale
(`strokeWidth` 1.5 for large marks). **[Medium]**

## 9. Themes and materials (the theme pass)

Three theme sources, in cascade order (`design.md`, "Themes"):

1. The system light/dark preference is the default.
2. A webview that exposes live system label, separator, and accent tokens picks
   those up through `@supports`, so text and chrome track the operating system.
3. A platform without them takes an explicit `<html data-theme="light|dark">`
   palette. That palette is deliberately more opaque, because no window material
   sits behind it.

`prefers-reduced-transparency` makes the window and popover surfaces solid in
every branch.

**RULE 9.1 — both themes must work.** Toggle to dark and look. Flag anything
that only reads in one theme: a near-white or near-black value, text that
disappears, a border that vanishes. **[High]**

**RULE 9.2 — reduced transparency must stay legible.** With the material gone,
the surfaces are solid. Check that text on a translucent tint still reads, and
that a hairline is still visible. **[High]**

**RULE 9.3 — do not fight the material.** A surface that stacks its own backdrop
blur or its own opaque fill over the window material defeats the point. Use the
`surface-*` tokens. **[Medium]**

## 10. Motion

`src/styles/motion.css` holds one global reduced-motion clamp. Under
`prefers-reduced-motion: reduce`, every animation and transition collapses to
about 0ms and every loop is cut to a single iteration. A surface that still
needs a hint of movement re-states a short duration there, with the reason.
Today there is exactly one such exception: the segmented control's reduced-motion
fill crossfades over 60ms, because an instant swap between two filled segments
reads as a flicker.

**RULE 10.1 — use a motion token, not a literal value.** The durations are
`--duration-quick` (100ms), `--duration-fast` (120ms), and `--duration-slow`
(300ms), declared in `src/styles/tokens.css`. Tailwind has no `--duration-*`
theme namespace, so a call site writes `duration-[var(--duration-fast)]`. The
easing `--ease-out-quart` is `@theme`-registered, so it is the `ease-out-quart`
utility. A literal such as `duration-[120ms]`, a bare `transition-colors` that
inherits the framework default, or an inline `cubic-bezier(...)` is a finding.
An animation timing stays with the keyframes that own it. **[Medium]**

**RULE 10.2 — an ambient loop stops, it does not shorten.** No duration makes a
loop acceptable, so an ambient loop sets `animation: none` under reduced motion
and keeps its resting meaning. The activity-row pulse settles at a fixed tint
that still marks the row; the title shimmer drops its overlay and paints as
plain primary text. Both are in `src/styles/session-rows.css`. A loop that
merely runs faster, or one whose resting state loses the meaning the movement
carried, is a finding. **[High]**

**RULE 10.3 — a new reduced-motion exception needs a written reason.** Put it in
`motion.css` beside the rule, and say why the movement is load-bearing.
**[High]**

**RULE 10.4 — motion never carries meaning on its own.** If an animation stops,
the reader must still learn the same fact from text or from a glyph. **[High]**

**RULE 10.5 — an exit animation that gates an action must exist.** The
notification defers its dismiss to `animationend`, so the keyframes are
load-bearing. The clamp collapses them to about 0ms, and the event still fires.
Do not "simplify" such an animation away. **[Blocker]**

## 11. Accessibility (WCAG 2.1 AA)

**RULE 11.1 — contrast.** At least 4.5:1 for text, and at least 3:1 for large
text and for a meaningful UI or graphic element. Check the tertiary label
tokens, which are the closest to the line, and check every coloured status
string (see rule 3.4). **[High]**

**RULE 11.2 — the focus ring is keyboard-only, on purpose.** It paints under
`html[data-keyboard]`, which `src/lib/focusModality.ts` sets on Tab and clears
on any pointer press (`src/styles/focus.css`). This is deliberate: webviews
paint `:focus-visible` for programmatic focus and for window activation too,
which would put a ring on a window that simply reopened. So:

- Do NOT report "no focus ring on click" as a finding. That is the design.
- DO report a control that shows no ring after Tab.
- DO report a control that suppresses the ring with `outline-none` and puts
  nothing gated on `data-keyboard` back.
- DO check that every interactive element is reachable by Tab at all. **[High]**

**RULE 11.3 — focus lands somewhere on a surface change.** Swapping a surface
leaves focus on `<body>`, which tells a screen-reader user nothing. Every
popover surface marks its heading with `data-view-heading` and `tabIndex={-1}`,
and the heading takes focus when the surface changes. A new surface with no
focus target is a finding. **[High]**

**RULE 11.4 — tab and panel wiring.** `SidebarNav` is a vertical tablist with a
roving tabindex: only the selected row is tabbable, and Up, Down, Home, and End
move both selection and focus. Each row's `aria-controls` points at
`<id>-panel`, so the pane it drives must carry `role="tabpanel"`,
`id="<id>-panel"`, and `aria-labelledby="<id>-tab"`. A broken pair is a finding.
**[High]**

**RULE 11.5 — never colour alone.** Pair a status colour with a glyph, a word,
or a shape. Follow `StatusText`, where the glyph is decorative and the message
carries the meaning. **[High]**

**RULE 11.6 — a decorative glyph is hidden.** `aria-hidden` on every Lucide icon
that repeats what the text says. An icon-only control gets an `aria-label` that
names the action. **[High]**

**RULE 11.7 — a keyboard escape exists, and it is the right one.** Escape
dismisses the popover, because a tray popover is transient. Escape must NOT
close the settings window, because that window is not a modal. Command-W or
Control-W closes a decorated window, because an accessory app has no application
menu to own the shortcut. **[High]**

**RULE 11.8 — the ratchet.** On an EXISTING screen, an AA miss reports as
**High**, not Blocker, so the app improves steadily instead of stalling. NEW UI
is held to AA from the start, and an AA miss there is a **Blocker**.

## 12. Copy and voice

**RULE 12.1 — plain and calm.** Short words. Present tense. Active voice. Say
what happened and what to do. "Last scan did not finish" beside a rescan
control, not "Oops! Something went wrong." **[Medium]**

**RULE 12.2 — sentence case everywhere.** Buttons, labels, headings, and
titles. Title Case belongs to a platform menu item, not to product chrome.
**[Medium]**

**RULE 12.3 — no marketing voice.** No superlative, no exclamation mark, no
promise the app cannot keep. The app reports what it found on this machine.
**[Medium]**

**RULE 12.4 — a label names the action, not the condition.** A pinned window
offers "Unpin Window". A control that pauses says which of the two states
pressing it produces. **[Medium]**

**RULE 12.5 — the product name is `antiburn`, lower case, always.** **[Medium]**

**RULE 12.6 — an accessible name is short.** The message is a whole sentence, so
the dismiss control is named "Dismiss" and not the sentence again. A
screen-reader user should not sit through it twice. **[Medium]**

**RULE 12.7 — say what the app cannot see.** A gap, a blocked folder, and a
paused scan are stated plainly; never imply completeness the app does not have.
The same holds for a figure the app worked out itself: where an estimate stands
in for a provider's own number, the reader needs *some* way to tell. A tooltip
is usually the right amount — `SessionCostBadge` leads with the number and
explains it on hover. Raise this only when there is no way to find out at all;
a figure without a visible label is not a finding, and labelling every one of
them clutters a dense surface. **[High]**

## 13. Icons, marks, and emoji

**RULE 13.1 — icons come from `lucide-react`.** They inherit `currentColor`.
Sizes: 12 for footnote scale, 14 to 16 by default, 24 for a feature. Colour with
`text-*`. `strokeWidth` 2, or 2.5 to 3 for a tiny mark, or 1.5 for a large or
chart mark. Add `shrink-0` inside a flex row. **[Medium]**

**RULE 13.2 — a vendor brand mark is the one exception, and it is not
interchangeable with an icon.** A mark is a trademark, so its shape and colour
belong to the vendor. Marks are filled paths, not stroked glyphs, and take
`--color-agent-mark` rather than a `text-*` label colour. A mark whose identity
is its colour keeps that colour in both themes. Marks are never drawn inline;
they come from the `renderAgentIcon` slot. Do NOT report a mark's colour as a
raw-colour violation. **[High]**

**RULE 13.3 — no emoji in product chrome.** Not in a list, a button, a label, a
pane, or a notification. Use a Lucide icon. **[Medium]**

## Severity ladder

- **Blocker** — broken or unusable as shipped. Unreadable text, a surface that
  clips its own content, a control that cannot be reached, a removed animation
  that a dismiss depends on, or an AA miss in new UI.
- **High** — a clear rule or token violation a reader would notice. A raw colour
  in feature code, a missing empty or error state, a broken tab and panel pair,
  an ambient loop that survives reduced motion, an AA miss on an existing
  screen.
- **Medium** — real but contained. A hand-rolled row, an ad-hoc radius, a
  missing `tabular-nums`, Title Case in a button, an emoji in chrome.
- **Nitpick** — polish. A few pixels of alignment, a slightly loose gap.
