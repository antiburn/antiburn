---
title: "Limit share from a learned dollars-per-percent factor"
created_at: "2026-09-08"
status: planned
---

# Limit share from a learned dollars-per-percent factor

- **Date:** 2026-09-08
- **Issue:** none. Opened from the 0.4.1 "no limit" badge report.
- **Status:** planned. Phases below, built in stacked worktrees.

## Problem

The session list shows "% 5h" and "% week" for each session. Release 0.4.1
(PR #426) computes these from a durable ledger: it stores one period per
provider allowance window, splits each observed percent delta across the turns
that fall inside the observation interval, and writes one allocation row per
session and period.

Three faults follow from that shape.

- A session gets a percent only if a period covered it while the app ran. Every
  session from before the install, and every session from a stretch when the
  app was closed, shows "no limit" forever.
- A session gets a percent only if a live poll bound it to an account within
  ten minutes of its last write. Sessions that finish while the app is closed
  never bind. The single-account fallback from 0.4.0 was dropped.
- A window that the meter reports as 0% gets no allocation, because the delta
  rounds to zero. The 5-hour lane starts every window in this state.

The arithmetic in the allocator is correct. A reproduction matched the stored
rows bit for bit. The shape is the problem, not the code.

A fleet constant is not a substitute. Cadence data for the same question shows
per-plan dollars-per-percent constants wrong by 2x to 10x for individuals, a
15x swing for one user across weeks, and 2x to 4x drift across months. The only
precise calibration source is the live meter that the app already polls.

## Target shape

One number per provider account and lane: the **factor**, in dollars per one
percent of the limit. The app learns the factor from the meter. Every session,
including sessions from before the install, shows `dollars / factor`.

```
meter reading a ─┐
                 ├─ percent delta ─┐
meter reading b ─┘                 ├─ sample = dollars / percent delta
turns between a and b ─ dollars ───┘

samples ─ weighted median ─ factor point (effective_at, usd_per_percent)

session dollars / factor in effect at session end ─ badge percent
Σ(estimates in current window) vs meter ─ residual
```

The factor belongs to the account and lane, not to the window. The limit does
not change when a window resets. A new window starts with the last factor. A
restart starts with the last saved factor. Only a true first install has none.

The factor drifts when the provider changes the limit or the pricing, or when
the user changes plan. New samples move it. The stored history of the factor
is itself a product: it shows when a provider tightened a limit.

### Inputs

| Input | Source | Already durable |
|---|---|---|
| Meter readings with reset time | `provider_usage_observation` joined to `provider_usage_period` | yes (90-day retention) |
| Turn dollars with timestamps | turn rows priced through `lookup_turn_pricing` | yes |
| Session dollars, inclusive of subagents | `session_analysis.pricing_breakdown_json` via `price_cached_breakdown` | yes |
| Session to account | `session_provider_account`, else single `provider_account_seen` row for the agent | yes, plus the fallback rule |
| Plan name | `ProviderUsageSnapshot.plan` / `plan_tier` | no, dropped before the observation is written; V39 adds it |

### Account resolution

One rule, used by learning and by reading. A turn or session belongs to an
account when:

1. its session has exactly one `session_provider_account` row for the
   provider, or
2. it has none, and `provider_account_seen` holds exactly one account for the
   session's agent and provider.

Otherwise it is unattributed. Unattributed turns do not enter samples.
Unattributed sessions show "unknown". This is the rule from branch
`fix/single-account-allocation-fallback` (`provider_known_accounts`), lifted
into one store helper so the two paths cannot disagree.

### Samples

A sample is one measurement of the factor. Three kinds.

**delta**: two observations `a` and `b` in the same period, where `b` is the
first later observation with `used_percent` greater than `a.used_percent`.
Percent delta is `b.percent - a.percent`. Dollars are the priced turns with
`a.observed_at < turn_at <= b.observed_at` that resolve to the period's
account. Then `a := b` and the search continues. Consecutive readings with an
equal percent are merged into one interval, so integer meters do not produce
zero deltas.

A delta interval with a positive percent delta and zero attributed dollars
means usage from another device. It is stored with kind `unattributed` and
excluded from the factor. The residual uses it.

**window_start**: one observation with `used_percent > 0` and no delta sample
yet for the lane. Dollars are the attributed turns from the window start to
the observation. Window start is `starts_at_epoch`, else `resets_at_epoch`
minus the lane duration (18 000 s or 604 800 s). The sample carries every
outside use in the window, so it is only used while no delta sample exists.

A Codex reading at 0% reports a projected reset (`is_sliding_reset_projection`).
Zero readings never form a window_start sample and never define a window start.

**rollout** (phase 4): a delta sample whose observations came from a Codex
rollout file instead of a live poll. Same arithmetic, different `source_id`.

Each sample stores the dollar split by token kind (input, output, cache read,
cache write). Without it, a later chart cannot tell a provider change from a
workload change.

### Plan tracking

Every live source reports the plan on the snapshot (Claude: subscription type
and rate-limit tier; Codex: plan type; Antigravity: plan and tier). V39 adds
`plan` and `plan_tier` to `provider_usage_observation`, stored as given. A
sample takes the plan of its closing observation. A factor point carries the
plan. Points are never pruned, so plan history survives the observation cap.

A plan change is a step in the factor, not a drift. When the latest
observation's plan differs from the current point's plan, the median uses only
samples from after the change and a new point is appended even if the value
is within 2%. Codex rollout files carry the plan on every turn, so phase 4
backfills plan history too.

Plan strings map to a closed vocabulary only at the analytics boundary.

### The factor

For each (provider, account, lane), the factor is the weighted median of delta
samples from the last 14 days. Weight is `percent_delta × 0.5^(age_days / 7)`.
Fewer than three samples in 14 days: use all delta samples. No delta samples:
use the most recent window_start sample. Nothing: no factor, badge "unknown".

Median, not mean, because a single interval that includes outside use pulls
the factor down and every session up. The median ignores it.

The app appends a **factor point** whenever the computed factor differs from
the last point by more than 2%, the method changes, or the plan changes. Points are the history.
The latest point is the current factor. A session uses the point in effect at
its end time, the earliest point for sessions before the first point.

### Recompute window

Turn rows can arrive after the observation that closes their interval, because
transcript ingest lags. Every learning pass recomputes samples whose `to_epoch`
is within the last 15 minutes and upserts them by observation pair. Older
samples are final. A pass handles at most 64 new observation pairs.

### Reading

`get_session_limit_allocations` returns one row per session and lane:

```
percent    = session.cost.total_usd / factor_point(account, lane, session.updated_at).usd_per_percent
confidence = "learned" | "seeded"    (delta vs window_start method)
```

No row when the session is unattributed or the lane has no factor. The
frontend shows "unknown" for a missing row, and "no limit" at the current
opacity only when the live summary for that provider has no window of that
kind (commit `4e7a7528`, `providerConfirmsNoWindow`).

The dollars are the inclusive session cost, subagents included. The meter
counts subagent turns. Subagent rows in the list keep their own cost and get
their own percent.

A percent above 100 is valid for a long session and shows as is.

### Residual

For each (provider, account, lane) and the current period: `meter_percent -
Σ(attributed turn dollars since window start) / factor`. Stored on each
learning pass as one row per period. It is the accuracy check. A large
positive residual means outside use. A large negative one means the factor
has drifted. Diagnostics export and analytics report it as a band.

## What is kept, what goes

Kept: `provider_usage_period`, `provider_usage_observation`,
`record_provider_usage_snapshots`, `period_for`, retention,
`session_provider_account`, `provider_account_seen`, `observe_provider_account`.

Removed: `provider_usage_session_allocation`, `provider_usage_allocation_dirty`,
`provider_usage_allocation_revision`, `allocation_frozen`, the
`provider_usage::allocation` module, `ledger::reconcile` and `reconcile_period`,
`cumulative_session_limit_allocations`, `cumulative_lane_allocation`, the
`coverage` field on the payload, and the retention branches that check those
tables.

Branch `fix/single-account-allocation-fallback` is not merged. Commit
`4e7a7528` (frontend unknown badge) is cherry-picked in phase 2. The
`provider_known_accounts` query and its tests from `fb321855` move into the
account resolution helper in phase 1. The `allocation.rs` changes are dropped.

## Schema

**V39** (phase 1):

```sql
ALTER TABLE provider_usage_observation ADD COLUMN plan TEXT;
ALTER TABLE provider_usage_observation ADD COLUMN plan_tier TEXT;

CREATE TABLE provider_limit_factor_sample (
    id                  INTEGER PRIMARY KEY,
    provider            TEXT NOT NULL,
    account_key         TEXT NOT NULL,
    lane                TEXT NOT NULL CHECK (lane IN ('weekly', 'fiveHour')),
    kind                TEXT NOT NULL CHECK (kind IN ('delta', 'window_start', 'unattributed', 'rollout')),
    period_id           INTEGER REFERENCES provider_usage_period(id),
    from_epoch          INTEGER NOT NULL,
    to_epoch            INTEGER NOT NULL,
    from_percent        REAL NOT NULL,
    to_percent          REAL NOT NULL,
    input_usd           REAL NOT NULL,
    output_usd          REAL NOT NULL,
    cache_read_usd      REAL NOT NULL,
    cache_write_usd     REAL NOT NULL,
    turn_count          INTEGER NOT NULL,
    plan                TEXT,
    plan_tier           TEXT,
    source_id           TEXT NOT NULL,
    computed_at_epoch   INTEGER NOT NULL,
    UNIQUE (provider, account_key, lane, from_epoch, to_epoch)
) STRICT;

CREATE TABLE provider_limit_factor_point (
    id                  INTEGER PRIMARY KEY,
    provider            TEXT NOT NULL,
    account_key         TEXT NOT NULL,
    lane                TEXT NOT NULL CHECK (lane IN ('weekly', 'fiveHour')),
    effective_at_epoch  INTEGER NOT NULL,
    usd_per_percent     REAL NOT NULL,
    method              TEXT NOT NULL CHECK (method IN ('delta', 'window_start')),
    sample_count        INTEGER NOT NULL,
    plan                TEXT,
    plan_tier           TEXT,
    UNIQUE (provider, account_key, lane, effective_at_epoch)
) STRICT;

CREATE TABLE provider_limit_residual (
    period_id           INTEGER PRIMARY KEY REFERENCES provider_usage_period(id),
    computed_at_epoch   INTEGER NOT NULL,
    meter_percent       REAL NOT NULL,
    estimated_percent   REAL NOT NULL
) STRICT;
```

Samples and points outlive observations. Retention deletes samples older than
the session retention setting, capped at 365 days, and never deletes points.
`period_id` on a sample is `ON DELETE SET NULL` in effect: the learner nulls
it before the period retention pass runs, so the period retention query stays
as it is.

**V40** (phase 2): drop the four allocation objects listed above.

## Phases

Each phase is one PR from a stacked worktree, built by a Sonnet builder from
this document, reviewed here, then merged. Phase 2 cannot ship without phase 1.
Phases 1 and 2 ship in the same release; there is no release between them.

### Phase 1: learn the factor

- V39 schema.
- `store::provider_limit` module: account resolution helper (SQL that returns
  the account for a session under the two-step rule, plus the reverse:
  attributed turn dollars for an account between two epochs, grouped by
  session and split by token kind, priced through the existing turn pricing
  key), sample upsert, point append, point lookup at an epoch, residual
  upsert. The learner sums the per-session rows. The later contribution chart
  reads the same query unsummed, so keep the session grouping from the start.
- `provider_usage::factor` module: `learn(store, now_epoch)`. Called where
  `ledger::reconcile` is called today (`live/mod.rs` after
  `record_provider_usage_snapshots`, and `usage_alerts::background_pass`).
  Bounded per pass as above.
- The old allocator keeps running in this phase. Nothing reads the new tables
  yet.
- Tests: synthetic observations and turn rows for one account; delta sample
  arithmetic; merged equal readings; unattributed interval; window_start only
  while no delta exists; weighted median with one outlier; point appended only
  on change; recompute window upserts a late turn; two accounts on one agent
  produce no fallback attribution; Codex 0% projected reset ignored; plan
  stored on the observation; plan change drops earlier samples and appends a
  point.

#### Decisions (implementation, 2026-09-08)

The document above did not settle these; they were decided during the phase 1
build and kept for phase 2 to build on.

- **`window_start` gating checks the period, not only the lane.** The
  document's rule is "no delta sample yet for the lane." Read literally, a
  period whose own two readings already form a delta pair would *also* emit a
  `window_start` sample from its first reading, because the lane-wide "a delta
  exists" flag is only checked once, before that period is processed. Phase 1
  adds one clause: a period only produces `window_start` when it cannot
  produce a delta pair of its own (zero or one usable reading). A period with
  a real pair never needs the fallback its own data has already outgrown.
- **`store::Store::lock` widened from private to `pub(crate)`.** The plan asks
  for factor-learning tests that build synthetic turns, periods, and
  observations the way the store's own lifecycle tests do. Those existing
  tests live inside the `store` module tree and can already reach the private
  connection lock; `provider_usage::factor`'s tests live outside it. Widening
  `lock` to `pub(crate)` (still crate-only, no new public surface) let the
  factor tests reuse the same direct-SQL fixture style instead of duplicating
  it through a second accessor.
- **`provider_usage::attribute`, `has_tokens`, and `Attributed::models` widened
  to `pub(crate)`.** `store::provider_limit`'s attributed-dollars query needs
  the same per-model provider routing `provider_usage::allocation::weighted_turns`
  already does, so it calls the same function rather than re-implementing
  routing rules a second place they could drift apart.
- **`factor_point_at` (point lookup at an epoch) is used by phase 1, not left
  for phase 2 alone.** The document lists it as a phase 1 storage primitive
  with no phase 1 caller, which is dead code under this repository's
  no-suppression rule. `compute_residual` uses it to price a period's residual
  against the factor in effect at that period's own latest observation, rather
  than always the newest point overall — arguably more correct than reading
  the single newest point directly, since a plan change between the reading
  and "now" would otherwise restate an old period's residual under today's
  factor.
- **Residual is computed once per (provider, account, lane) touched in a
  pass, from the most-recently-observed period among those already loaded**,
  not from a separate "current period per lane" scan. Candidate-period
  selection already sorts by `last_observed_epoch` descending, so the first
  period seen per lane in a pass is already its current one; reusing it keeps
  the residual step inside the same bounded per-pass work instead of adding
  another unbounded query.
- **Candidate-period selection is cursor-based, not "observed in the last 15
  minutes."** The original rule never learned from a period whose readings
  were older than 15 minutes, which breaks bootstrap on upgrade (90 days of
  stored observations sit unread until a new reading lands) and phase 4's
  rollout backfill (it writes observations timestamped in the past, so those
  periods would never enter a pass). `provider_limit_learn_cursor
  (period_id, learned_through_epoch)` now tracks how far each period has been
  read. A period is a candidate when it carries a primary lane and account
  scope, and either has no cursor row, has a reading newer than its cursor, or
  was observed at or after `now - 15 minutes` (so a just-touched period stays
  open to recompute even once its cursor catches up). A pass advances a
  period's cursor to its `last_observed_epoch` only after considering every
  sample it could yield; stopping mid-period on the pass budget leaves the
  cursor where it was, so the period stays a candidate next time. A cursor row
  is deleted in the same place, and against the same about-to-be-removed set
  of periods, that samples are detached before period retention runs.
- **Plan comparison uses the (plan, plan_tier) pair, not `plan` alone.** On
  Claude, moving between tiers of the same plan (Max 5x to Max 20x) changes
  only `plan_tier`; `plan` stays `"max"`. Comparing `plan` alone would miss a
  tier change entirely. `recompute_point`'s `plan_changed` check,
  `filter_by_plan`, and `window_start_factor`'s plan guard all compare the
  full pair.
- **A factor point's `effective_at_epoch` is the `to_epoch` of the newest
  sample its estimate used** (for a `window_start` estimate, that sample's own
  `to_epoch`), not the epoch `learn` happened to run at. Bootstrapped or
  backfilled history needs to date its points by the data, not by when the
  pass computing them ran, so that "the factor in effect at a session's end
  time" resolves correctly for historical sessions. The point upsert stays
  keyed on `(provider, account_key, lane, effective_at_epoch)`, so recomputing
  the same interval replaces its point rather than appending a duplicate.
- **`has_delta_factor_sample` is scoped to the account's current
  `(plan, plan_tier)`,** not asked lane-wide. Read lane-wide, a plan or tier
  change would find the *old* plan's delta history, conclude a delta sample
  already exists, and block a `window_start` sample from ever seeding the new
  plan's own factor — leaving the point stuck on the old plan indefinitely.
  Scoping the check to the pair the latest observation reports lets a fresh
  plan or tier seed its own `window_start` sample immediately; the median path
  already prefers delta samples over `window_start` once they exist.

### Phase 2: read from the factor, delete the allocator

- `get_session_limit_allocations` computes from session cost and factor
  points. DTO: `coverage` becomes `confidence`. TypeScript payload updated.
- Cherry-pick `4e7a7528`. Reconcile the "unknown" / "no limit" split with the
  new missing-row semantics.
- V40. Delete the removed list above. No lint suppressions; dead code goes.
- Tests: command returns percent from cost and point-at-end-time; earliest
  point for older sessions; missing factor gives no row; subagent rows priced
  on their own cost; frontend badge tests from the cherry-pick still pass.

#### Decisions (implementation, 2026-09-08)

The document above did not settle these; they were decided during the phase 2
build.

- **Attribution routes on `model_breakdown_json`; pricing reads
  `pricing_breakdown_json`.** `session_analysis` stores two breakdowns:
  `model_breakdown_json` keys by the literal model name;
  `pricing_breakdown_json` keys by `turn_pricing_key(model, speed)`, which
  appends a "-fast" suffix when the turn ran fast and the model's own name
  does not already end that way. `provider_usage::attribute` routes by-model
  for bring-your-own agents and needs the literal model name to match its
  routing table, so `session_limit_allocations` calls `attribute` with
  `model_breakdown_json`. It then prices each provider's dollars from
  `pricing_breakdown_json`, keeping each entry's own key (so a fast-mode turn
  prices at its fast rate) and assigning it to whichever provider's
  attributed models match it directly or with a trailing `-fast` removed.
  The factor's own samples price turns through the same speed-aware
  `lookup_turn_pricing`, so this keeps the badge on the same rate the factor
  was learned at. It falls back to pricing `model_breakdown_json` directly
  only when `pricing_breakdown_json` is empty or fails to parse — the only
  breakdown available then.
