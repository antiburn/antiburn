# HUD LED: blink rate follows dollars per minute

_Plan + handoff. Designed on branch `claude/hud-led-spend-rate-d25667`, 2026-09-16,
then moved here because the token-map work on this branch already owns most of
the machinery. Implemented in PR #565 (`feat/hud-v2`)._

## Handoff — read this first

I designed this as a standalone feature, then found `hud_token_map.rs` on this
branch and rewrote Part 1. **Most of what a spend rate needs already exists
here.** What does not exist is the two fields that make pricing possible, and
they are cheapest to add now, while `modes.rs` is fresh, rather than as a
migration later.

| Question                              | Answer                                                            |
| ------------------------------------- | ----------------------------------------------------------------- |
| Build a new sampler?                  | **No.** `get_hud_token_map` is the sampler.                        |
| What blocks pricing?                  | `ModeSample` carries neither the model nor the token *kinds*.      |
| How big is the unblock?               | Two fields on one struct, plus the call site. Small.               |
| Does anything else need to change?    | One thing worth verifying — see "A thing to check" below.          |

Status of the design questions: one decided by Keith (fast = concerning), three
still open. They are at the bottom.

## The idea

Today the HUD's last lit segment blinks at a fixed 3s while a session is live
(`led-blink` in `src/styles/hud.css`, `blinkLast` in `LedBar.tsx`). It says one
bit: something is running.

Make the blink **period** the inverse of spend: the more dollars per minute the
machine is burning, the faster the LED flashes. A quiet session ticks like a
heartbeat. A parallel fleet of Opus agents strobes.

This sits alongside the token map, and the two answer different questions. The
map answers "what are my agents doing" and you have to look at it. The LED
answers "should I care right now" from the corner of your eye. The map is a
reading; the LED is an alarm.

## Part 1 — where the number comes from

### What this branch already built

`get_hud_token_map` (`apps/desktop/src-tauri/src/hud_token_map.rs`) is, almost
exactly, the sampler a spend rate needs:

| Need                                  | Already there                                              |
| ------------------------------------- | ----------------------------------------------------------- |
| A rolling window                      | `DEFAULT_WINDOW_SECS = 300` — the same 300s I had picked     |
| Per-session rate                      | `tokens_per_min`, parent and each sub-agent separately        |
| Cheap polling                         | Cached per `fingerprint_with_subagents`; only changed transcripts re-parse |
| Bounded cost                          | `MAX_SESSIONS = 64`, `KEEP_SECS = 3600`                       |
| Rides the HUD's existing tick         | The module docs say the HUD polls it on the liveness tick     |

So: no new command, no new poll, no new cache. A spend rate is a second
aggregation over samples this branch already collects.

### The one blocker

`ModeSample` (`crates/antiburn-local/src/analysis/modes.rs:57`) is:

```rust
pub struct ModeSample {
    pub ts_ms: Option<i64>,
    pub mode: WorkMode,
    pub tokens: u64,     // added_tokens(): effective input + output
    pub source: EventSource,
}
```

`tokens` cannot be converted into dollars, and not approximately either. Three
reasons, each independently fatal:

1. **No model.** Opus and Haiku differ by more than 10×. Without the model, a
   token count is not a price, it is a price *range* two orders of magnitude
   wide.
2. **The kinds are flattened.** Output costs roughly 5× input. `added_tokens`
   adds them together, so the split is gone by the time the sample exists.
3. **Cache reads are dropped.** `added_tokens` deliberately excludes them, which
   is right for "how much work did this turn add" and wrong for "what did it
   cost". Cache reads are cheap per token and enormous in volume — on a long
   session they are a real share of the bill, and here they are invisible.

### The fix

Widen the sample at the point where the information still exists:

```rust
pub struct ModeSample {
    pub ts_ms: Option<i64>,
    pub mode: WorkMode,
    pub tokens: u64,            // unchanged — the map keeps using this
    pub source: EventSource,
    pub model: Option<String>,  // new: NormalizedEvent already has it
    pub usage: Usage,           // new: the unflattened four-way split
}
```

Both are already on `NormalizedEvent` at the call site, so this is a widening,
not a new derivation. `tokens` stays exactly as it is and the token map does not
change behaviour — worth keeping precisely so the map's tests stay honest.

