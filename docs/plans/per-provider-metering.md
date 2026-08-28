# Show Meter — a per-provider switch

Status: **built 2026-08-27 — checks green, awaiting screenshots and PR**

| Piece | State |
|---|---|
| Setting (`liveUsageHiddenProviders`) + store round-trip | done |
| `collect` skips a hidden provider's source | done |
| `meters` roster on the summary payload and event | done |
| Usage pane: "Show Meter" section, one switch per provider | done |
| HUD empty track + "No meter selected." in the hover card | done |
| HUD takes the pushed summary, so a switch lands at once | done (fixed after first run) |
| Popover Usage view: "No meter selected." note | done |
| Checks: `cargo fmt`/`clippy`/`test` (567), `pnpm lint`/`type-check`/`test` (891)/`build`, `slop:all`, `secrets` | green |
| Screenshots + PR | open |

## What it does

Settings → Usage grows one switch per meterable provider (today: Claude and
Codex). Switch off, and that provider is gone: no meter in the popover, none in
the HUD, no rows in the Usage pane, and **no requests to that provider at
all**.

Not shown means not asked. There is no reason to spend a request and a
credential read on a number nobody will see.

## Decided in review

| Question | Decision |
|---|---|
| Does off stop the request, or only hide the row? | **Stops the request.** Keith: "Why would it keep happening? They should not." |
| Section title | **Show Meter** (replaces "What antiburn can currently see") |
| Where the meter disappears from | Popover / menubar meter **and** the HUD |
| HUD with nothing selected | Empty track, dimmed, no percentage — no strikethrough, no question mark. Popover says "No meter selected." Keith: "hud: fine." |
| Per-model switches | Not possible today — no source reports usage per model. Two provider rows is the whole list. |

## What is there today

`apps/desktop/src/views/settings/UsagePane.tsx:103` renders **results**: one row
per provider that came back in the last snapshot, plus error rows, plus an
empty state.

Behind it:

- Two registered live sources, both provider-level:
  `anthropic_fetch::ClaudeDirectFetch` (**Claude**) and
  `codex_fetch::CodexDirectFetch` (**Codex**) —
  `provider_usage/live/sources/mod.rs:50`.
- One gate for both, `AppSettings::live_usage_active()`, applied at
  `sources::collect(sources, online, max_age)`. Two callers:
  `provider_usage/live/mod.rs:192` (`summarize`) and `usage_alerts.rs:239`
  (the milestone monitor). Both already hold `settings`.
- Settings are a flat key/value table, every field read and written by hand in
  `store/mod.rs`.

## The one real complication

Once hiding stops the fetch, a hidden provider drops out of
`summary.providers` — so a list built from results would lose the row that
turns it back on. The section must render a **roster of meterable providers**
with live status layered on, not a list of results.

The roster is already knowable: `sources::registered()` gives the sources, each
knows its provider id, and `providers::display_name` gives the label. Add
`meters: [{ provider, displayName }]` to `LiveUsageSummary` /
`LiveUsageSummaryPayload`, built in `summarize()` from the `sources` slice it
already receives. One payload, one event, no second IPC command — the pane is
already subscribed.

## Design

### 1. The setting

New `AppSettings` field `live_usage_hidden_providers: Vec<String>`, stored as
one comma-separated key `liveUsageHiddenProviders` (same shape as the existing
`milestonePercentages*` keys). Storing the **hidden** set, not the shown one,
means a provider added by a later build is metered by default and an unknown id
left by an older database is harmless.

Reaches the webview as `liveUsageHiddenProviders: string[]`.

### 2. The gate

`sources::collect` takes the hidden set and skips a source whose provider is in
it, the same way it already skips on `!online`. Both callers pass it through.

Gating at `collect` is what makes one switch true everywhere: the pane, the
popover meter, the HUD and the milestone monitor all read the same snapshot, so
a hidden provider vanishes from all of them with no per-view work, and the HUD
needs no settings plumbing of its own (`OverlaySession` does not read settings
today, and this keeps it that way).

### 3. Two consequences the copy must state

