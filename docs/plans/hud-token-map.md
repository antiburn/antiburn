# HUD token map: what are my agents doing right now

_Plan. Branch `claude/token-usage-breakdown-f921e8`. 2026-09-15._

Proto lineage: `token-map-v1` … `v6` (scratchpad). Direction picked: v6, the
dot-blob square. One blob per live session, one dot per unit of tokens/min,
each dot coloured by the mode of work it paid for, smaller dots for sub-agents.

## Status

| Step                                              | State       |
| ------------------------------------------------- | ----------- |
| 0. Plan reviewed, open questions decided          | done        |
| 1. Engine: turn → mode attribution + tests        | done        |
| 2. Desktop Rust: `get_hud_token_map` command      | done        |
| 3. Frontend: `deriveTokenMap` layout + tests      | done        |
| 4. Frontend: `TokenMap` SVG in the HUD + setting  | in progress |
| 5. Detail window: legend + per-session rows       | not started |
| 6. Docs (`hud-states.md`, `design.md`), slop pass | not started |

Steps 1–2 ship as one PR (Rust only, dark). Steps 3–6 ship as a second PR
stacked on the first, so each review stays under ~1k lines.

## The problem

The HUD answers "how much quota is left" and "is a session live" (the blinking
LED). It does not answer the question Keith keeps asking when the LED blinks:
**my agent is busy, but what the heck is it doing, and how hard is it burning?**

Today the only way to find out is to open the popover, find the session, and
read the tool mix, which is a session-lifetime total, not "the last few
minutes".

## The proposal

A small square above the LED bars. It ticks once a turn lands, not
continuously, because tokens are paid per turn.

```
┌──────────────────────┐
│   ●●●●●    ●●        │  one blob per live session
│   ●●●●●●   ●●●       │  dot count  = tokens/min over the window
│   ●●●●●● ·· ●        │  dot colour = mode (look / run / change / think …)
│    ●●●●  ··          │  small dots = a sub-agent of that session
│                      │
│ ▮▮▮▮▮▮▮▮▮▮▮▮▮▮▯▯▯▯▯▯ │  quota bars (unchanged)
│ ▮▮▮▮▮▮▮▮▯▯▯▯▯▯▯▯▯▯▯▯ │
└──────────────────────┘
```

Encoding, from the v6 protos:

| Channel     | Means                                     | Source                                     |
| ----------- | ----------------------------------------- | ------------------------------------------ |
| blob        | one session (parent transcript)           | discovery: sessions written in the window  |
| blob frame  | session identity (Y3) _or_ hue (Y6)       | stable index per session key               |
| dot count   | tokens per minute over the window         | per-turn `usage`, bucketed by `ts_ms`      |
| dot colour  | mode of the turn that paid for it         | `tools[].category`, `has_thinking`, role   |
| dot radius  | parent (full) vs sub-agent (small)        | `EventSource`                              |
| dot pulse   | the newest turn                           | max `ts_ms`                                |
| blob order  | busiest first, top-left                   | sorted by rate                             |

### Modes (the "7 things", mapped onto what the engine already knows)

| Mode         | Rule on one assistant turn                                        |
| ------------ | ----------------------------------------------------------------- |
| looking      | any tool in `Read` or `Search`                                    |
| running      | any tool in `Bash` or `Test`                                      |
| changing     | any tool in `Edit`                                                |
| delegating   | a `Task` / `Agent` tool call, or every turn with `source: Subagent` |
| thinking     | no tools, `has_thinking`                                          |
| talking      | no tools, no thinking (plain assistant text)                      |
| other        | tools only in `Other` (MCP, skills, web)                          |

A turn with several categories splits its tokens evenly across them. This is
a first cut; a later pass can weight by tool-result size.

"Standing overhead" (the re-paid cache) is deliberately not a mode here. It
is a different question ("why does every turn cost so much") and variant G
from the v1 protos is the candidate for a second view later.

### What a "token" is

`output + input + cache_creation` per turn: the tokens the turn *added*.
Cache reads are excluded, matching `tokens_in` in `SessionMetrics` and the
Tokens header. Cost is not shown; the HUD stays unit-free.

## Data

### What exists

- `NormalizedEvent` carries everything needed per turn: `ts_ms`, `usage`,
  `tools[].category`, `has_thinking`, `source`, `role`
  (`crates/antiburn-local/src/analysis/model.rs`).
- `Explorers::DISK.discover_recent_sessions(now, ACTIVE_SESSION_WINDOW_SECS)`
  lists transcripts written recently (used by `hud.rs` for the LED).
- Sub-agent transcripts are merged and tagged by `merge_subagent_events`.
- No IPC payload exposes per-category **tokens**; the engine keeps
  `tool_calls_by_name` counts only. `SessionEfficiency` is cost by kind, not
  by mode.