A turn that splits across several modes splits `tokens` evenly today. **Do not
split `usage` the same way.** Price the turn once, against the turn's own model,
and attribute the dollars to modes afterwards if a per-mode cost is ever wanted.
Splitting first and pricing after would price each fragment separately and
invite double counting on the cache-write tier.

### The rate

```
usd  = Σ price(sample.model, sample.usage) over samples inside the window
rate = usd / (window_secs / 60)
```

with `pricing::calculate_cost` and the bundled catalog. Divided by the **fixed
window**, not by elapsed-since-first-sample — that is what makes the LED decay:
stop working, the samples age out, the blink slows back to a heartbeat.

Carry the share that could be priced:

```rust
pub struct HudSpendRate {
    pub usd_per_minute: f64,
    pub window_secs: u32,
    /// Priced tokens over total tokens in the window, 0.0–1.0.
    pub priced_share: f64,
}
```

`priced_share` is load-bearing, not diagnostics. See Part 3.

## Part 2 — the mapping

Linear inverse (`period = k / rate`) is unusable: unbounded at both ends, and it
spends its whole resolution on the cheapest tenth of the range. Use a
**geometric** map between two anchors, in both rate and period:

```
FLOOR  = $0.05/min     at or below → PERIOD_SLOW
CEIL   = $2.00/min     at or above → PERIOD_FAST
PERIOD_SLOW = 3000ms   (today's value: a quiet session is unchanged)
PERIOD_FAST =  300ms   (3.3 Hz)

t      = clamp(ln(rate/FLOOR) / ln(CEIL/FLOOR), 0, 1)
period = PERIOD_SLOW * (PERIOD_FAST/PERIOD_SLOW)^t
```

Then **quantise to 8 geometric rungs** (3000, 2100, 1480, 1040, 730, 510, 360,
300 ms):

- WebKit restarts a CSS animation when `animation-duration` changes; rungs keep
  that to a few times a session instead of every poll.
- It reads as a gear change, not as noise.
- It is a pure function of one number, so it tests in three lines.

### Flash safety

WCAG 2.3.1's general flash threshold applies to flashes covering more than 25%
of a 10° visual field. The LED is a 6 px dot, far below it, and 3.3 Hz is the
bottom edge of the 3–60 Hz band the guidance is about. Do not raise
`PERIOD_FAST` past this without revisiting the number.

### Reduced motion

`design.md` is explicit: "An ambient loop stops instead of shortening: no
duration makes a loop acceptable." So under `prefers-reduced-motion: reduce` the
LED does exactly what it does today — `animation: none`, sitting solid.

That leaves a reader with no rate at all, which is why stating the rate in words
in the hover detail window is **in scope, not a follow-up**. The motion is the
ambient glance; the words are the fact.

## Part 3 — the degradation ladder

The HUD picks the first rung that holds:

| Condition                                   | LED                                 |
| ------------------------------------------- | ----------------------------------- |
| Live, priced samples in the window          | Rate-mapped period (300–3000 ms)    |
| Live, nothing priced, live-usage available  | `forecast.consumptionRate` (% of allowance per hour) on the same 8 rungs |
| Live, nothing measurable                    | Fixed 3000 ms — today's behaviour   |
| Not live                                    | No blink — today's behaviour        |

Rung 2 is a real consolation prize, not a fudge: `consumptionRate` is already in
the `getLiveUsage` payload the HUD polls at 60s, and for a subscription user
percent-of-allowance-per-hour is close to what they actually care about. It is
just not dollars, so it sits below dollars.

**An unpriced model must never fall to the slow end.** Since fast means
"concerning", slow is a claim of safety — and we would be making it about a
model we could not price. That is what `priced_share == 0.0` routes around.

## Part 4 — wiring

### Rust

Add the spend rate to the token map's payload rather than to a new command. It
is the same samples, the same window, the same tick:

```rust
pub struct HudTokenMapPayload {
    pub now_epoch: i64,
    pub window_secs: u32,
    pub sessions: Vec<HudTokenMapSession>,
    pub spend: Option<HudSpendRate>,   // new: summed across every session
}
```

Summed across sessions and sub-agents, because the LED is one dot and the
question is "what is this machine costing me", not "what is this session
costing me".

### Frontend

- `src/lib/ledPeriod.ts` — pure: `ledPeriodMs(rate: number | null): number | null`,
  holding the anchors and the 8 rungs. Fully unit tested.