Both follow from gating at `collect`, and both are why the row needs a sentence
rather than a bare label:

- **Milestone notifications for that provider stop.** `usage_alerts.rs`
  evaluates crossings from the same pass. No reading, no crossing, no "Codex at
  80%".
- **Its reading history gets a gap.** `history::record` appends per pass and
  forecasts are computed off that series, so a provider re-shown after a week is
  cold until it re-warms.

Row description when on: today's status line (`sourceLabel`, window count) or
the provider's error line. When off: "antiburn does not ask Codex for usage, and
its milestone notifications do not fire."

### 4. Empty states

`meters` and `providers` together tell the two cases apart, which a bare empty
list cannot:

| Condition | Copy |
|---|---|
| roster non-empty, every entry hidden | "No meter selected." — HUD popover and Usage pane |
| roster non-empty, none hidden, no readings | today's "No plan limits found" / sign-in prompt |

HUD with nothing selected: empty track, all segments off, dimmed, no
percentage. It should read as off-by-choice, not as failure.

### 5. The pane

One row per roster entry: provider display name, a `ToggleSwitch` trailing
(there is no checkbox in `components/ui`, and a switch is what every other row
in this pane uses), description per §3. Rows dim while the master switch above
is off, since it already overrides them.

## Files

| File | Change |
|---|---|
| `apps/desktop/src-tauri/src/store/model.rs` | new `AppSettings` field + default (empty) |
| `apps/desktop/src-tauri/src/store/mod.rs` | read/write `liveUsageHiddenProviders` |
| `apps/desktop/src-tauri/src/dto.rs` | `meters` on `LiveUsageSummary`; settings field |
| `apps/desktop/src-tauri/src/provider_usage/live/mod.rs` | build `meters`; pass the hidden set to `collect` |
| `apps/desktop/src-tauri/src/provider_usage/live/sources/mod.rs` | `collect` skips hidden providers |
| `apps/desktop/src-tauri/src/usage_alerts.rs` | pass the hidden set through |
| `apps/desktop/src/lib/ipc.ts` | payload types + defaults |
| `apps/desktop/src/views/settings/UsagePane.tsx` | section rename + roster rows |
| `apps/desktop/src/views/popover/UsageView.tsx` | "No meter selected" note above the cards |
| `apps/desktop/src/views/overlay/*` | HUD empty-track state |

## Tests

- Rust: `collect` never calls a hidden provider's source — fake source that
  records calls, asserted at zero.
- Rust: settings round-trip, including an unknown id and an empty value.
- Rust: `summarize` reports the full roster even when every provider is hidden.
- Rust: the milestone monitor records no crossing for a hidden provider.
- TS: `UsagePane.test.tsx` — both providers listed with no snapshot at all;
  toggling writes the setting; a hidden provider still shows its row. That last
  one is the regression the roster exists to prevent.
- TS: HUD and popover render the "No meter selected" state, distinct from the
  not-signed-in state.

Checks before commit: `cargo fmt --check`, `cargo clippy --all-targets -D
warnings`, `cargo test`, `pnpm --filter @antiburn/desktop lint type-check test`,
`pnpm run slop:all`. Commit with `-s`.

## Out of scope

- Per-model metering — nothing reports it.
- Adding new providers to meter.
- Any change to the master switch above it.

## Found while testing

- **The HUD lagged a switch by up to a minute.** `OverlaySession` only polled
  `get_live_usage` every 60s and never listened to the change event the shell
  already emits. The gate itself was right — the summary dropped the provider
  immediately — but the HUD kept painting the old bars until its next poll.
  Fixed by subscribing to `onLiveUsageChanged` alongside the poll; the poll is
  now the floor, not the only path.

## Changed while building

- **`UsageLimitsBar` stays silent instead of saying "No meter selected".** The
  bar already withholds itself when no provider has a reading. A permanent
  strip in the closed popover would take a row from the activity list forever,
  to restate a setting the reader just chose. The full Usage view carries the
  sentence instead, where the missing limits are visible.

## Open

- **Screenshot for the PR.** Needs the pane with both switches, and the HUD in
  its no-meter-selected state.
