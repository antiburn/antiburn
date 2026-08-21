---
name: design-review
description: Review one antiburn desktop window against the antiburn design system. Captures the live window per surface and per theme, then reports ranked design findings (Blocker/High/Medium/Nitpick) tied to the tokens and rules in apps/desktop/design.md. Use when asked to design-review a window, pane, or surface, run a design pass, critique the UI, or check the desktop app against the design system.
disable-model-invocation: true
---

# antiburn design review

An AI design reviewer for the antiburn desktop app. It looks at a live window,
checks it against the design system, and returns a ranked list of what to fix.
The method comes from OneRedOak's design-review agent. The rules are antiburn's
own.

Scope: the Tauri desktop app in `apps/desktop`. The app has four windows, and
each window is a fixed size. There is no responsive breakpoint pass, because
there are no fluid widths to break at. The equivalent pass here walks every
window, every surface inside it, and every theme.

## Before you start: you need a running app

This skill grades a live window, so the app must run.

- Ask the user which instance to review. Do NOT start a second one on your own.
- The app runs with `pnpm tauri dev` from `apps/desktop`.
- Port 1420 is strict (`vite.config.ts` sets `strictPort: true`, and
  `src-tauri/tauri.conf.json` points `devUrl` at `http://127.0.0.1:1420`). One
  instance holds it at a time.
- If another worktree holds 1420, do NOT kill the process. Ask the user. If the
  user wants a second instance, start it with `tauri dev --config <override>`
  and give the override its own port and `devUrl`.
- The tray popover hides when it loses key status. Ask the user to select
  **Pin Window** in the tray menu before you capture it. Unpinned, the popover
  disappears the moment another window takes focus.

Secondary path, with a limit: `pnpm dev:web` serves the same bundle in a
browser, and the URL fragment picks the view (`#/settings`, `#/nudge`,
`#/onboarding`; no fragment gives the popover). A browser has no shell, so
`hasShell()` is false and every view falls back to `DEFAULT_SETTINGS` and empty
data. Use this path for the accessibility tree, the console, the type ladder,
and the themes. Do NOT grade data-dependent states from it, and never report
"no data" as a finding when the shell is absent.

## Step 1: load the rulebook

Read `design-principles.md` in this skill folder. It numbers every rule. Each
finding must cite a rule number plus the token, class, or file it breaks.

## Step 2: capture the window, per surface and per theme

Pick ONE window per invocation. Capture each surface of it, then repeat the
worst surface in each theme.

**The surface pass.** Capture every surface the chosen window can show:

| Window | Size | Surfaces to capture |
|---|---|---|
| Tray popover | 380 wide; 700 tall, 780 on Usage | `activity`, `session`, `usage` (`lib/popoverHeight.ts`) |
| Settings | 960 × 680, fixed | 7 panes: General, Privacy, Notifications, Usage, Sources, Appearance, About |
| Onboarding | 680 × 480 | 5 steps: welcome, sources, repositories, scan, ready |
| Notification | 344 wide, always on top | resting and expanded (the card expands on hover) |

**The theme pass.** Repeat the surface that carries the most colour in each of
these four conditions:

1. Light.
2. Dark.
3. Reduced transparency. Every window and popover surface turns solid.
4. Reduced motion. The global clamp stops animation.

Switch light and dark from the app's own Appearance pane. Switch reduced
transparency and reduced motion in the operating system's accessibility
settings. In the browser path, emulate the media features from the developer
tools instead.

**How to capture.** The app is a native window, so use the computer-use
screenshot tools. Bring the window forward first. Ask the user to take a shot
for you if the capture path would steal focus from an unpinned popover. On the
browser path, use the browser tools for a screenshot, the accessibility tree,
and the console.

## Step 3: grade it

Walk these dimensions. Check each one against `design-principles.md`.

1. First impression and hierarchy. Does the eye land on the one thing the
   surface exists to say? Is anything inflated?
2. Information architecture. Does the content sit in the right window? Is the
   pane order right? Does every deep link land where it claims?
3. Surfaces and geometry. All surfaces of the window. Clipping, overflow, a
   surface that outgrows its fixed height, a scroll area with no edge treatment.
4. Colour and tokens. Semantic utilities only. No raw hex, no ad-hoc `rgb()`.
5. Typography. The `type-*` ladder, the settings type ladder, `tabular-nums` on
   figures, no hardcoded sizes.
6. Primitives. The real `ui/` components, and the `ui-*` classes under them.
7. State taxonomy. Empty, loading, error, permission-blocked, first-run.
8. Themes and materials. Light, dark, and reduced transparency.
9. Motion. Token durations, the reduced-motion clamp, ambient loops.
10. Accessibility (WCAG 2.1 AA). Contrast, keyboard-only focus, `role` and
    `aria` wiring, never colour alone.
11. Copy and voice. Plain, calm, present tense, sentence case.
12. Console and robustness. Errors and warnings in the webview console.

Grade the rendered window first. Then open the source to confirm a violation —
a raw hex value, an arbitrary duration, a hand-rolled row. A guess from code
alone is not a finding.

## Step 4: write the report

Use `report-template.md`. Give each finding a severity, a dimension, what is
wrong, the rule and token it breaks, a suggested fix, and evidence. Evidence is
a screenshot or a quoted class or value. Rank the most severe first.

Save the report to `docs/design-reviews/<surface>-<date>.md`. Create the folder
if it does not exist. `<surface>` names what you reviewed, such as
`popover-activity` or `settings-privacy`. Give the user the top fixes in chat.

Review only by default. Propose the fixes. Do not apply them unless the user
asks.

## The passes

Run one window, or one surface of a window, per invocation. This keeps the
context small and the findings specific. For a full sweep, work through this
order:

1. Popover, activity surface. The window most readers see most.
2. Popover, session analytics.
3. Popover, usage. The one surface that exceeds the default height.
4. Settings, pane by pane.
5. Onboarding, step by step.
6. The notification window, resting and expanded.

## Severity

Blocker, High, Medium, and Nitpick. `design-principles.md` defines each one at
the end.

Accessibility ratchet: on an EXISTING screen an AA miss is **High**, not
Blocker. New UI is held to AA from the start.

## Principles

- Live window first. Grade what the app draws, then confirm it in the code.
- Be specific. Tie every finding to a rule number and to a token or a file.
- Show evidence. Give a screenshot or a quoted class for each finding.
- Assume competence. State the problem, its impact, and a fix. Do not lecture.
