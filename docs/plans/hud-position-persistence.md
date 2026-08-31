# HUD position persistence, across launches and across displays

_Plan. Branch `feat/hud-position-persistence`. 2026-08-28._

## Status

| Step                                                | State                             |
| --------------------------------------------------- | --------------------------------- |
| 1. Store: `internal:hudPlacements` read/write       | done                              |
| 2. HUD crate: placement type, resolve, apply        | done                              |
| 3. Shell: pass placements into `open`, save on drag | done                              |
| 4. Display watcher: react to connect/disconnect     | done                              |
| 5. Frontend: record position when a drag settles    | done                              |
| 6. Tests + `docs/hud-states.md` + slop pass         | done                              |

Automated checks pass: 611 Rust tests, 923 frontend tests, clippy, type-check,
lint. Keith ran the eleven-step walkthrough on a real two-display desk on
2026-08-28 and it passed.

## The problem

The HUD has no memory of where it was put. Every `open` centres it at the top
of the **primary** monitor:

```rust
// crates/hud/src/lib.rs — open()
if let Ok(Some(monitor)) = window.primary_monitor() {
    let x = monitor_x + (monitor_width - OVERLAY_WIDTH) / 2.0;
    let y = monitor_y + OVERLAY_TOP_INSET;
    window.set_position(LogicalPosition::new(x, y))?;
}
```

Drag it somewhere better, quit, relaunch — it is back at the top centre of the
laptop screen. On a two-display desk that is the wrong screen entirely.

Two things are missing, and only one of them is "save the coordinates":

1. **Across launches** — nothing persists the position at all.
2. **Across display changes** — plugging and unplugging a monitor is a live
   event, not a launch. The HUD should follow the desk it is on: back to the
   laptop when the big screen goes away, back to the big screen when it
   returns, without being told again.

Today's on/off switch is stored in `localStorage`
(`antiburn.showFloatingHud`, `src/lib/overlayWindow.ts`). Position cannot live
there: Rust builds and positions the window before any webview mounts, so
reading the saved place from JS would mean a visible jump on every launch. The
position belongs in the SQLite store, which Rust can read at build time.

## The behaviour we want

Keith's walkthrough, and what each step demands:

| #     | Action                 | HUD             | What it needs                                             |
| ----- | ---------------------- | --------------- | --------------------------------------------------------- |
| 1     | Laptop only. Drag to A | at A            | save on drag end                                          |
| 2–4   | Quit, relaunch         | at A            | restore at window build, before reveal                    |
| 5     | Connect monitor 2      | stays at A      | a new display does not steal the HUD                      |
| 6     | Drag to B on monitor 2 | at B            | save per display, not one global point                    |
| 7     | Quit, relaunch         | at B            | restore picks monitor 2, not the primary                  |
| 8–9   | Disconnect monitor 2   | moves to A      | live reaction; fall back to a display still connected     |
| 10–11 | Reconnect monitor 2    | moves back to B | the fallback must **not** overwrite the preferred display |

Step 11 is the one that decides the data model. If the fallback in step 8 were
recorded as "the user is now on the laptop", reconnecting the monitor would
leave the HUD on the laptop and the walkthrough breaks. So:

> **Only a drag changes which display is preferred.** A fallback move borrows a
> display; it does not claim it.

## Design

### What is stored

One internal scalar in the existing `setting` table, alongside the other
`internal:` state rows (`Store::internal_value` / `set_internal_value`):

```
key:   internal:hudPlacements
value: {"version":1,"entries":[
         {"monitor":"LG HDR 4K|3008x1692@2","x":100,"y":40},
         {"monitor":"Built-in Retina Display|1512x982@2","x":668,"y":32}
       ]}
```

- **`entries` is ordered by recency, most recent first.** The head is the
  preferred display. A drag moves that display's entry to the head; nothing
  else reorders the list.
- **`x`/`y` are logical and relative to that monitor's own origin**, not to the
  global desktop space. Monitors move around in global space every time the
  arrangement changes; an offset from the monitor's own top-left does not.
- Capped at 8 entries, oldest dropped. A desk does not have nine monitors, and
  an unbounded list in a settings row is a slow leak.
- Unreadable or unparsable value is treated as empty, and the HUD uses today's
  primary-monitor default. A bad row must never stop the HUD appearing.

Preferences the user chose live in `AppSettings`; this is remembered state, so
`internal:` is the right namespace (`store/mod.rs` documents that split).

### Monitor identity

Tauri's `Monitor` gives a name, a physical size, a position, and a scale
factor. The key is `name|WxH@scale`, e.g. `Built-in Retina Display|1512x982@2`.

Position is deliberately excluded from the key — it is exactly the thing that
changes when displays are rearranged.

**Known limit:** two identical external monitors of the same model produce the
same key, so the HUD may pick the wrong one of the pair. Accepted for v1. The
fix, if it ever bites, is the macOS `CGDirectDisplayID` through `objc2` — the
HUD crate already links `objc2_app_kit`, so the door is open.

### Where the code goes

The HUD crate's own doc comment sets the boundary: it "creates, positions,
sizes, reuses, and shows" the window; "the desktop shell owns IPC policy". So
geometry goes in the crate, storage in the shell.