- **Subagent cost needs no separate handling.** The plan's phase 2 test list
  asks for "subagent rows priced on their own cost," which assumes a subagent
  appears as its own row to price. It does not: `assemble_session_analysis`
  merges every subagent's tokens into the parent session's own
  `inclusive_model_breakdown` before it is stored as `model_breakdown_json`.
  `recent_sessions_excluding` returns one row per top-level session, so
  `session_limit_allocations` already prices each session's subagent work
  through that single inclusive breakdown. No separate subagent `SessionRecord`
  exists to test in isolation.
- **`Store::analyses` batches the per-session analysis lookup**, the same way
  `session_bound_accounts` batches account resolution: one query across every
  session key in the activity window (chunked at 500, matching
  `MAX_ACTIVITY_ROWS`) rather than one `session_analysis` lookup per session.
  `session_limit_allocations` was calling the existing single-session
  `Store::analysis` once per row.
- **`window_id` on `SessionLimitAllocation` now carries the lane string**
  (`"weekly"` or `"fiveHour"`), not a period or window identifier. Periods no
  longer drive the badge, so no per-period id exists to report; the lane is the
  only "which window" fact the frontend needs to pick an icon and label.
- **`coverage`, `period_count`, and `resets_at` are dropped from
  `SessionLimitAllocation`**, not renamed or kept alongside `confidence`. The
  frontend badge (`PopoverSession`, `SessionList`) reads only `metric`,
  `percent`, and now `confidence`; nothing consumes a reset time or a period
  count once the estimate comes from a factor point instead of a durable
  per-period ledger row.
