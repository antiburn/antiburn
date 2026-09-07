# UI sandbox: play with the real antiburn UI, then tell Claude what you did

Status: discussion, 2026-09-07. Nothing built yet. Implementation plan: [ui-sandbox-implementation.md](ui-sandbox-implementation.md). Decided so far: personal tool only, popover first, adding things is an Ask not a drop-in, changes reach Claude in batches on a Send button.

## The idea in one line

Open the real antiburn UI in a browser, move and hide things with the mouse, draw on it, and have Claude turn that into code changes.

## What already exists

Three of the four pieces are closer than they look.

| Piece | Today | Gap |
|---|---|---|
| See the real UI in a browser | `pnpm --filter @antiburn/desktop dev:web` serves the frontend on port 1420. | Every IPC call is gated on `hasShell()` and returns nothing outside Tauri, so the browser shows empty states. |
| Chat to Claude while looking at it | The Claude desktop app has a Browser pane. Claude can open the URL, screenshot it, read the DOM, and read the console. | Nothing in the page tells Claude what you did with the mouse. |
| Click-to-comment | `discuss` already does element-level comments on static HTML and pin comments on images. `/proto` uses this. | discuss renders files in a sandboxed iframe. It cannot point at a live dev server. No drawing. |
| Move and remove elements | Nothing. | Needs an overlay in the page. |

## Proposed shape

Two parts. One lives in the antiburn repo, one is a global skill like `/proto`.

### Part 1: sandbox mode in the repo (`?sandbox=1`)

A Vite-only mode that makes the browser build useful:

1. **Fixture IPC.** When the page loads with `?sandbox=1` and no shell, `ipc.ts` answers from a fixture file instead of returning null. The test files already hold complete `invoke` mocks (PopoverView.test.tsx alone has four). Lift the richest one into `src/sandbox/fixtures.ts` and reuse it in both places.
2. **Popover first.** The sandbox page is index.html with fixture data, at the popover's true size. Settings, the overlay HUD and the nudge are separate Tauri windows; a later `&view=` flag plus a wrapper page that iframes two or three of them side by side gives the multi-view case, so "put the sub menu into the main view" is something you can see.
3. **Real data option, later.** The Tauri debug shell already loads from the Vite server, so edits hot-reload in the real app. A fixture snapshot command ("dump what the engine sees now to fixtures.json") would give the sandbox your real sessions without a backend. Not needed for v1.

### Part 2: the overlay (a global `/sandbox` skill)

A small script the skill injects into the page. Vanilla JS, no framework, same rule as `/proto`. It adds a toolbar with three modes:

- **Arrange.** Hover shows the React component name and source file (React dev builds carry this). Drag to reorder siblings, click to hide, drag an edge to resize. Each action is recorded as intent, not as DOM: `move SessionRow[2] before SessionRow[0]`, `hide UsageLimitsBar`.
- **Draw.** A canvas over the page. Pen, arrow, box, text label. Strokes stay on screen so a screenshot captures them.
- **Ask.** A text box for the "no, bring the sub menu into the main view" kind of note, attached to whatever is selected. Adding something new ("a chart of daily burn here") is also an Ask; Arrange never inserts placeholders in v1.

### How it reaches Claude

Batched, on a Send button. Nothing leaves the page until you press it, so the CLI is not spammed with every drag:

1. The overlay keeps a running change list in the toolbar: arrange actions, notes, and the drawing strokes.
2. **Send** POSTs the list to a tiny local receiver the skill starts (a few lines of node). The receiver appends one JSONL line per Send.
3. A Monitor task tails that file, so each Send wakes Claude once, the same way a discuss comment does now. One Send is one turn.
4. On wake, Claude screenshots the page through the Browser pane with the drawings still on it, reads the change list for intent, and uses the component source names to know where to edit.

No Vite plugin, no repo change, no new discuss feature. A **Copy** button next to Send gives the same block as text for pasting into chat when the receiver is not running.

## What this is not

- Not a code generator. Dragging a row records "move A before B". Claude edits the source. The DOM move is visual only and a React re-render may undo it, which is fine with static fixtures.
- Not Claude Design or Figma. Those draw mockups. This is the real UI with the real design tokens.
- Not a discuss feature, at least not first. discuss would need a "live URL" mode plus drawing. Worth a ticket to codesoda if the overlay works and we want the comment threads.

## Prior art to check before building

Stagewise, react-grab, locatorjs and click-to-component all do the "select an element in a dev build and send it to an agent" part. Unverified which are alive and whether any do drag or draw. Worth one hour of looking before writing an overlay from scratch.

## Rough order

1. Fixture IPC for the popover. Half a day. Useful on its own: it makes `dev:web` show something.
2. Overlay with Draw and Ask, Copy for Claude. Half a day. Most of the value.
3. Arrange mode. A day. The fragile part.
4. Multi-view wrapper page and real-data snapshot. Later.

## Open questions

- ✋ Is this for you alone, or a dev tool that ships with the open-source repo? Changes how careful the fixture mode has to be. **Answered: just Keith for now. Reconsider if it proves out.**
- Is "one page with every window" right, or do you mostly want to work on the popover? **Answered: popover first, multi-view later.**
- Copy-paste hand-off first, or is the Monitor feed the thing you actually want? **Answered: live feed, but batched behind a Send button so it does not spam the CLI.**
- Should Arrange also let you *add* things (drop in a chart placeholder), or is add always an Ask? **Answered: Ask only for v1.**