- `OverlaySession.ts` — add `blinkPeriodMs` to `OverlaySnapshot`, off the token
  map response the tick already fetches. No new timer.
- `LedBar.tsx` — new optional `blinkPeriodMs`; when set, the blinking span also
  carries `"--led-period": \`${ms}ms\``.
- `hud.css` — `.led-blink { animation: led-blink var(--led-period, 3s) steps(1, end) infinite; }`.
  The fallback keeps every existing caller unchanged.
- `HudDetailView.tsx` — one line stating the rate in words.

No `useEffect` is added: the period is derived during render from the snapshot
`useSyncExternalStore` already supplies.

## A thing to check (found while reading, not verified by running)

`get_hud_token_map` gets its session list from `store.recent_sessions(...)` —
the store, which only the scan writes. `scan.rs` says scheduled passes are
"paused entirely while the popover is hidden", and the HUD does not trigger one.

If that reading is right, then with the popover closed a **brand-new** session
never enters the store, so the token map cannot see it. Sessions already in the
store keep updating, because the fingerprint check goes to disk directly.

The sharp edge: the existing blink calls `latest_session_activity`, which goes
straight to `discover_recent_sessions` on disk and bypasses the store entirely.
So the LED would blink for a session the token map shows as nothing — and once
the LED's *rate* comes from the token map, it would blink at the slow,
"everything is fine" end while a fresh agent burns.

Worth ten minutes with the popover closed and a new session started before
building on top of it. If it holds, the fix is probably to seed the list from
`discover_recent_sessions` like the liveness path does, rather than to unpause
the scan.

## Tests

**Rust**
- `modes.rs`: the widened sample carries the turn's model and the unsplit
  `Usage`; `tokens` is unchanged for every existing case.
- Pricing: a turn priced once, not once per mode. An unpriced model lowers
  `priced_share` and does not inflate `usd`.
- Window: a sample older than the window drops out and the rate falls.
- Cache reads contribute to cost while staying out of `tokens`.

**TypeScript** (`ledPeriod.test.ts`)
- Below floor → 3000. Above ceiling → 300. Geometric midpoint → the middle rung.
- `null` rate → `null` period (the caller then uses the CSS fallback).
- `LedBar` sets `--led-period` only on the blinking segment.

Plus the formatter, linter, type check and Rust tests from `CONTRIBUTING.md` and
`apps/desktop/README.md`. Commits DCO-signed (`git commit -s`).

## Docs to update in the same change

- `docs/hud-states.md` — the blinking note currently says only that the first bar
  blinks during a live session. It needs the rate, the ladder, and the
  reduced-motion behaviour.
- `apps/desktop/design.md` — `hud.css` is already in `sources:`; the motion
  section gains the LED period range and the flash-safety reason for the cap.

## Size

Much smaller than it was before the token map existed: roughly 250–350 lines net,
most of it the mapping and its tests. One PR, stacked on the token-map PRs. Needs
a screenshot — a GIF of the LED at two rates sells it better than a still.

## Decisions

1. **Fast means concerning** (Keith, 2026-09-16). A faster LED says "this
   deserves your attention", not "look how much work is happening". So the scale
   is an alarm scale: the resolution belongs where a reader would act, and the
   slow end is a claim of *safety*. Two things follow, both already above and
   both load-bearing:

   - The unpriced case must never fall to the slow end (Part 3).
   - The fast end stays inside the flash-safety cap (Part 2). An alarm that makes
     the HUD unpleasant to have on screen gets switched off, and then it warns
     nobody.

   The LED carries this in rhythm alone. No colour shift toward `--color-burn` at
   the fast end: the bar colour already means "how much allowance is gone", and
   giving one dot two meanings makes both harder to read at a glance. Revisit
   only if rhythm turns out to be too quiet a signal in practice.

## Open questions

1. **Anchors.** $0.05 and $2.00 per minute are judgement, not measurement. Now
   that fast means "concerning", the ceiling should sit where *Keith* would want
   to be interrupted, not at a round number. Worth sampling a few real days of
   his own transcripts first.
2. **A setting?** "LED pulse: steady / spend rate", defaulting to spend rate.
   Cheap to add, but nobody has asked for it yet.
3. **Window length.** 300s matches the token map's default, which is a good
   reason to keep it. Easy to retune after living with it.