- **`clear_local_session_data` also deletes `provider_limit_residual`,
  `provider_limit_learn_cursor`, `provider_limit_factor_sample`, and
  `provider_limit_factor_point`, ahead of `provider_usage_period`.** These
  phase 1 tables foreign-key onto `provider_usage_period`, and every
  connection runs with `PRAGMA foreign_keys = true`. The plan did not call this
  out because the tables did not exist when this method was last touched;
  leaving them out would raise a foreign key violation the first time a user
  clears local data.
- **Dead code removed beyond the plan's explicit list**, found by following
  compiler and clippy errors after deleting `allocation.rs` and `ledger.rs`:
  `Store::session_usage_turns` and `session_usage_turns_between` (and their
  `SessionUsageRecord`/`SessionUsageTurnRecord` types), `database_path` on
  `Store` (only ever written, its one reader was `open_allocation_reader`),
  and `ALLOCATION_READ_BUSY_TIMEOUT`. None had a use outside the allocator.
- **V40 drops the three allocator tables before dropping the
  `allocation_frozen` column.** The dirty queue and its generation counter, and
  the materialized per-session rows, all foreign-key onto
  `provider_usage_period`; dropping the whole table they reference is safe
  regardless of order, but dropping them before touching
  `provider_usage_period` itself keeps the migration's intent obvious to a
  reader. `ALTER TABLE ... DROP COLUMN` needs SQLite 3.35+, available through
  the bundled `libsqlite3-sys` 0.30.1.

