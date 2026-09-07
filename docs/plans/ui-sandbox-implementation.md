# UI sandbox: implementation plan

_Plan. Branch `claude/antiburn-ui-sandbox-759ec8`. 2026-09-07._

Builds what [ui-sandbox.md](ui-sandbox.md) decided: personal tool, popover first, Send button batches changes to Claude, adding things is an Ask.

## Status

| Step                                              | State       |
| ------------------------------------------------- | ----------- |
| 0. Prep: install deps in this worktree            | done        |
| 1. Sandbox mode in the repo (fixture IPC, page)   | done, in PR |
| 2. `/sandbox` skill: overlay with Draw + Ask, Send | not started |
| 3. Arrange mode                                   | not started |
| 4. Later: multi-view page, real-data snapshot     | parked      |

## What gets built, in one picture

```text
Browser pane (Chromium, inside Claude desktop)
  http://127.0.0.1:1420/sandbox.html?scenario=busy
    ├─ the real popover, React + Tailwind, fed by fixtures
    └─ overlay.js toolbar: Draw · Ask · Arrange · [Send] [Copy]
                │ POST /send on Send
                ▼
  receiver.mjs (node, port 4680)  ──append──▶  sandbox-sends.jsonl
                                                     │ tail -f
                                                     ▼
                                              Monitor task wakes Claude
                                              Claude screenshots the pane, edits source, HMR updates the page
```

Two halves. Step 1 is a small PR in this repo. Steps 2 and 3 live in a global skill at `~/.claude/skills/sandbox/` and never ship.

## Step 0: prep

This worktree has no `node_modules`. Run `pnpm install` from the repo root before anything else.

## Step 1: sandbox mode in the repo

### How the seam works

Product code imports `invoke`, `isTauri` and `listen` straight from `@tauri-apps/api/*` in ten files. Rather than touch any of them, a Vite mode swaps the packages:

- New script `dev:sandbox` = `vite --mode sandbox`.
- In `vite.config.ts`, when `mode === "sandbox"`, add `resolve.alias` entries:
  - `@tauri-apps/api/core` → `src/sandbox/tauri-core.ts` (exports `invoke`, `isTauri: () => true`)
  - `@tauri-apps/api/event` → `src/sandbox/tauri-event.ts` (exports `listen`; keeps handlers in a map; exposes `window.__sandbox.emit(name, payload)`)
  - `@tauri-apps/plugin-dialog` → `src/sandbox/tauri-dialog.ts` (`confirm` → `window.confirm`, `save`/`open` → `null`)

The production build and the Tauri dev build never see the alias, so nothing ships. This replaces the `?sandbox=1` runtime check from the discussion doc: a runtime check would touch `ipc.ts` and all its call sites, an alias touches nothing.

### Fixtures

- `src/sandbox/fixtures.ts`: lift the fixture set from `PopoverView.test.tsx` (settings, scan status, activity entries, provider usage, live usage, storage health) and the rich session analysis from `PopoverSession.test.ts`. Export factories, typed against the `ipc.ts` payload types. A shell-side rename then fails `type-check`, which is the same protection the tests give today.
- `src/sandbox/scenarios.ts`: named sets picked by `?scenario=`. Two for v1:
  - `default`: the test set, one session.
  - `busy`: a dozen sessions across claude-code, codex and cursor, one active, one flagged high-cost, a scan that finished two minutes ago, both providers with live usage.
- `src/sandbox/commands.ts`: the `invoke` switch. Same shape as `mockCommands` in the test. Unknown commands resolve to `null`, matching the test's default.
- Timestamps are computed relative to `Date.now()` at load so "3 minutes ago" stays true.

Optional, if it is cheap on the day: make `PopoverView.test.tsx` import its fixtures from `src/sandbox/fixtures.ts` so there is one copy. Skip if the test needs its own overrides in more than a couple of places.

### The page

- `apps/desktop/sandbox.html`: same as `index.html` plus an inline `<style>` that sizes `#root` to the popover's 380px width and a 700px max height on a neutral page background, and a small inline loader that fetches `http://127.0.0.1:4680/overlay.js` and appends it when the receiver answers. Silent when it does not.
- Vite serves any root HTML file in dev. The file is not added to `build.rollupOptions.input`, so it is not bundled.
- Inline styles in an HTML file are not a stylesheet, so `design.md` and the drift check are untouched.

### Housekeeping

- `knip.json`: add `src/sandbox/tauri-*.ts` to `entry`. They are reached only through the alias, and knip would otherwise flag them unused.
- No `useEffect` anywhere. The shims are plain modules.
- Comments in Simplified Technical English, as in `AGENTS.md`.
- One paragraph in `apps/desktop/README.md` under Commands: what `dev:sandbox` is and that it is a development aid.

### Done when

- `pnpm --filter @antiburn/desktop dev:sandbox`, open `sandbox.html?scenario=busy` in the Browser pane, the popover renders with rows, usage bars and a session detail that opens.
- `lint`, `type-check`, `test`, `knip`, `build` all pass. `build` output is unchanged (no alias in production mode).
- One PR, target under 500 lines, `git commit -s`. Keith uploads the screenshot.