**`crates/hud/src/lib.rs`** gains:

```rust
pub struct Placement { pub monitor: String, pub x: f64, pub y: f64 }

/// Keys for every connected display, in the platform's order.
pub fn monitor_keys(app: &AppHandle) -> Vec<String>;

/// Where the HUD is now, as a key and an offset inside that display.
pub fn current_placement(app: &AppHandle) -> Option<Placement>;

/// Move the HUD to the first remembered display that is connected.
pub fn apply_placement(app: &AppHandle, entries: &[Placement]) -> tauri::Result<()>;

/// `open` takes the remembered entries so the window is placed before reveal.
pub fn open(app: &AppHandle, entries: &[Placement]) -> tauri::Result<()>;
```

and two pure functions under them, which are where the tests go:

```rust
fn resolve<'a>(entries: &'a [Placement], connected: &[String]) -> Option<&'a Placement>;
fn clamp_into(x: f64, y: f64, width: f64, height: f64, frame: &Frame) -> (f64, f64);
```

`clamp_into` keeps a remembered offset inside the display it lands on — a
monitor may come back at a lower resolution, and the HUD's height changes with
its content. It holds the frame fully on screen where it fits, and prefers the
top-left edge where it does not, matching the existing `clamp_detail` habit of
never panicking when `max < min`.

**`src/hud.rs`** (the shell's HUD policy module) gains:

- `load_placements(store) -> Vec<Placement>` and `save_placement(store, Placement)`,
  the serde and the recency reordering.
- `spawn_display_watcher(app)`.

**`src/commands.rs`** gains `record_hud_position`, which asks the crate where
the HUD is and hands the answer to the store. It takes no arguments: the
frontend knows _that_ a drag ended, the shell knows _where_ the window is, and
splitting it that way keeps geometry out of the IPC payload.

`open_overlay_window` loads the entries and passes them to `hud::open`.

### Reacting to display changes

Steps 8–11 happen while the app runs, so something has to notice.

**Poll `available_monitors()` every 2 seconds** from a task spawned at startup.
When the connected key set differs from the last poll, re-resolve and move the
HUD if the winning display changed. This matches the pattern already in the
same file — `spawn_hover_watcher` polls the cursor at 100ms — and needs no new
Objective-C plumbing.

The alternative is observing
`NSApplicationDidChangeScreenParametersNotification`, which is the correct
event rather than a sample of its effect. It is the upgrade if the poll shows
up in a profile; a 0.5Hz `NSScreen` query should not.

The watcher only ever _reads_ the stored entries. It never writes, which is
what preserves the step 11 behaviour.

### Saving on drag

`OverlaySession.settleDrag()` already runs at the end of every drag
(`views/overlay/OverlaySession.ts`). It gains one fire-and-forget call:

```ts
void recordHudPosition().catch(() => {});
```

with the wrapper next to the other HUD invokes in `lib/overlayWindow.ts`. No
`useEffect` — this is the event that caused the work, which is where AGENTS.md
says the work belongs.

## Open assumptions

Both are stated rather than asked, because either answer still ships:

1. **Connecting a display does not move the HUD** (step 5). The walkthrough
   shows the HUD staying on the laptop until it is dragged. Only a
   _disconnect_ of the preferred display forces a move.
2. **Identical twin monitors share a key** (see Monitor identity). The HUD may
   pick the wrong twin.

## Steps

1. **Store** — `internal:hudPlacements` constant, `load_placements`,
   `save_placement`, recency reorder, 8-entry cap, tolerant parse.
2. **HUD crate** — `Placement`, `monitor_keys`, `current_placement`,
   `apply_placement`, `resolve`, `clamp_into`; `open` takes entries and places
   the window before the renderer reveals it.
3. **Shell wiring** — `record_hud_position` command, registered in `lib.rs`;
   `open_overlay_window` passes the loaded entries.
4. **Display watcher** — spawned at startup, polls, moves, never writes.
5. **Frontend** — `recordHudPosition` wrapper, called from `settleDrag`.
6. **Tests and docs** — see below.

Each step builds on its own; step 5 is the first one where the walkthrough can
be run end to end.

## Tests

Rust, in the HUD crate, against the pure functions:

- `resolve` picks the head entry when its display is connected.
- `resolve` skips a disconnected head and takes the next connected entry —
  step 8.
- `resolve` returns `None` when no remembered display is connected, so the
  caller falls back to the primary-monitor default.
- `clamp_into` holds a frame inside a display that came back smaller.
- `clamp_into` prefers the low edge on a display narrower than the HUD.

Rust, in the shell:

- A drag on a display already in the list moves it to the head without
  duplicating it — step 6 after step 1.
- The list stays capped at 8, dropping the oldest.
- A malformed stored value parses as empty rather than failing.

Frontend, in `OverlayWindow.test.tsx`:

- A settled drag calls `record_hud_position` exactly once.
- A rejected `record_hud_position` does not break the session.

Manual, on the real desk — the eleven steps of the walkthrough. This is the
one that actually proves it; the unit tests only stop the pieces regressing.

## Docs

`docs/hud-states.md` describes HUD positioning today and needs the new rule.
No design tokens or stylesheets change, so `apps/desktop/design.md` is
untouched.