### Phase 3: residual, diagnostics, analytics

- Residual row per period on each learning pass.
- Diagnostics export: one entry per (provider, lane) with factor, method,
  sample count, point count, latest residual. No account keys, per the
  export's content notice; update the notice text.
- Analytics event `antiburn.limit_factor_observed`: provider, lane, plan (closed
  vocabulary: mapped known plan names, else `other`), factor band (closed
  vocabulary, banded by powers of two: `under_1`, `1_to_under_2`,
  `2_to_under_4`, `4_to_under_8`, `8_to_under_16`, `16_to_under_32`,
  `32_to_under_64`, `64_to_under_128`, `128_and_over` dollars per percent),
  residual band (`within_5`, `within_20`, `over_20`, `unknown`). Fire on first
  computation and on band change, at most once per day per (provider, lane).
  Follow the catalogue steps in `docs/analytics.md` and the review contract in
  `docs/analytics-measurement.md`.
- Tests per the analytics review contract.

#### Decisions (implementation, 2026-09-08)

The document above did not settle these; they were decided during the phase 3
build and kept for later phases to build on.

- **`compute_residual` already matched the design and needed no fix**, only a
  return value. It writes one `provider_limit_residual` row per current period
  on each learning pass, keyed by `period_id`, exactly as specified. It was
  changed to return the `(meter_percent, estimated_percent)` pair it just
  wrote, purely so `learn` could hand it to the analytics call without a
  second read of the row it just upserted.