## Step 2: the `/sandbox` skill, Draw + Ask + Send

Lives at `~/.claude/skills/sandbox/`, next to `/proto`. Three files.

### `overlay.js`

Vanilla JS, one file, no build. Injected by the loader in `sandbox.html`. Adds a floating toolbar and:

- **Select.** Hover highlights the element under the cursor and shows its React component name. Found by reading the `__reactFiber$…` key on the DOM node and walking up to the nearest named function component. The source file comes from the fiber's debug stack in React dev builds. Verify this on the first build; fallback is component name plus the CSS path.
- **Draw.** A full-page canvas above the popover. Pen, arrow, box, text label, undo. Strokes are kept as SVG paths in page coordinates so they survive a Send.
- **Ask.** A note box tied to the selected element, or to the page when nothing is selected. "A chart of daily burn here" is an Ask.
- **Change list.** A panel listing every action so far. Each entry can be removed.
- **Send.** POSTs the change list to the receiver. Clears the list on success, leaves drawings on screen.
- **Copy.** Same block as text on the clipboard, for the no-receiver case.

Payload per Send:

```json
{
  "sentAt": "2026-09-07T01:12:00Z",
  "scenario": "busy",
  "viewport": { "width": 380, "height": 700 },
  "actions": [
    { "kind": "ask", "target": "SessionRow[2]", "source": "src/components/session/SessionRow.tsx", "text": "move the cost pill left of the time" },
    { "kind": "hide", "target": "UsageLimitsBar", "source": "src/components/providerUsage/UsageLimitsBar.tsx" }
  ],
  "strokes": [ { "tool": "arrow", "path": "M 40 120 L 200 120", "label": "" } ]
}
```

### `receiver.mjs`

Node, no dependencies. `node receiver.mjs --port 4680 --log <file>`.

- `GET /overlay.js` serves the overlay.
- `POST /send` appends one JSON line to the log. CORS open to localhost.
- `GET /latest` returns the last line, so a second tab can replay strokes.

### `SKILL.md`

The run book Claude follows:

1. Check `dev:sandbox` is up on 1420. If not, ask Keith to start it, or start it through `preview_start` with a launch config.
2. Start the receiver in the background with the log in the session scratchpad.
3. Start a `Monitor` that tails the log. One line per Send, one wake per line.
4. Open `sandbox.html?scenario=…` in the Browser pane.
5. On each wake: read the line, screenshot the pane (drawings are still on it), restate the asks in one line each, make the edits, say what changed. HMR shows the result in the pane.
6. On "done": stop the Monitor and the receiver, summarise the session's asks and which landed.

### Which browser

v1 assumes Keith draws in the Browser pane inside Claude desktop. It is Chromium, he can click in it, and Claude's screenshot then carries the drawings for free. If Safari turns out to be the preference, the `/latest` endpoint plus a `?replay=1` flag on the overlay lets Claude's tab redraw the strokes before it screenshots. Small, so it is a follow-up rather than a v1 item.

### Done when

- Draw an arrow, add an Ask, press Send. Claude wakes once, screenshots, reads the ask, edits the source, the pane updates.
- Copy produces the same content as text.

## Step 3: Arrange mode

The fragile part, kept separate so steps 1 and 2 are usable without it.

- **Reorder.** Drag an element among its siblings. The DOM move is visual only; the action records `move SessionRow[2] before SessionRow[0]` with component names and sources.
- **Hide.** Click with the hide tool. Sets `display: none`, records `hide UsageLimitsBar`.
- **Resize.** Drag an edge. Records the new width or height in pixels.
- **Undo** per action, and the change list removes the visual effect when an entry is deleted.
- A React re-render from a fixture event will undo a DOM move. Static fixtures make that rare, and the intent is already recorded.

### Done when

- Reorder two rows, hide the usage bar, Send. Claude's edit produces the same arrangement from source.

## Step 4: parked

- Multi-view page: `sandbox.html?view=settings|overlay|nudge` plus a wrapper that iframes two or three views side by side. Needs shims for `@tauri-apps/api/window` and `webviewWindow`.
- Real-data snapshot: a command that dumps what the engine sees to a fixture file. Needs a Rust-side command. Not before the fixture mode has proved useful.

## Risks

- **Fiber source lookup.** React 19 removed `_debugSource`. Owner stacks exist in dev builds but their shape is not stable. The component name alone is enough for Claude to find the file with a search, so this only affects convenience.
- **Fixture drift.** Every new IPC payload field must be added to the fixtures. Typing them against `ipc.ts` turns drift into a `type-check` failure rather than a blank popover.
- **Lazy chunks.** Session detail is a lazy chunk. The sandbox loads it on click, same as the app, so nothing special is needed.

## Checks before each PR

```bash
pnpm --filter @antiburn/desktop lint && pnpm --filter @antiburn/desktop type-check && pnpm --filter @antiburn/desktop test && pnpm --filter @antiburn/desktop knip && pnpm --filter @antiburn/desktop build
```

Plus `pnpm run slop:all` and `pnpm run secrets` before pushing, per `CONTRIBUTING.md`.