### What is new

**Step 1 — engine (`crates/antiburn-local/src/analysis/modes.rs`).**
`pub fn mode_samples(events: &[NormalizedEvent]) -> Vec<ModeSample>` where
`ModeSample { ts_ms, mode: WorkMode, tokens: u64, source: EventSource }`.
Pure, tested with the fixtures the engine tests already use. `WorkMode` is a
new enum; `ToolCategory` stays unchanged.

**Step 2 — desktop (`apps/desktop/src-tauri/src/hud_token_map.rs`).**
`#[tauri::command] get_hud_token_map(window_secs: u32) -> HudTokenMapPayload`:

```
HudTokenMapPayload {
  now_epoch: i64,
  window_secs: u32,
  sessions: [ {
    agent, session_id, title, started_at_epoch, last_turn_epoch,
    tokens_per_min: f64,
    modes: { looking: u64, running: u64, … },          // parent tokens in window
    subagents: [ { subagent_id, tokens_per_min, modes } ],
  } ]
}
```

Cost control: parsing a whole transcript on a 5 s poll is not acceptable for
long sessions. The module keeps a per-session cache keyed by
`(path, mtime, len)` and re-parses only when the file changed, the same
mtime-gating `get_session_analysis_fingerprint` relies on. First cut uses the
existing full parse; if the biggest transcripts still hurt, the follow-up is
a tail cursor (byte offset) that parses appended records only. Measure before
building the cursor.

Poll cadence: the HUD already polls liveness every 5 s; the map polls on the
same tick. Later the two can collapse into one command.

## UI

**Step 3 — `apps/desktop/src/lib/tokenMap.ts`.** A pure
`deriveTokenMap(payload, opts) -> TokenMapLayout` that turns the payload into
positioned dots. The square is fixed; the dot value is not. The function picks
the smallest dot value (from a ladder: 250, 500, 1k, 2k, 5k, 10k tokens/min…)
at which every blob fits in the square, so a quiet map shows fine grain and a
burst shows coarse grain, never an overflow. There is no session cap: more
sessions push the ladder up the same way a burst does. A session whose rate
rounds to zero dots at the chosen value still gets one dim dot, so it is not
lost. The chosen value goes to the
detail window ("● = 2k tokens/min") so the map stays honest. Dots ordered by
mode so a blob reads as bands, not noise; blobs packed busiest-first. Vitest
covers the ladder choice, ordering, empty, and the one-huge-session case.

**Step 4 — `apps/desktop/src/components/ui/TokenMap.tsx`.** An SVG the width
of the LED bars, rendered by `OverlayWindow` above the bars. The HUD grows
upward when the map appears; the window is content-sized, so the shell handles
the resize. Data flows through
`OverlaySession` (a `tokenMap` field on the snapshot, polled with liveness).
No `useEffect`: the session class owns the timer, the component derives.
Hidden when no session wrote in the window, so an idle HUD looks as it does
today. Behind a new setting `floating_hud_token_map` (default on) next to the
existing floating-HUD toggle.

Mode colours are new tokens in `hud.css` (`--color-mode-looking` …), light and
dark, listed in `design.md`. Values start from the v6 proto palette
(system-blue, system-green, brand-tint, system-indigo, system-gold, tertiary).
Reduced motion disables the newest-turn pulse.

**Step 5 — detail window.** `HudDetailView` gains a legend (mode → colour) and
one row per session: title, tokens/min, top mode. Pushed via the existing
`HudDetailState` payload.

**Step 6 — docs.** `docs/hud-states.md` gets the map states (idle, one
session, many, sub-agents), `design.md` the tokens. Run fmt, clippy, eslint,
tsc, vitest, cargo test.

## Open questions

1. ~~Y3 or Y6?~~ Decided: Y3 (dot colour = mode, thin session-coloured
   frame). Y6 is a one-function swap in the layout if Y3 reads wrong.
2. ~~Placement.~~ Decided: above the bars in the always-on HUD.
3. ~~Dot value.~~ Decided: dynamic, so the map always fits the fixed square.
   Window length (5 minutes) is still a guess.
4. ~~Token definition.~~ Decided: added tokens only, cache reads excluded.
5. ~~Max sessions.~~ Decided: no cap and no fold; the dot-value ladder keeps
   stepping up until every session fits.

## Decisions

- Variant: Y3 (Keith, 2026-09-15).
- Sessions: no cap, no "+N" fold; the ladder keeps scaling (Keith, 2026-09-15).
- Tokens: added tokens only (input + output + cache writes), no cache reads; matches the Tokens header (Keith, 2026-09-15).
- Dot value: dynamic ladder, chosen so every blob fits the fixed square (Keith, 2026-09-15).
- Placement: above the LED bars in the always-on HUD (Keith, 2026-09-15).