- **`factor::learn` now returns `Vec<LearnedFactor>`**, one entry per
  `(provider, account, lane)` the pass touched, carrying the account's latest
  factor point and current residual. Existing callers that discarded `learn`'s
  former `()` result need no change; the two call sites that have an
  `AppHandle` available (`usage_alerts::background_pass` and
  `live::mod::summarize_collected`'s `storage_app: Some(_)` branch) pass the
  result to `analytics::record_limit_factor_observed`. The third call
  site — `summarize_collected`'s `storage_app: None` branch, exercised only by
  the `#[cfg(test)]` `summarize` helper — has no `AppHandle` to report through
  and fires no event, which matches every other analytics call gated on an
  `AppHandle` in this codebase.
- **The event key is `(provider, lane)`, not `(provider, account, lane)`.**
  The wire payload carries no account dimension — deliberately, since an
  account key must never leave this machine — so there is nothing to key a
  second observation on when one provider has more than one account for the
  same lane. Within one pass, only the first account `learn` produced for a
  given `(provider, lane)` pair can report; a second account's differing
  tuple is suppressed by the same 24-hour floor that suppresses a genuine
  same-account band change. This is a real information loss for the
  (uncommon) multi-account case, accepted rather than widening the payload.
- **The 24-hour floor applies even to a genuine band change**, not only to a
  repeated one. Read literally, "fire on first computation and on band
  change, at most once per day" could mean the daily cap only throttles
  repeats of an *unchanged* tuple. Phase 3 instead treats the cap as an
  absolute floor between any two events for the same pair: a pair that
  changes bands twice in one day reports the first change and stays silent
  on the second until 24 hours have passed. This keeps a factor bouncing
  between two adjacent bands from becoming a daily-volume source.
- **Plan mapping, factor banding, and residual banding are pure functions in
  `analytics::event`** (`map_plan`, `factor_band`, `residual_band`), not
  methods on `LearnedFactor` or inline in `record_limit_factor_observed`.
  This mirrors `band_for_percent` in `provider_usage::live::model`: a single
  function each call site and each test reads, so the boundaries cannot drift
  between two hand-written copies.
- **The `Properties`/`Facts` payload grew three optional fields** (`plan`,
  `factorBand`, `residualBand`) rather than reusing `label`/`detail`/`bucket`
  for a third and fourth dimension. The existing fields are already spoken
  for by `label` (provider) and `detail` (lane); reusing `bucket` for a factor
  or residual band would collide with its documented "always a magnitude
  count" meaning. The event schema grew from twenty-three to twenty-six
  wire fields; every document that counts them (`docs/analytics.md`,
  `docs/privacy-policy.md`, `docs/support.md`, `PrivacyPane.tsx`) and the
  Rust test that pins the count were updated together.
- **The diagnostics export's `limit_factors` section groups by `(provider,
  lane)` and numbers accounts `"account 1"`, `"account 2"`, ... only when a
  group has more than one**, ordered by the (opaque, never-exported) account
  key so the numbering is stable across repeated reads of the same database.
  A lane with exactly one account carries no `account` field: there is
  nothing to disambiguate, and an ever-present `"account 1"` would be a
  needless field to explain in the content notice.
- **`DiagnosticsExport::FORMAT_VERSION` was bumped from 1 to 2** even though
  no existing test pinned it. Adding a new top-level section is a real shape
  change for any external consumer of the exported JSON; bumping the version
  costs nothing and documents the change in the file a consumer would
  actually read.
- **The diagnostics export's recent-sample window is 14 days**, matching
  `factor::FACTOR_WINDOW_SECS`. The constant is duplicated
  (`store::provider_limit::RECENT_SAMPLE_WINDOW_SECS`) rather than shared,
  because `factor.rs`'s constant is private and the two call sites have no
  other reason to depend on each other; a future change to one lookback is
  not implied to require the other.
- **On 2026-09-09, `factor_band` changed from four-fold steps to power-of-two
  steps** (`under_1` through `128_and_over`). The original four-fold bands
  were not based on any measured factor distribution; finer log buckets let
  Cadence regroup adjacent bands later without a contract change.

### Phase 4: Codex rollout observations

- Resurrect the bounded rollout reader from closed PR #427
  (`provider_usage/backfill.rs`, `store/usage_backfill.rs`,
  `read_rollout_batch`, checkpoints, retry backoff). Skip its legacy
  `internal:liveUsageHistoryV2` import; that history has no reset times.
- Rollout readings become observations with `source_id`
  `codex-rollout-backfill`. The learner treats them as any other observation;
  samples from them carry kind `rollout`.
- Effect: a Codex account has a factor from history on first install, without
  waiting for a live poll.

#### Decisions (implementation, 2026-09-08)

The document above did not settle these; they were decided during the phase 4
build.

- **No legacy-import state key.** #427's `BackfillState { legacy_history_imported }`
  existed only to run the `internal:liveUsageHistoryV2` blob import once; the
  document explicitly drops that import (reset-less history), which leaves
  the state struct with no remaining field and no remaining purpose. Adding
  it back only to leave it empty would be dead code under this repository's
  no-suppression rule, so phase 4 carries no state setting key at all.
- **The checkpoint is keyed by session and provider, not by account.**
  #427's `provider_usage_backfill_checkpoint` included `account_key` in its
  primary key, inherited from a candidate query that joined a direct
  `session_provider_account` binding and so only ever saw one account per
  session. Phase 4's candidate query instead resolves an account through the
  shared two-step rule (`resolve_bound_account`, falling back to
  `provider_known_accounts`), which is a Rust-side step over a raw SQL scan
  rather than a join, and a resolved account is not guaranteed stable if the
  machine's known-account set changes between passes. The checkpoint tracks
  how far a *file* has been read, which does not depend on which account a
  later pass attributes it to, so dropping `account_key` from its identity
  removes a source of drift between the checkpoint and the resolution rule
  without losing anything the checkpoint needs.
- **Rollout windows reuse `codex_rollout::parse_windows` rather than
  reimplementing window parsing.** #427 parsed `primary`/`secondary` a
  second time in `backfill.rs`, with its own `authoritative` rule
  (`resets_at.is_some()`) that disagreed with the live tail-reading path's
  (`authoritative: true` always, per `codex_rollout.rs` and
  `codex_app_server.rs`). Reusing the same function both paths share means a
  historical reading and a live one build a window through the identical
  rule — same `authoritative` value, same `is_sliding_reset_projection`
  filtering — so the zero-reading/projected-reset case is "a stated reading
  with no committed boundary," never "an unauthoritative, ignorable one."
  Feeding a `used_percent`-bearing reading into the learner as
  unauthoritative would have silently dropped it from every delta and
  window-start computation, which is the opposite of phase 4's purpose.
- **`import_rollout_batch` and its `RolloutImportBatch`, `RolloutBatch`, and
  `RolloutReading` types carry no `scanned_bytes` or `skipped_bytes` field.**
  #427 exposed both for test observability; neither is read by any
  production caller. The "a too-long record is skipped forward, not
  reread forever" behavior — the reason `skipped_bytes` existed — is instead
  asserted directly against `RolloutBatch::next_offset` advancing between
  two calls, which proves the same thing without carrying a field no
  caller uses.
- **The scheduler runs one rollout-history batch, then `factor::learn`,
  every tick — and, only when a batch reports more work, wakes again after
  a short delay (or its own retry time) via `tokio::select!` rather than
  waiting for the next full `TICK`.** This mirrors #427's
  `schedule_backfill`/`wait_for_backfill` shape exactly, renamed to
  `schedule_rollout_continuation`/`wait_for_rollout_continuation`, and
  `usage_alerts::blocking::run` is widened from `FnOnce(Thread)` to
  `FnOnce(Thread) -> T` so the scheduler can read a batch's
  `continue_soon`/`next_retry_epoch` back out of the blocking hop.
- **Retention deletes a `complete` checkpoint once its `completed_at_epoch`
  ages past the same cutoff observations use**, added to
  `apply_provider_usage_retention_in` next to the existing sample and period
  cleanup. A later append to that file is then read from byte zero instead
  of its old cursor; the readings that offset would have skipped past have
  already expired, so nothing already-imported is lost, and a stale
  checkpoint for a file that no longer changes does not accumulate forever.

### Later, not in this round

- A factor history chart per account, from `provider_limit_factor_point` and
  the token-kind split on samples.
- A limit-over-time chart per account and lane, from
  `provider_usage_observation`. Needs the 90-day observation cap raised or a
  downsampled copy that survives it. Phase 1 must not depend on the cap.
- A contribution chart: which sessions used the most of a bucket. Per-session
  attributed dollars inside the bucket, divided by the factor in effect, from
  the phase 1 query. The meter line drawn over the stacked estimates shows the
  residual as the gap.
- Seed the 5-hour factor from the weekly factor and a per-plan ratio while no
  5-hour sample exists.
- Cadence: read the coarse factor and residual bands per plan in Metabase.

## Known limits

- The first-open estimate comes from one window_start sample. It includes
  every outside use in the window, so early percentages run high. Delta
  samples correct it within a few polls.
- The provider does not meter every token at list price. A cache-heavy session
  and an output-heavy session with the same dollars get the same percent. The
  residual shows the error. The token-kind split on samples is the data a
  later model would need to fix it.
- An account with no local usage in the current window has no window_start
  sample. The badge stays "unknown" until the first local session moves the
  meter.
- Session cost is repriced on read against the current pricing table. Samples
  hold dollars priced at learning time. A pricing table update shifts the
  two apart until new samples land. Acceptable; the drift is small and
  self-correcting.
