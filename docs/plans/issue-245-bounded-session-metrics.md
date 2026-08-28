# Bounded session metrics — removing the retained turn history

Status: **implemented.** This document remains the design record for the
deliberately smaller alternative to issue #245. The implementation uses a
540-cell active-position quantum plus a 64-record reorder window. Cell
compaction can move continuous values between progress buckets.

Implementation measurement on the baseline machine: 241,599 estimated derived
bytes and 241,988 allocator-observed live bytes after 1,269,150 turns. The
pre-change values were 525.6 MB retained and a 797.0 MB metrics-only peak; the
new metrics peak is 220.6 MB, including 220.3 MB of adapter state outside
metrics. The UUID-bearing composite peaks at 317.8 MB because evidence identity
state remains unbounded. The saturating profile retains 366,413 derived bytes
at 40,000 records and 395,789 bytes at 400,000 records.

Owner: analysis pipeline (`crates/antiburn-local/src/analysis`).

---

## 0. Revision history

### Revision 2 → 3 (this revision): blockers resolved

Every item below was verified against the working tree before being accepted.

| # | Finding | Verified how | Resolution |
|---|---|---|---|
| B1 | The `Axis::Ordinal → Axis::Active` flip reintroduced the out-of-order bug at full magnitude. Revision 2 bypassed the reorder ring on the ordinal axis and collapsed every earlier record into slot 0. On `timestamps_repeated_and_out_of_order` (10 s, 5 s, 5 s, 20 s) record 0 folds at ordinal 0, then record 1 flips the axis and pins it to bucket 0 — the golden has it in bucket 60 | Re-derived against the fixture and its golden | **Fixed** in §5.4: the ring is never bypassed, and the flip collapses folded ordinal slots into **two** carriers (`i64::MIN` and the single shared pre-flip timestamp `T0`), which finalize places like any other slot |
| B2 | §6.3's mode-1/mode-2 "two small pending lists" were uncapped, so they were O(qualifying cache-miss turns) — the exact failure this plan removes — and "discard the other's slot writes" is impossible after a destructive doubling merge | Read revision 2 against `cache_miss_events` | **Fixed** in §5.5/§6.3: the four cache fields are **duplicated per slot** (mode 1 and mode 2), and finalize selects one pair. No pending list |
| B3 | The 256-entry late-tool candidate cap silently dropped `tool_calls_by_name` and MCP counts, a second unnamed exception to "counters never degrade" | Read `RecordSink::finish` and §8.4's decision to leave `pending_commands` uncapped | **Fixed** in §6.9: name-derived effects apply from the scalar test `ordinal < observed_turns` alone. Candidates are reserved for slot-targeted effects |
| B4 | Retroactive slot writes (mode-2 resolution, late-tool candidates) assumed a position that the 64-record reorder ring has not yet assigned | Read revision 2's §6.3 and §6.9 against §5.3 | **Fixed** in §5.6: one stated rule for patching a target that is still in the ring versus already folded |
| B5 | `metrics(&self)` cannot flush the ring, so up to `REORDER_WINDOW` turns would be missing from the buckets it returns | `CompositeSink::metrics`, the desktop inline test and §9.2 all reach it | **Fixed** in §5.7: `project()` folds the ring residue into the **output** buckets, non-destructively |
| B6 | The 512 KiB budget did not survive its own name-flood test: capped maps stored owned `String` keys the interner did not deduplicate | Counted the saturated collections | **Fixed** in §5.9: map keys are interned ids, each name category has its own limit, and the bound is 640 KiB with the arithmetic published at stage 0 |
| B-1 | §3 claimed "no test golden serializes `NormalizedSession`". **False.** `claude_characterization::actual_document` emits `{"normalizedSession": …, "sessions": …}`, so every event is serialized in all 15 goldens. A plain `#[serde(default)] bool` would add `"mayResolveLateTool": false` to every event and change all 15 goldens at stage 4 | Read `actual_document` and `goldens/records_all_kinds.json` | **Fixed** in §8.3: the field uses `#[serde(default, skip_serializing_if = …)]`, following the `wrapper_tool` / `uuid` / `parent_uuid` precedent in `model.rs`, and §13's stage-4 exit criterion asserts an empty golden diff |
| H-1 | §7.2's option (b) ("stop re-tagging") was imprecise and contradicted §7.3: `push_stream` re-tags **sub-agent** streams too, and a sub-agent's own records are `Parent` unless sidechained | `vendors/jsonl::parse_record`; `merge::push_stream`; `metrics_sink::push_stream` | **Fixed** in §7.2: the rule is stated at stream granularity, and the fifth merge divergence it creates is added to the exempt list |
| H-3 | `src/analysis/tests.rs` (2,560 lines, ~97 in-crate tests pinning exactly these semantics) was absent from the touched-file list | Listed the test names | **Fixed** in §8.8 and §13, with per-stage expectations |
| H-4 | Capping `skill_uses` and the tool maps is a user-visible output change with no changelog entry; `export.rs` was missing from the consumer audit | `analysis.rs` builds `skills` from `metrics.skill_uses`; `export.rs` carries `metrics` and `skills` at `FORMAT_VERSION = 2` | **Fixed** in §3, §11 and §13 |
| H-5 | `docs/plans/local-insights-architecture.md` records the unbounded-metrics acceptance this plan removes, in three places | Read lines 222, 470 and the CH-005 entry | **Fixed** in §8.9 |
| M-1…M-7, S1…S9, L-1…L-5 | Corpus wiring, no nightly workflow, incomplete slop-rule list, three desktop Rust workspaces in CI, two efficiency rules, `evidence_fixture_names()` and the fixtures README, four `retained_turns()` call sites, and nine smaller items | Each checked in the tree | **All applied**; the largest is S1, which deletes `MAX_PENDING_DURATION` and `RecentTimestamps` by resolving skill durations on the ring's sorted pop stream |

### Revision 1 → 2

Three findings, all verified: the transcript-order slot key disagreed with the
placement anchor for out-of-order streams (fixed by the reorder window);
`merge_metrics` re-tags sidechain turns (§7); and the golden gate has almost no
power because the largest fixture is 15 records and three `SessionMetrics`
fields are `skip_serializing` (fixed by making the differential oracle the drift
gate). Also added in revision 2: `wrapper_tool`, the `summary.model` fallback,
late-skill ordering and duration, the compaction/cache move into slot fields,
the corrected quantum arithmetic, and the rebuilt benchmark and check sections.

---

## 1. Problem and measured baseline

`SessionMetricsAccumulator` keeps every metric-bearing record for the lifetime
of the accumulator:

```rust
pub struct SessionMetricsAccumulator {
    identity: MetricsIdentity,
    turns: Vec<MetricTurn>,   // one entry per NormalizedRecord::MetricsEvent
    tallies: OnlineTallies,
    summary: Option<SessionSummary>,
}
```

`metrics()` borrows the whole vector and runs `finalize_metrics`, which rebuilds
a sorted timestamp vector, a cumulative-active vector, a cache-turn vector, an
efficiency turn vector and a `HashMap<&str, usize>` on **every** call. antiburn
is an always-running utility; `CONTRIBUTING.md` and `AGENTS.md` require retained
data to stay bounded by the visible feature's needs. The visible feature is a
**180-bucket** context chart plus scalar headline numbers.

### Measured baseline (`crates/antiburn-local/benches/BASELINE.md`)

| Figure | Value |
|---|---|
| Metrics accumulator, 10 MiB source | 34,361 turns, 10.4 MB retained (~303 B/turn) |
| Metrics retained at 500 MiB | **525.6 MB** over 1,722,451 turns (~305 B/turn, ≈0.92× source) |
| Metrics-only peak at 500 MiB | 797.0 MB (1.40× source) |
| Composite peak at 500 MiB | 797.0 MB (evidence adds 5,825 bytes) |
| Streaming peak, 1/10/50 MiB | 1.49× / 1.84× / 1.75× source |

Two facts from re-reading `benches/memory_baseline.rs` that issue #245 does not
state, and that this plan must not paper over:

1. The metrics-only peak (797 MB) exceeds retained turns (525.6 MB) by ~271 MB.
   That block is **not** owned by `metrics_sink`. The known candidates are inside
   the Claude adapter, which the same `visit` call keeps live:
   `ClaudeStreamState::max_usage_by_message_id` (one entry per distinct
   `message.id`; the corpus emits a unique id per assistant record) and
   `ClaudeStreamState::pending_commands`.
2. `SessionEvidenceAccumulator::seen_thread_uuids` is uncapped. The original
   measurement corpus emitted no `uuid`, so its evidence figure did not measure
   Claude-shaped identity state. The corpus now emits unique synthetic UUIDs;
   the historical 500 MiB rows need replacement after the next benchmark run.

**Claim wording.** After this plan lands the honest claim is: *session-metrics
retained state is bounded and constant in the record count.* It is **not**: *the
pipeline peak is bounded.* §12 tracks both items; §10.5 attributes the residual
with a measurement.

### Why the turn buffer exists

Every output of `finalize_metrics` is an online fold except the bucket index:

```
bucket_index = min(179, floor(180 * cum_active(t) / active_ms))
```

`cum_active(t)` is knowable during the stream. `active_ms` is known only after
the last record. This is an impossibility result for *exact + bounded + single
pass* simultaneously — see §4.

---

## 2. Scope

### In scope

- Delete `MetricTurn` and `SessionMetricsAccumulator::turns`; replace the
  finalize-from-history design with an online reducer whose retained state is
  bounded and constant in the record count.
- Keep every user-visible metric and every order-sensitive semantic in §6.
- Rewrite `merge_metrics` as a bounded chronological merge over derived facts.
- Cap the remaining unbounded per-session collections inside `metrics_sink`.
  This lands as its own commit inside stage 4 with its own sign-off and its own
  changelog entry, because two of them are user-visible output.
- Replace `thread_efficiency_from_inputs`' `Vec<Turn>` + `HashMap` with a
  streaming reducer; keep `thread_efficiency`'s public signature and tests.
- Prove the bound with a retained-bytes test **and** an allocator-corroborated
  bench line.
- One `NormalizedEvent` field (`may_resolve_late_tool`) so the sink can honour
  `SessionSummary::late_tools` without a turn history.

### Out of scope (explicit)

- SQLite `turns` / `turn_content` archival, or any new table, column, or
  migration. Nothing in the shipped product queries per-turn rows: the six
  hygiene checks read `SessionEvidence` only, and the chart is 180 buckets.
- Retaining transcript text of any kind.
- Incremental/byte-offset parsing, adapter checkpoints, resumable parses.
- Two-pass parsing. `BASELINE.md` measures ~133 MiB/s and a second
  pass doubles the dominant cost inside a worker that competes with the user's
  live agent sessions, and every append to a live session moves the denominator.
  Issue #245 rejects it for the same reason.
- Capping `ClaudeStreamState::max_usage_by_message_id` or `pending_commands`,
  `SessionEvidenceAccumulator::seen_thread_uuids`, or the batch path's
  `Vec<NormalizedEvent>` (§12).
- Desktop UI, DTO, IPC, store schema, design tokens.
- Committing, pushing, or opening a PR from this workflow. The untracked
  `.agents/` and `.worktrees/` entries stay untouched.

### Mapping onto issue #245's numbered decisions

| #245 decision | This plan |
|---|---|
| 1. Rows are the source of truth (SQLite `turns`) | **Deferred.** No shipped consumer demonstrates the need |
| 2. Capture message text in `turn_content` | **Deferred.** This plan retains no transcript text |
| 3. Keep the hot table narrow | **Not applicable** without decision 1 |
| 4. One database file | **Not applicable** without decision 1 |
| 5. Aggregates incremental and rebuildable from rows | **Half.** Aggregates become incremental; rebuild-from-rows defers with decision 1 |
| 6. Chart buckets use a time quantum with merge-on-overflow | **Landed with two deviations.** The implementation uses `SLOTS = 540`, and `merge_metrics` uses one shared active-time axis instead of elementwise bucket addition. |
| 7. Remove the `turns` vector | **Landed.** The headline |
| 8. Incremental parse with checkpoints | **Deferred.** `BASELINE.md` dismisses it with measurements |
| Step 6 (add `uuid`/`parentUuid` to the corpus) | **Partly.** The 500 MiB bench enables one `SessionSpec::thread_identity` flag (§10.5) |
| Step 7 (assert bounded peak memory) | **Scoped down** to bounded session-metrics retained state (§1) |

---

## 3. Existing consumers (audited)

### Public API surface of `metrics_sink`

| Item | Consumers |
|---|---|
| `SessionMetricsAccumulator::new` | desktop `stream_claude_with_hooks` and its inline test; `CompositeSink`; tests; both benches |
| `metrics(&self)` | desktop `stream_claude_with_hooks`; `CompositeSink::metrics`; tests |
| `earliest_ts_ms(&self)` | desktop `stream_claude_with_hooks` (`started_at_epoch`) |
| `retained_turns()` / `retained_bytes()` | **tests and benches only**, at four call sites: `tests/streaming_metrics_memory.rs`, `tests/pipeline_corpus.rs`, `benches/memory_baseline.rs`, `benches/pipeline_baseline.rs` (twice — a `black_box` in a criterion loop and a printed `retained_bytes()/retained_turns()` ratio that `BASELINE.md`'s "~303 bytes/turn" line comes from) |
| `from_parts` (`pub(crate)`) | `engine::analyze_session` — the batch path for **every** agent |
| `merge_metrics` | desktop `stream_claude_with_hooks`; `merged_streaming_metrics_equal_the_merged_batch` |
| `RecordSink for SessionMetricsAccumulator` | `ClaudeAdapter::visit*`, the default `VendorAdapter::visit`, `CompositeSink` |

### Output surface

- `SessionMetrics` / `Bucket` serde shape is persisted as
  `session_analysis.metrics_json` and mirrored in the desktop TypeScript types.
  **This plan changes no field.**
- `skill_uses`, `mcp_tool_calls`, `tool_calls_by_name` are `skip_serializing`,
  so they are absent from `metrics_json` and from the export payload. They are
  still compared by `SessionMetrics: PartialEq`, which is what makes the
  differential oracle (§9.1) the real gate, and `skill_uses` **is** copied into
  `SessionAnalysis::skills` by the desktop, so capping it is user-visible.
- `apps/desktop/src-tauri/src/export.rs` carries `metrics: Option<SessionMetrics>`
  and `skills: Vec<SkillUse>` at `FORMAT_VERSION = 2`. No exported field changes
  shape, so **no `FORMAT_VERSION` bump** — recorded here as a decision, not an
  omission. The skill *list length* can shorten past the cap; that is the
  changelog entry in §11.
- **The characterization goldens serialize `NormalizedSession`.**
  `claude_characterization::actual_document` emits
  `{"normalizedSession": …, "sessions": …}`, so every `NormalizedEvent` field
  that serializes appears in all 15 goldens. `wrapper_tool`, `uuid` and
  `parent_uuid` all carry `skip_serializing_if = "Option::is_none"` for exactly
  this reason. §8.3 follows that precedent.
- `NormalizedSession` JSON is **not persisted**: `store/schema.rs` stores only
  `metrics_json`, and the desktop uses `NormalizedSession` in memory only. That
  is what keeps `PARSER_REVISION` at 3 (§8.6).
- `ANALYZER_REVISION` (currently `5`) gates analysis cache reuse and **must** be
  bumped to 6.

### Desktop behaviour that constrains the design

`apps/desktop/src-tauri/src/analysis.rs` takes the merged metrics and then
overwrites `initial_context` and `skill_uses` with the parent's, and
`efficiency` with `parent + Σ child`. So the merged stream's own `efficiency`
and `skill_uses` are **not user-visible** on any shipped path.
`efficiency.rs`'s module doc already states the per-thread sum is correct. The
only observer of merged efficiency is
`merged_streaming_metrics_equal_the_merged_batch`.

### Desktop chart behaviour that bounds the visual risk

`sessionAnalysis.ts::contextTokenSeries` holds the last observed context level
across empty buckets, sets it to 0 at a compaction bucket, marks the buckets
after it `NaN`, then back-fills from the next observation. `forwardFillMode`
does the same for `model`, `thinkingMode`, `speed`.
`ContextTokensChart.tsx` renders a label on each `isCompactionBoundary` bucket.

Consequences used in §4.1: a one-bucket shift of a forward-filled 180-point
series is not observable; a **lost** compaction boundary is observable and
corrupts the line after it. That is why §5.5 puts compaction state in slots with
no cap.

---

## 4. The impossibility result, and the contract

Let `A = active_ms`. `A` only grows as records arrive, so the index
`floor(180 · cum_active(t) / A)` of an already-seen turn keeps moving until the
last record. Exactly one of {exact, bounded, one pass} must give.

| Option | Memory | CPU | Output | Decision |
|---|---|---|---|---|
| Compact turn rows (~32 B/turn, interned) | O(n), 10× smaller | unchanged | byte-identical | **Rejected** — still O(n) |
| Two-pass | O(1) | +100 % on the dominant cost | byte-identical | **Rejected** — §2 |
| Hybrid: exact rows under a cap, quantized above | O(1), large constant | unchanged | identical below the cap | **Rejected** — two complete implementations of every order-sensitive rule |
| **Bounded slot grid + bounded reorder window** | **O(1), ≤ 640 KiB** | unchanged, one fewer allocation per turn | contract below | **Chosen** |

### 4.1 The contract

1. **Totals are exact.** `Σ buckets[i].tokens_in == tokens_in`, and likewise for
   `tokens_out`, `subagent_tokens`, `cache_read_tokens`, `cache_write_tokens`,
   `user_prompts`, `subagent_launches`. Every scalar in `SessionMetrics` outside
   `buckets` is exact **except** in three named, counted and tested overflow
   regimes: more than `MAX_ACTIVE_SEGMENTS` idle gaps (§5.2), the capped maps of
   §5.8, and the model-attribution overflow of §6.7.
2. **Positions are exact when timestamp disorder is at most
   `REORDER_WINDOW − 1` records** (§5.3). Beyond that, an over-late record folds
   at the last folded position and a counter records it.
3. **Sessions with at most `SLOTS` metric-bearing turns and disorder within the
   window are bit-identical** to today: each turn owns its slot and is placed by
   its own exact position with the same `f32` arithmetic. This covers every
   characterization fixture and every unit-test fixture. It does **not** cover a
   typical field session: `BASELINE.md`'s 10 MiB tier is 34,361 turns, so
   **drift is the normal case at scale**. §10.6 sizes the constant from data.
4. **Above that, cell compaction can cross progress-bucket boundaries.** The
   implementation doubles the position quantum only when a new distinct cell
   would exceed `SLOTS`. It publishes no fixed displacement bound.
5. **Compaction, cache-miss, model, mode, speed, `last_tool` and gap facts move
   with the tokens they belong to**, because they live in the same slot (§5.5).
   No marker channel can disagree with the token channel.
6. **Ordering semantics are preserved exactly, independent of fold order**,
   because every non-additive slot field carries the arrival ordinal that
   decides it.
7. **Named visible artefact.** Above `SLOTS` turns, a high-context
   pre-compaction turn that today shares the boundary bucket (and reads 0) can
   land in an earlier bucket and render its true context before the reset. §9.2 pins it with an above-`SLOTS` test.

Points 3, 4 and 7 are behaviour changes needing sign-off before stage 0 (§13).

---

## 5. Chosen data structures

All live in a new `metrics_sink/` module directory (§8.1).

### 5.1 Constants

```
BUCKETS             = 180     // unchanged, engine::BUCKETS
SLOTS_PER_BUCKET    = 3       // selected to keep saturated state below 640 KiB
SLOTS               = 540     // BUCKETS * SLOTS_PER_BUCKET
REORDER_WINDOW      = 64      // records; tolerates displacement <= 63
MAX_ACTIVE_SEGMENTS = 1_024   // distinct intervals retained before compaction
MAX_OPEN_MESSAGES   = 64      // efficiency message-fragment window
MAX_EFF_REORDER     = 32      // efficiency turn reorder window
MAX_SKILL_USES      = 256
MAX_SKILL_NAMES     = 64      // the by-name counter map behind the cap
MAX_LATE_CANDIDATES = 256
MAX_DEFERRED_CACHE  = 8       // mode-2 resolutions awaiting summary.model
MAX_TOOL_NAMES      = 256     // metrics-side; §5.8
MAX_MCP_SERVERS     = 128
MAX_MODELS          = 32      // matches evidence::MAX_MODELS
MAX_MODEL_RUNS      = 32
MAX_THINKING_MODES  = 64
MAX_SPEEDS          = 64
MAX_SKILL_NAME_BYTES = 192    // each retained skill display name
```

Every constant needs an ASD-STE100 comment that states **why the bound is
safe**, in the style of `CACHE_REHYDRATION_MIN_GAP_SECS`. Keep each comment to
one idea and ≤ 25 words, and state a fact rather than narrating the design, so
`ai-slop/narrative-comment` and `ai-slop/meta-comment` stay quiet (§11).

### 5.2 `ActiveSegments` — exact active time from an interval union

Identity, derived from `finalize_metrics` (`active_ms = Σ clamp(gap, 0, G)` over
sorted timestamps, `G = IDLE_GAP_MS`):

```
active_ms      = max(0, |⋃ᵢ [tᵢ, tᵢ + G]| − G)      // 0 for 0 or 1 timestamps
cum_active(t)  = |⋃ᵢ [tᵢ, tᵢ + G] ∩ (−∞, t]|
```

A set union is associative and commutative, so this is order-insensitive,
reproduces today's "sort every timestamp first" result exactly including
duplicates, and merges across streams by construction.

- State: a sorted list of disjoint `[start, end]` segments plus a prefix-sum
  array. A timestamp at or after the last segment's start extends it or opens a
  new one, O(1) — the monotone path.
- An earlier timestamp that falls **inside** an existing segment changes
  nothing: no insert, no prefix rebuild. This is the overwhelmingly common
  disorder case, because real disorder is seconds while a segment spans at least
  `IDLE_GAP_MS`.
- An earlier timestamp that opens a **new** segment costs a binary search, a
  memmove and a prefix rebuild, both bounded by `MAX_ACTIVE_SEGMENTS`. It
  requires a disordered timestamp more than five minutes away from every
  existing segment. §10.4 adds a fully-disordered bench row so this cannot hide.
- **Implementation deviation:** overflow merges a new interval with its nearer
  neighbour and increments `segments_merged`. The compacted span retains the
  summed active duration and distributes it across the span for progress
  projection. This avoids counting the idle gap as active, but positions inside
  compacted spans remain approximate.
- `duration_secs` comes from **running min/max timestamp scalars**, as
  `((last_ts - first_ts).max(0) / 1000) as u64`, not from the segment list,
  whose last segment ends at `last_ts + G`.

### 5.3 `ReorderWindow` — what makes positions exact

Today's code already uses two orders, and the reducer must keep both:

- **Transcript order** drives every online reducer: tallies, tool and model
  maps, the cache chain, compaction state, candidate lists, `ActiveSegments`
  insertion, the efficiency reducer. This matches `finalize_metrics`, which
  walks the turn vector in transcript order.
- **Timestamp order** drives the slot fold only. This matches
  `active_progress`, which reads the *sorted* cumulative-active vector.

`ReorderWindow` is a ring of at most `REORDER_WINDOW` compact
`SlotContribution` values keyed by `(effective_ts, arrival_ordinal)`:

- `effective_ts` is the turn's own `ts_ms`, or the last timestamp seen earlier in
  this same stream (today's `last_progress` carry), or `i64::MIN` before the
  first timestamp.
- When the ring is full, pop the minimum key and fold it. A ring that pops the
  minimum at size `W` sorts any record displaced by at most `W − 1` positions;
  **disorder** here means "the number of records with a strictly larger
  effective timestamp that precede this record". `REORDER_WINDOW = 64` therefore
  tolerates disorder ≤ 63, and §9.2 tests 1, 63 and 64.
- `finish` flushes the ring in key order.
- A record whose key is below the last popped key folds at the last folded
  position and increments `reorder_window_overflow`.
- The ring is **never bypassed**, including on the ordinal axis (§5.4).

Cost: 64 × ~80 B ≈ 5 KiB, constant. This is a bounded reorder buffer, not turn
history.

### 5.4 `ProgressSlots` — the doubling-quantum position grid

- On the **active axis** a contribution's position is `cum_active(effective_ts)`
  computed at pop time; on the **ordinal axis** it is the arrival ordinal.
- Slot index = `position / quantum`, `quantum` starting at 1. When a new
  distinct index would exceed `SLOTS` occupied cells, merge adjacent pairs and
  double `quantum` until the contribution fits. Amortised O(1).
- The backing store is a `Vec<SlotAggregate>` that **grows lazily** to at most
  `SLOTS`. A 20-turn sub-agent occupies ~20 slots (≈4 KiB), not 540. This is
  what keeps small sessions from regressing and what makes §5.9's session-tree
  budget affordable.
- Each slot keeps `first_pos` and `first_ts` for its earliest occupant.
- **Placement at finalize** always recomputes from the *final* segments, so an
  ordinal-axis carrier and an over-late record are both placed correctly:

  ```rust
  let position = match axis {
      Axis::Active  => cum_active_final(slot.first_ts),   // 0 when first_ts is i64::MIN
      Axis::Ordinal => slot.first_pos,                    // the arrival ordinal
  };
  let denominator = match axis { Axis::Active => active_ms, Axis::Ordinal => observed_turns - 1 };
  let progress = (position as f32 / denominator as f32).clamp(0.0, 1.0);
  let index = ((progress * BUCKETS as f32) as usize).min(BUCKETS - 1);
  ```

  The `f32` expression is copied verbatim from `finalize_metrics` so rounding
  cannot drift. `observed_turns <= 1` yields progress 0, as today.

- **The degenerate axis and the flip (B1).** While `active_ms == 0` the grid is
  ordinal-keyed, reproducing today's `index / (len − 1)` fallback. Note that
  `active_ms == 0` holds **exactly when every observed timestamp is equal**,
  because sorted gaps are non-negative and clamp at 0; call that single value
  `T0`. When a second distinct timestamp arrives the axis flips, and the slots
  already folded are collapsed into **two carriers**, split at the ordinal of
  the first timestamped record:
  - carrier A, `first_ts = i64::MIN` — the leading untimestamped run, which has
    progress 0 today as well;
  - carrier B, `first_ts = T0` — every other pre-flip record, placed at finalize
    by `cum_active_final(T0)` like any other slot.

  Revision 2 collapsed everything into slot 0, which put the fixture's `ts = 10 s`
  user record in bucket 0 instead of bucket 60. Because the ring is never
  bypassed, in practice fewer than `REORDER_WINDOW` records can even be folded
  before a flip, so the carriers are usually empty; they are the correctness
  backstop for a session that stays single-timestamped for more than 64 records.
  A session whose timestamps are all identical never flips and keeps the ordinal
  axis, exactly as today.

### 5.5 `SlotAggregate` and the merge algebra

Every non-additive field carries the **arrival ordinal** that decides it, which
makes the algebra associative and fold-order independent. The same rule serves
doubling merges, slot→bucket folding, and cross-stream folding.

The **gate** column reproduces the `EventSource` gating in `finalize_metrics`;
without it §7.3's per-stream fold is ambiguous.

| Field | Gate | Type | Merge rule |
|---|---|---|---|
| `tokens_in`, `tokens_out`, `cache_read_tokens`, `cache_write_tokens` | Parent | `u64` | saturating add |
| `subagent_tokens` | Subagent | `u64` | saturating add |
| `context_tokens` | Parent | `u64` | `max` — raw; compaction zeroing is a finalize projection on output buckets |
| `user_prompts` | Parent + `Role::User` | `u32` | saturating add |
| `subagent_launches` | Parent, `Task` tools | `u32` | saturating add |
| `has_thinking` | Parent | `bool` | `or` |
| `first_pos`, `first_ts` | — | | the occupant with the smaller `first_pos` |
| `model`, `thinking_mode`, `speed` | Parent, non-empty | `Option<(u32, u16)>` | greater ordinal wins |
| `last_tool` | Parent, `tools.last()` | `Option<(u32, u16)>` | greater ordinal wins |
| `compaction` | Parent | `Option<(u32, Option<CompactionTrigger>, Option<u64>, Option<u64>)>` | greater ordinal wins, **as one atomic tuple** |
| `first_gap` | Parent, `context_tokens > 0` | `Option<(u32, u32)>` | smaller ordinal wins |
| `m1_rehydration_gap`, `m2_rehydration_gap` | Parent | `Option<(u32, Option<u32>)>` | greater ordinal wins |
| `m1_rehydration`, `m1_routing_miss`, `m2_rehydration`, `m2_routing_miss` | Parent | `bool` | `or` |

Notes on the four subtle ones:

- **`compaction` lives in the slot, not in a capped mark list.** First-N
  truncation would drop markers from the right edge of the chart, and a lost
  boundary corrupts `contextTokenSeries` after it (§3). A per-slot tuple needs no
  cap (at most `SLOTS` boundaries are representable), stays atomic, and places
  the boundary in the **same** bucket as the context tokens it must zero.
  `evidence::MAX_COMPACTION_BOUNDARIES = 64` is a different contract (a
  serialized evidence list) and is not a precedent here.
- **The cache fields are duplicated for both detection modes (B2).** The mode is
  chosen by `summary.cache_write_tokens_available`, which arrives at `finish`,
  and a folded slot write cannot be revoked after a doubling merge. Revision 2's
  "pending lists" were O(qualifying turns). Two field sets cost ~20 B per slot
  and finalize simply selects one pair. Two counter pairs shadow them, so
  `cache_rehydration_count` and `cache_routing_miss_count` are exact for either
  mode with no cap anywhere.
- **`first_gap` is "the first `Some` gap".** Today's rule writes `None` as a
  no-op for the first-wins branch, so only `Some` values count.
- **`*_rehydration_gap` is `Option<(ordinal, Option<u32>)>`, a genuine
  tri-state**, so "the last rehydration's own gap was `None`" is distinguishable
  from "no rehydration in this slot". The finalize projection is:

  ```
  secs_since_prior_turn = match rehydration_gap {
      Some((_, gap)) => gap,          // last rehydration wins; gap may be None
      None           => first_gap,    // else the first Some gap
  }
  ```

  which reproduces all eight left/right flag-and-value combinations.

Slot size ≈ 204 B. `SLOTS × 204 B ≈ 287 KiB` fully occupied.

### 5.6 Patching a target that the ring has not yet folded (B4/H-2)

Two mechanisms need to reach a turn after it was recorded: the mode-2 cache
resolution (decided one cache-turn later) and a late-tool candidate (resolved at
`finish`). One rule covers both:

- **Still in the ring** — patch the `SlotContribution` in place, addressed by
  arrival ordinal. The reorder ring is 64 deep and the cache ring is 3 deep, so
  this is the mode-2 path in practice.
- **Already folded** — patch the slot addressed by `position / current_quantum`,
  recomputed with the quantum in force at patch time. The position used is the
  one recorded **at pop time**, never a value recomputed later, so a patch and
  the turn's own tokens can never land in different slots.
- A late-tool candidate therefore stores `Option<position>`, filled in when its
  contribution pops. Candidates are matched by arrival ordinal.

### 5.7 `project()` — why `metrics(&self)` stays complete and pure (B5)

`metrics()` may run before `finish()` (`CompositeSink::metrics`, the desktop
inline test, §9.2's own test). It cannot flush the ring, so `project()`:

1. folds every slot into the output `Vec<Bucket>`;
2. folds each **residual ring entry** directly into its output bucket, using
   `cum_active_final(effective_ts)`;
3. applies the compaction zeroing to the output buckets last.

Nothing mutates, so `metrics()` stays `&self`, repeatable and complete.

### 5.8 Capped collections and the interner

Caps follow the `evidence.rs` idiom: first-N wins plus a truncation counter.
**Every headline counter stays exact past every cap**; the three exceptions in
§4.1 point 1 are the only degradations, and each is named.

| Collection | Cap | Note |
|---|---|---|
| `tool_calls_by_name` | 256 | keyed by **interned id**, not by an owned `String` (B6). Feeds `fill_use_counts` for builtin rows. Larger than `evidence::MAX_TOOL_NAMES = 128` because a metrics truncation is invisible in the DTO |
| `mcp_tool_calls` | 128 | interned key; feeds `fill_use_counts` for MCP rows |
| `model_breakdown` | 32 | interned key; overflow tokens fold into the unattributed accumulator (§6.7) rather than being dropped, so `cost` stays complete and only attribution degrades |
| `model_runs` | 32 | `(first_pos, first_ordinal, Option<model>, Option<mode>)`; resolved and deduplicated at finish |
| `skill_uses` | 256 | plus a by-name counter map capped at `MAX_SKILL_NAMES = 64`, so `fill_use_counts` stays right past the cap. That map is capped too (S6) |
| `late_tool_candidates` | 256 | slot-targeted effects only (§6.9) |
| Dedicated name stores | tools 256; MCP 128; models 32; thinking/speed 64 each; last tools 256; skills 64 | map keys cannot consume another category's capacity |

Truncation increments a private counter, emits one `tracing::debug!`, and is
asserted by in-crate tests, which read private fields. No public API and no
`SessionMetrics` field is added.

### 5.9 Retained-state budget (B6)

| Component | Saturated bytes |
|---|---|
| `ProgressSlots` (≤540 positioned aggregates) | ≤ ~190 KiB |
| Reorder window (≤64 aggregates) | ≤ ~22 KiB |
| Active segments and prefix | ≤ ~24 KiB |
| Efficiency state and ordered contributions | bounded at 1,440 contributions |
| Dedicated name stores, skills, late candidates, maps, and summary | bounded by per-category caps |
| **Measured saturation range** | **366,413–395,789 B** |
| **Combined cap-saturation test** | **Below 640 KiB** |
| **`RETAINED_METRICS_BYTES_BOUND`** | **640 KiB** |

The in-crate combined cap-saturation test fills 1,440 efficiency contributions,
540 progress cells, every name category, skill and late-tool caps, and maximum
bounded summary data. It remains below the blocking threshold.

**Per session tree, not per accumulator.** `stream_claude_with_hooks` holds one
accumulator per input, parent and every sub-agent, until `merge_metrics`. The
budget the always-running rule pays is `(N+1) × footprint`. Lazy growth keeps a
50-turn sub-agent small. The measured tree of one 10 MiB parent plus 20
50-turn sub-agents retains 867,833 derived bytes, below the 1 MiB acceptance limit.
A pathological tree of many large threads multiplies the per-accumulator cap.

---

## 6. Every order-sensitive calculation, and its bounded handling

### 6.1 Compaction boundaries stay visible

- `compaction_count` increments online for `EventSource::Parent` turns only.
- A boundary writes the slot's `compaction` tuple with its arrival ordinal, so
  two boundaries in one slot keep the last one's triple atomically, including
  when the last one's metadata is `None` and an earlier one's was `Some`.
- `context_tokens = 0` is applied once, at the end, to output buckets, which
  preserves
  `claude_compaction_sharing_bucket_with_pre_compaction_turn_still_resets`.
- `first_turn_after_compaction` is a sticky online scalar, cleared only by a
  context-bearing parent turn.
- No cap.

### 6.2 Context resets and `secs_since_prior_turn`

`previous_turn_ts` advances only on parent turns with `context_tokens > 0` — one
scalar, exact. Slot storage uses the `first_gap` / `*_rehydration_gap` pair of
§5.5 and the projection reproduces today's rule. Writes follow §5.6.

### 6.3 Cache rehydration and routing misses

- **Mode 1** (`cache_write_tokens_available == true`, the Claude path): the
  predicate reads `previous_context`, `previous_cache_read`,
  `first_turn_after_compaction` and the gap. Four scalars; decided at the turn;
  writes the `m1_*` fields.
- **Mode 2** (inference, the non-Claude path): a 3-entry ring of `CacheTurn`
  replaces `cache_turns.windows(3)`; the middle entry is decided when the third
  arrives and writes the `m2_*` fields through §5.6 (normally still in the
  reorder ring).
- Both run always; **finalize selects** by `summary.cache_write_tokens_available`
  and reads only that mode's fields and counters. `metrics()` before `finish()`
  uses `SessionSummary::default()`, i.e. mode 2, exactly as today.
- `gap_allows_rehydration(None) == true` is preserved: a resolution with a `None`
  gap classifies as a rehydration and writes
  `rehydration_gap = Some((ordinal, None))`.
- `same_known_model` compares an `active_model` that today falls back to
  `summary.model`, unknown mid-stream. A mode-2 resolution stores
  `Option<interned>` model ids where `None` means "resolve against
  `summary.model` at finish". Because `same_known_model` returns `true` whenever
  either side is `None`, deferral is needed only across the single transition to
  the first model-bearing turn: at most a couple of windows.
  `MAX_DEFERRED_CACHE = 8` is a backstop with a counter, not a working limit,
  and the counters stay exact because no realistic stream reaches it.
- The parent-only filter is preserved.

### 6.4 Active time

§5.2. Exact and order-insensitive outside the segment-cap overflow regime.

### 6.5 Efficiency

`EfficiencyReducer` in `efficiency.rs`:

- It observes **every** record, not only assistants, because today's `last_ts`
  carry is updated by non-assistant records too.
- **Every** assistant turn enters the ring, including turns with **no
  `message_id`** (M-5). Today an id-less record is pushed straight into `turns`
  and is final immediately; if the reducer emitted it directly while an earlier
  id-bearing turn was still open, emission order would stop matching creation
  order and the `f64` accumulation order would change.
- `OpenMessages` is a **FIFO** ring of `MAX_OPEN_MESSAGES = 64` slots, evicting
  the oldest, so emission order equals creation order. 64 rather than 16 because
  `finalize_metrics` feeds **all** sources into one reducer and a parent
  transcript interleaves sidechain records (S2). The id is owned or interned,
  because `EfficiencyInput::message_id` borrows the event.
- **Model back-fill on merge** (M-5): when a later fragment merges into an open
  message, `if turn.model.is_none() { turn.model = event.model }`. Omitting this
  changes pricing for fragmented messages whose first record carries no model.
- A finalized turn with `output_tokens == 0` is dropped, matching today's
  `retain` **after** the message merge.
- `ReorderWindow` of `MAX_EFF_REORDER = 32` finalized turns, emitted in
  non-decreasing `ts` **tie-broken by creation order**, reproducing today's
  stable sort for local disorder ≤ 31. Duplicate timestamps are common, so the
  tie-break is load-bearing for `SessionMetrics: PartialEq`.
- Forward pass: `prev_ctx: Option<u64>` plus `EfficiencyTotals`.
- **Documented degradation.** Today `index_by_id` is never cleared, so a
  `message_id` recurring after thousands of messages still merges. With a
  64-slot window it becomes a new turn. The Claude adapter's `dedup_usage`
  subtracts the running max for that id, so a late duplicate carries near-zero
  usage and is usually dropped by the zero-output rule. §9.2 pins the boundary.
- `thread_efficiency` keeps its signature and delegates, so `efficiency.rs`'s
  unit tests are unchanged.

### 6.6 Skill timing

- Each `Skill` tool call appends a mark with `(ordinal, tool_index, position,
  ts, interned name, tokens_out, context_tokens)`.
- `progress` is recomputed from the exact position at finalize.
- **`duration_ms` resolves on the ring's sorted pop stream** (S1). The ring emits
  in non-decreasing timestamp order, so `min{ts' > ts}` is simply the first
  popped timestamp strictly greater than the mark's. A min-heap of unresolved
  marks keyed by mark timestamp resolves each mark exactly once, amortised
  O(log k), with `duration = min(t' − ts, IDLE_GAP_MS)`. A mark whose timestamp
  is not exceeded stays `None`, exactly as today. A mark whose turn has no
  timestamp never enters the heap.
  This deletes revision 2's `MAX_PENDING_DURATION`, `RecentTimestamps`, the
  seeding rule, one documented degradation, and an O(n·64) per-record scan aimed
  straight at §10.5's timing gate.
- **Order** is `(ordinal, tool_index)` (S7), which is today's order: skills are
  emitted per turn in `tools` order, and a late tool is appended to the turn's
  `tools`, so it sorts last **within its own turn**. Late-resolved marks take
  `tool_index = TOOL_INDEX_LATE + k`.

### 6.7 Model, mode, speed, `last_tool`, `model_runs`, `model_breakdown`

- Per slot: interned "last parent value wins" by ordinal, empty strings ignored,
  sub-agent turns never write.
- **The `summary.model` fallback cannot be applied online.** `finalize_metrics`
  attributes a turn with tokens but no model to `summary.model`, unknown
  mid-stream. The reducer keeps an **unattributed** `ModelTokens` accumulator
  and a capped list of raw `(first_pos, first_ordinal, Option<model>,
  Option<mode>)` run entries; at finalize the unattributed tokens fold into
  `summary.model`'s breakdown entry and the runs are resolved, deduplicated and
  ordered by `(first_pos, first_ordinal)`. Without this, `cost` changes on
  `delegated_model_missing`, whose golden contains `cost`.
- Tokens for a model beyond `MAX_MODELS` also fold into the unattributed
  accumulator (S5), so the headline `cost` stays complete and only per-model
  attribution degrades. That is the third named exception in §4.1 point 1.

### 6.8 `wrapper_tool`

`finalize_metrics` increments `tool_calls_by_name[wrapper]` for every turn
carrying `wrapper_tool` (the Codex `exec` wrapper), for **all** sources, not
parent-only. A plain online counter bump. No Claude fixture can catch a
regression, so §9.2 adds a Codex `exec` test.

### 6.9 Late tools

`SessionSummary::late_tools` patches a past turn by ordinal, with unbounded
lookback: the `<command-name>` record can be ordinal 0 while the skill marker
arrives at the last record.

- `NormalizedEvent` gains `may_resolve_late_tool: bool` (§8.3 for the serde
  attribute, which is load-bearing).
- `vendors/claude.rs` sets it for a known skill command or an unknown custom
  command. It excludes known built-in commands because they cannot resolve to a
  skill. Unknown custom commands remain candidates because their skill marker
  can arrive later. `state.ordinal` increments once per emitted `MetricsEvent`,
  matching the sink's index — verified.
- **Name-derived effects need no candidate (B3).** Today `finish` only requires
  `self.turns.get_mut(ordinal)` to succeed, i.e. `ordinal < observed_turns` — a
  scalar test. `tool_calls_by_name += 1` and the MCP count apply from that test
  alone, so they never degrade with the candidate cap.
- **Slot-targeted effects need a candidate**: `subagent_launches` for a parent
  `Task`, `last_tool` when the candidate's ordinal exceeds the slot's, and the
  `SkillUse` (which needs the turn's `tokens_out`, `context_tokens` and
  position). Candidates are capped at 256 and bind their position at pop time
  (§5.6). Past the cap a slash-command skill still counts in
  `tool_calls_by_name["skill"]` but produces no `SkillUse`; a counter records it.
- Every candidate joins the duration heap of §6.6 from creation, so a late skill
  at ordinal 0 still gets its exact `duration_ms`.
- An ordinal past the end is silently dropped, as today.

Alternatives recorded for sign-off: a new `NormalizedRecord` variant (rejected —
every sink matches the enum exhaustively, so `SessionCollector` and the evidence
sink would churn for a metrics-only concern), and resolving late tools entirely
inside the adapter (rejected — the adapter cannot know the turn's chart
position). §9.2 asserts every non-Claude adapter leaves the flag `false`.

### 6.10 `earliest_ts_ms` and `context_available`

`earliest_ts_ms` is a running minimum — `min`, not "first".
`context_available` (`agent != "claude" || summary.context_window.is_some()`) is
a finish-time scalar, unchanged, and named here because `aggregate_metrics`
reads it (L-2).

### 6.11 `metrics()` stays `&self`, pure and repeatable

`finish` stores the summary and resolves late-tool candidates; `metrics()`
performs only the pure projection of §5.7.

---

## 7. Parent / sub-agent behaviour and the merge

### 7.1 What today's merge actually does

`merge_metrics` interleaves the parent's turns with each sub-agent's turns by
carried timestamp (stable sort) and re-runs `finalize_metrics`.
`metrics_sink::push_stream` assigns the passed-in `EventSource` to **every** turn
of a stream, and `merge::push_stream` does the same on the batch path. So:

- A record the parent adapter tagged `Subagent` from `"isSidechain": true` is
  re-tagged **`Parent`** in the merge. Five characterization fixtures contain
  `isSidechain` (`delegated_models`, `delegated_turns`, `delegated_model_missing`,
  `reasoning_and_fast_mode`, `thread_identity_chain`), and Claude writes sidechain
  records into parent transcripts in the field.
- A sub-agent input's own records — which `vendors/jsonl::parse_record` tags
  `Parent` unless sidechained — are re-tagged **`Subagent`**.

Consequences today: `parent.metrics()` and `merge_metrics(parent, …)` classify
the *same* parent-file sidechain records differently, so the merged series
counts them in `tokens_in`, `context_tokens`, `peak_context_tokens` (which feeds
`resolve_context_window`), the cache chain and the mode fields. There is also a
plausible double-count when a sub-agent's own transcript is passed as a child
input *and* its turns appear as sidechain records in the parent file.

### 7.2 The decision this forces

| Option | Cost | Output |
|---|---|---|
| **(a) Preserve the re-tag exactly** | The parent accumulator must also hold parent-flavoured aggregates for its sidechain turns: a second slot grid plus a second cache chain, compaction state and tallies. ≈2× the §5.9 budget and ≈2× the hardest logic | Unchanged |
| **(b) Honour each turn's own `EventSource` inside the parent input** | Small | Sidechain tokens move from the parent series to the sub-agent series; parent context occupancy, peak context and the cache chain stop seeing another context window's turns; the `context_window` tier can drop |

**Option (b), stated precisely (H-1)** — "stop re-tagging" is wrong in the other
direction, because it would promote sub-agent inputs' `Parent`-tagged records
into the merged parent series:

> Re-tagging stays at **stream granularity**: every slot of a **sub-agent input**
> contributes as `Subagent`. Inside the **parent input**, each turn's own
> `EventSource` is honoured, so the parent accumulator's slots carry both the
> parent-flavoured fields and a `subagent_tokens` accumulator for the parent's
> own sidechain turns. `SlotAggregate` already has that field, so the shape does
> not change.

**Recommendation: (b)**, because `analyze_session` on a lone transcript already
classifies those records as `Subagent` (so today's two views disagree), because a
sidechain turn genuinely runs its own context window, and because (a) doubles the
memory this plan exists to reduce.

Stage 0 pins today's behaviour first with a new synthetic fixture
`sidechain_in_parent.jsonl` and a test on `merge_metrics(parent, &[])`, so the
delta is measured rather than discovered.

### 7.3 Bounded chronological merge

```
merge_metrics(parent, subagents) -> SessionMetrics
 1. segments  = union(parent.segments, each subagent.segments)
    active_ms = max(0, |segments| - IDLE_GAP_MS)
    duration_secs = ((max_ts - min_ts).max(0) / 1000) as u64   over every stream
 2. buckets = [Bucket::default(); 180]
    for each parent slot in position order:
        fold into buckets[place(cum_active_final(slot.first_ts))]
          with the §5.5 algebra and gates
    for each sub-agent slot:
        buckets[place(...)].subagent_tokens += slot.tokens_in + slot.tokens_out
                                              + slot.subagent_tokens
 3. additive tallies: parent + Σ subagents (each accumulator's tallies already
    sum every source). Recompute parent cache detection in chronological slot
    order. Keep peak context, compaction count, and context window parent-only.
 4. tool_calls_by_name / mcp_tool_calls / model_breakdown: capped merge over
    every stream (the tool loop is not parent-gated today). Each stream's
    unattributed tokens attribute to the **parent's** summary.model, as today
 5. model_runs: merge by (position, stream index), resolve, dedup, cap
 6. skill_uses: merge by (stream index, ordinal, tool_index), cap
 7. efficiency: parent.efficiency + Σ subagent.efficiency
 8. zero context_tokens on every output bucket flagged as a compaction boundary
```

Step 2's sub-agent formula is correct for both child tags: a child slot tagged
`Subagent` has `tokens_in/out == 0`, and one tagged `Parent` has
`subagent_tokens == 0`. No raw-turn sort exists anywhere; the work is
`O(streams × SLOTS)`. Parent-only fields come only from the parent's slots, so
no cross-stream ordinal comparison is ever needed.

### 7.4 Deliberate divergences from today's merge

All five are invisible on shipped desktop paths, and §9.3 names them as the
exempt list of the merge equality test.

1. **`merged.efficiency` is the per-thread sum.** The desktop already overwrites
   the merged value with exactly that. `merge_metrics` gains an STE-100 sentence
   stating the rule, so the crate's contract does not depend on a desktop
   convention.
2. **`merged.skill_uses[].duration_ms`** uses the parent stream's successor
   timestamp. The desktop replaces merged `skill_uses` with the parent's.
3. **Untimestamped sub-agent turns** take their own stream's carried timestamp.
   Today `merge_subagent_events` carries `last_ts` per stream for the sort key,
   but `finalize_metrics` then carries `last_progress` across the merged stream.
   Affects only which bucket `subagent_tokens` land in for untimestamped
   sub-agent records.
4. **The degenerate merged axis.** When merged `active_ms == 0`, today falls back
   to `index / (len − 1)` over the interleaved stream. The bounded merge uses each
   stream's own ordinal axis. Note the related case (S4): a sub-agent whose
   records all share one timestamp keeps its **own** ordinal axis, yet §7.3
   places its slots by `first_ts` on the merged axis — which is correct, because
   §5.4 guarantees every ordinal-axis slot carries a meaningful `first_ts`
   (`T0` or `i64::MIN`), and today those turns all land in `bucket(cum_active(T0))`
   as well. §8.8 adds a merge fixture for it.
5. **Sidechain classification** under option (b): `merge_metrics` diverges from
   `merge_subagent_events` + `analyze_session` for any sidechain-bearing parent,
   because §7.5 leaves the batch merge unchanged. Today's two merge fixtures
   (`parent_with_task_spawn`, `subagent_child`) contain zero `isSidechain`
   records, so the shipped equality test does not observe it — but the plan
   states the property rather than implying a general one.

### 7.5 The non-Claude desktop path

`merge_parent_and_subagents` + `analyze_session` keeps working unchanged, because
`from_parts` replays a pre-merged stream into the same reducer. It is not
rerouted through `merge_metrics`: a larger desktop diff for no user-visible gain,
and it would erase `merge_subagent_events`' only test coverage.

---

## 8. File-by-file steps

### 8.1 `metrics_sink.rs` → `metrics_sink/` (stage 0, pure move)

`git mv` plus `mod` wiring **in stage 0**, before any behaviour change, so
stages 1–6 stay small and individually revertable.

| New file | Contents |
|---|---|
| `metrics_sink/mod.rs` | `MetricsIdentity`, `OnlineTallies`, `SessionMetricsAccumulator`, `RecordSink` impl, `project`, `merge_metrics`, `mcp_server_name`, `resolve_context_window` |
| `metrics_sink/active.rs` | `ActiveSegments` |
| `metrics_sink/slots.rs` | `Axis`, `SlotAggregate`, `ProgressSlots`, `ReorderWindow`, `SlotContribution`, the §5.5 algebra, the §5.4 flip |
| `metrics_sink/tally.rs` | `Interner`, `CappedMap`, skill marks, late-tool candidates, the duration min-heap |
| `metrics_sink/cache_miss.rs` | mode 1, mode 2's 3-entry ring, the §5.6 patch rule, deferred model resolution |
| `metrics_sink/reference.rs` | `#[cfg(test)]` only: today's `MetricTurn` + `finalize_metrics`, verbatim |
| `metrics_sink/tests.rs` | the new in-crate tests (§9.2) |

Deletions at stage 5: `MetricTurn`, `turns`, `push_stream`, `CacheMissEvents`
and its `HashSet<usize>`, the `timestamps` / `cumulative_active` / `cache_turns`
/ `skill_event_indices` vectors, and the `&[(EventSource, &MetricTurn)]`
finalize signature.

API changes (tests and benches only): `retained_turns()` → `observed_turns()`;
`retained_bytes()` keeps its name and reports bounded reducer-owned state from
`capacity()` where a collection can over-allocate. Exact identity strings are
additional and remain available in projected metrics.

Unchanged: `new`, `metrics(&self)`, `earliest_ts_ms`, `from_parts`,
`merge_metrics`, the `RecordSink` impl.

### 8.2 `efficiency.rs`

Add `pub(crate) struct EfficiencyReducer` with the FIFO open-message ring, the
model back-fill, id-less turns entering the ring, and the creation-order
tie-broken reorder window. `thread_efficiency_from_inputs` and
`thread_efficiency` delegate. Public signatures and unit tests unchanged.

### 8.3 `model.rs` — the serde attribute is load-bearing (B-1)

```rust
/// True when the vendor can resolve a tool call for this event only after the
/// last record. The metrics sink reserves a slot-targeted candidate for it.
/// The Claude adapter sets it for potential skill commands. No other vendor
/// path may depend on it.
#[serde(default, skip_serializing_if = "std::ops::Not::not")]
pub may_resolve_late_tool: bool,
```

`skip_serializing_if` is mandatory, not stylistic: the 15 goldens serialize
`normalizedSession`, so a plain `#[serde(default)] bool` emits
`"mayResolveLateTool": false` on every event and changes all 15 goldens.
`wrapper_tool`, `uuid` and `parent_uuid` already use this pattern. `false` in
`NormalizedEvent::new`.

### 8.4 `vendors/claude.rs`

Set `event.may_resolve_late_tool` when a command matches a loaded skill or is
not a known built-in command. The second case preserves skills whose marker
arrives later. A characterization test sends more than 256 built-in commands
before a late skill and proves they do not consume the candidate budget.
**Do not** cap `pending_commands` here: it is drained in `into_summary` and also
consumed by `SessionCollector::into_session`, so a cap would change the batch
path's normalized output. §12 tracks it.

### 8.5 `engine.rs`

No field changes. Extend the `Bucket` doc comment with the drift, the
compaction-zeroing rule and §4.1 point 7's artefact (STE-100).

### 8.6 `analysis/mod.rs`

`ANALYZER_REVISION: 5 → 6`. `METRICS_SCHEMA_REVISION` stays `1`.
`PARSER_REVISION` stays `3`, resting on two verified facts: no store column and
no export field carries `NormalizedEvent`, and §8.3's attribute keeps the
serialized shape byte-identical.

### 8.7 `apps/desktop/src-tauri/src/analysis.rs`

No desktop behavior change is expected. A comment-only update can document the
engine's new description bound without changing runtime output.

### 8.8 Tests, fixtures, docs and benches touched

- **`src/analysis/tests.rs`** (2,560 lines, ~97 in-crate tests) is an **audited
  surface** (H-3). Most of its metrics tests route through `analyze_session` /
  `from_parts` and must pass unchanged, including
  `claude_compaction_sharing_bucket_with_pre_compaction_turn_still_resets`,
  `two_compactions_sharing_a_bucket_keep_the_last_triggers_and_sizes`,
  `bucket_keeps_the_gap_that_enters_it_and_counts_user_prompts`,
  `cache_rehydration_is_detected_when_the_prefix_stays_cached`,
  `rehydration_detection_ignores_subagent_turns`,
  `mode_signal_bucket_keeps_the_last_value_seen_in_it`,
  `subagent_mode_signals_never_override_the_parent_buckets`,
  `subagent_tokens_use_their_own_bucket_series_time_aligned`,
  `claude_skill_use_is_captured_with_position_tokens_and_duration`,
  `duplicate_claude_message_ids_are_deduplicated`,
  `active_time_excludes_idle_gaps`,
  `codex_exec_wrapper_recovers_current_tool_categories`. The three that touch
  the merge —
  `peak_context_and_context_window_stay_parent_only_after_merge`,
  `idle_gap_counts_only_when_every_stream_is_idle`, and any sidechain-sensitive
  assertion — are re-pinned in stage 6 under the §7.2 decision. Each stage names
  which of these it must not break (§13).
- **New in-crate tests** go in `metrics_sink/tests.rs`, not in `tests.rs`.
- **New synthetic fixtures**: `sidechain_in_parent`, `late_skill_metrics`,
  `two_compactions_second_without_metadata`, `rehydration_gap_none`,
  `disorder_ladder`, `subagent_single_timestamp` (§7.4 case 5). For each, decide
  and record: whether it joins `fixture_names()` (15, metrics goldens), whether
  it joins `evidence_fixture_names()` (18), and its row in the fixtures
  `README.md` "Proves" table — all three are required per file (M-6).
- **`tests/streaming_metrics_memory.rs`** needs
  `#[path = "support/corpus.rs"] mod corpus;` added; today it has an inline
  generator and no corpus wiring (M-1).
- **`tests/pipeline_corpus.rs`** — per-turn bound replaced by the absolute bound.
- **`tests/claude_characterization.rs`** — merge equality re-pinned with the
  §7.4 exempt list; the self-comparison caveat commented on the two batch
  equality tests.
- **`tests/support/corpus.rs`** — one opt-in `SessionSpec::thread_identity` flag
  adds chained `uuid`/`parentUuid` values for the 500 MiB bench. Existing specs
  stay unchanged. `memory_baseline.rs` constructs its saturation profile
  directly with skills, MCP tools, modes, speeds, models, and tool names.
- **Benches** — four `retained_turns()` call sites (M-7). The
  `retained_bytes()/retained_turns()` per-turn ratio printed by
  `pipeline_baseline.rs` becomes meaningless; replace it with
  `retained_bytes()` plus `observed_turns()` printed separately, and update the
  "~303 bytes/turn" line in `BASELINE.md` to a bounded-footprint line.

### 8.9 `docs/plans/local-insights-architecture.md` (H-5)

Three passages record the unbounded-metrics acceptance this plan removes and go
stale on merge:

- the bounded-memory guarantee paragraph ("…does not initially apply to every
  retained collection needed for exact existing `SessionMetrics` output…");
- the metrics-contract paragraph ("Exact parity … can require retained state
  proportional to the number of metric-bearing events. … The first
  implementation accepts this existing unbounded metrics behavior.");
- CH-005's acceptance clause ("…document the remaining proportional growth of
  retained metrics state.").

Update all three in stage 7, stating that session-metrics retained state is now
bounded, naming the drift contract, and naming the residual that remains outside
`metrics_sink` (§12).

---

## 9. Tests

### 9.1 The differential oracle is the real gate

`analyze_session` routes through `from_parts` into the same accumulator, so
`streaming_metrics_equal_the_shipped_batch_for_every_fixture` compares the new
implementation with itself — state that in the test's comment. The oracle is
today's `MetricTurn` + `finalize_metrics`, kept verbatim in
`metrics_sink/reference.rs` under `#[cfg(test)]`. `#[cfg(test)]` code exercised
by in-crate tests is not dead code, so no lint suppression is needed.
Integration tests in `tests/*.rs` **cannot** see it; that is why the drift gate
lives in-crate.

The oracle compares full `SessionMetrics` values with `PartialEq`, which covers
`skill_uses`, `mcp_tool_calls` and `tool_calls_by_name` — the three fields no
golden reaches.

Generator (an LCG in the style of `tests/support/corpus.rs`, no new dependency):
0–400 turns (small tier) and 2,000–6,000 turns (drift tier); present/absent
`ts_ms`; disorder 0–65; duplicate timestamps; gaps straddling `IDLE_GAP_MS`;
all-identical timestamps; compaction density; `message_id` fragment runs;
sidechain records; 0–3 sub-agents; `cache_write_tokens_available` both ways;
`summary.model` present/absent; `late_tools` at random ordinals including out of
range; `wrapper_tool` turns.

| Test | Assertion | Tier |
|---|---|---|
| `bounded_reducer_matches_the_retained_reference_for_small_streams` | `assert_eq!(bounded.metrics(), reference.metrics())`, no tolerance | default, 200 seeds |
| `bounded_reducer_drift_tier_preserves_exact_totals` | drift tier: additive totals and bucket sums remain exact | default |
| `bounded_reducer_drift_tier_extended` | 100,000-turn bounded-state stress | `#[ignore]` |
| `bounded_merge_matches_reference_chronology_for_small_streams` | one parent and one sub-agent, with documented merge differences | default |
| `empty_metrics_are_repeatable` | two `metrics()` calls are equal | default |
| `metrics_before_finish_include_every_observed_turn` | a stream ended without `finish`; repeated projections are equal | default |
| Large-session chart-shape tests | Uniform traffic populates all 180 buckets; repeated compaction and cache markers remain visible | default |

The `#[ignore]` tiers run **on demand at stage 5 review** and are named in the
PR body. The repository has **no scheduled workflow** (`ci.yml` triggers on
`push` and `pull_request` only), so this plan does not say "nightly" and does not
add one — creating a workflow would put `.github/` in the diff and flip
`classify-ci-changes.mjs` to `full`, pulling in the frontend and installer jobs
(M-2).

The default tier must run in **under 10 s** in the debug profile on one core;
measure at stage 0 and record it here, because `cargo nextest run` executes on a
three-OS matrix.

### 9.2 Unit tests (`metrics_sink/tests.rs`)

- **Algebra**: one test per row of §5.5, including the gate column, the raw
  `context_tokens` rule, the atomic compaction tuple, and ordinal-decided
  first/last-wins folded in reverse order to prove fold-order independence.
- **`secs_since_prior_turn` truth table**: all eight combinations, including
  "last rehydration with a `None` gap beats an earlier `Some` gap".
- **Position and axis**: `timestamps_repeated_and_out_of_order` reproduces
  buckets {0, 60, 179}; a disorder ladder at 1, 63 and 64, the last asserting the
  documented degradation and the overflow counter; **the ordinal→active flip**
  with more than `REORDER_WINDOW` single-timestamp records before a second
  timestamp arrives, asserting both carriers land where the reference puts them;
  an all-identical-timestamp session never flips.
- **Cache**: mode 1 and mode 2 written into their own slot fields and selected at
  finish; a stream where the two modes disagree, asserting each selection;
  deferred model resolution across the first model-bearing turn.
- **Invariants**: parent-only compaction counting; a compaction bucket reads zero
  when sharing with a high-context turn; the last compaction's triple wins with
  `None` metadata; `first_turn_after_compaction` survives zero-context parent
  turns; `gap_allows_rehydration(None)`; the unattributed-token fallback
  reproduces `delegated_model_missing`'s `cost`; `MAX_MODELS` overflow keeps
  `cost` complete; `metrics()` before `finish()`; `earliest_ts_ms` is a minimum;
  sub-agent mode signals never write parent fields; the final timestamp lands in
  bucket 179; an untimestamped turn inherits the previous transcript record's
  progress; `wrapper_tool` counts for all sources on Codex-shaped input; every
  non-Claude adapter leaves `may_resolve_late_tool` false.
- **Degenerate axes**: zero turns; one turn; every turn untimestamped with
  `len == 1` (progress 0) and `len > 1` (`index/(len−1)`); a leading
  untimestamped run; negative gaps clamped at 0; `secs_since_prior_turn` with
  `previous_turn_ts == None`; a sub-agent-only stream; `merge_metrics` with an
  empty sub-agent; `resolve_context_window` above the 1M tier.
- **Skills and late tools**: a late skill at ordinal 0 appears at `skill_uses[0]`
  with the reference's `duration_ms`; two skills in one turn keep `tool_index`
  order and a late tool sorts last within its turn; a skill in the final turn has
  `duration_ms == None`; past the candidate cap the skill still counts in
  `tool_calls_by_name` (B3) and a counter records the missing `SkillUse`; a late
  `Task` bumps `subagent_launches`; a late tool displaces `last_tool`; an ordinal
  past the end is dropped.
- **Efficiency**: fragments separated by more than `MAX_OPEN_MESSAGES` (pinned
  degradation); an id-less turn between two open messages preserving creation
  order; model back-fill on a fragment merge; duplicate timestamps; disorder at
  `MAX_EFF_REORDER` and `+1`; a non-assistant record advancing the carried `ts`.
- **Above-`SLOTS` artefact**: the §4.1 point 7 pre-compaction column, pinned.
- **Caps**: exceeding each cap keeps every counter exact and sets its counter.

### 9.3 Regression gate (`tests/claude_characterization.rs`) and its stated power

State the limits in the test module's comment: the largest fixture is 15
records, and three `SessionMetrics` fields are `skip_serializing`. The goldens do
cover `normalizedSession`, which is why §8.3's serde attribute matters.

- `streaming_metrics_match_every_golden` — byte-identical.
  `git diff --stat crates/antiburn-local/tests/fixtures/claude_characterization/goldens`
  must be empty at **every** stage, checked explicitly at stage 4 (the
  `NormalizedEvent` field) and stage 5 (placement). A changed golden is an
  escalation, not a re-baseline.
- `streaming_metrics_equal_the_shipped_batch_for_every_fixture` and
  `composite_metrics_json_equals_the_streaming_metrics_json_for_every_fixture` —
  unchanged, with the self-comparison caveat commented.
- `merged_streaming_metrics_equal_the_merged_batch` — compares every field
  exactly **except** the five named in §7.4, each with its own assertion or its
  own explanatory comment. No prose-only exemptions.
- New: `merge_metrics_honours_each_parent_turns_own_source` (option (b)) or
  `merge_metrics_retags_sidechain_turns_as_parent` (option (a)), pinned against
  `sidechain_in_parent`.

### 9.4 Merge invariants

- `merge_metrics(parent, &[])` equals `parent.metrics()` on every field the merge
  computes, with the exempt list spelled out as constants in the test:
  `initial_context`, `skill_uses[].description`, `initial_context.*.use_count`.
  Under option (a) the test is valid only for a parent with no sidechain records
  — state which.
- Keep `subagent_tokens_use_their_own_bucket_series_time_aligned`.

---

## 10. Benchmarks and measurable bounded-memory acceptance

### 10.1 Two independent measurements

`retained_bytes()` is an estimate this PR rewrites, so every memory claim is
asserted twice: by `retained_bytes()` in a test, and by an **allocator-observed**
figure in `benches/memory_baseline.rs` (which already installs a counting global
allocator) taken at end of stream **while the accumulator is still alive**, minus
the pre-stream baseline. The allocator figure also includes exact identity
strings. A disagreement of more than 25 % means the derived-state estimate is
wrong, and is a blocker.

### 10.2 Acceptance tests (`tests/streaming_metrics_memory.rs`)

The grid grows lazily, so retained bytes legitimately increase from a small
session to a saturated one; the O(1) claim is about **saturation**.

| Test | Assertion |
|---|---|
| `retained_state_stops_growing_once_saturated` | 40,000 and 400,000 records of the saturating profile: both stay below `RETAINED_METRICS_BYTES_BOUND`; sparse-cell capacity varies by at most 32 KiB |
| `retained_state_is_bounded_for_a_name_flood` | 5,000 distinct tool names, 5,000 skill names, 5,000 models: ≤ the same bound; every truncation counter non-zero |
| `retained_state_stays_small_for_a_small_session` | a 50-turn sub-agent-shaped session retains ≤ 32 KiB |

The saturating profile fills every cap; sparse occupied-cell capacity can still vary within the tested 32 KiB bound;
`tests/pipeline_corpus.rs` swaps its per-turn bound for the absolute bound.

### 10.3 Session-tree measurement

A bench row streams one 10 MiB parent plus 20 sub-agents of 50 turns, holds every
accumulator live as `stream_claude_with_hooks` does, and prints the summed
retained bytes and allocator-observed live bytes at the moment `merge_metrics` is
called. Accept ≤ 1 MiB.

### 10.4 Bench procedure

1. Record "before" numbers on `main`, on the machine that will run "after":
   `cargo bench --bench memory_baseline` and `--bench pipeline_baseline`. Keep
   the output; note toolchain and machine.
2. Extend the 500 MiB tier to print: source bytes; metrics-only peak; metrics
   retained bytes; allocator-observed live bytes; the residual (`peak − live`);
   composite peak; serialized evidence bytes. Add a saturation row at
   1/10/50/500 MiB over the saturating profile, a row printing the maximum
   occupancy of every capped collection (P3-26), and a **fully-disordered**
   10 MiB row that exercises `ActiveSegments`' insert path (S9).
3. Add an accumulator-only timing measurement over pre-normalized events (§10.5),
   reporting median and inter-quartile range over a stated run count.
4. Re-run after the change and update `BASELINE.md` in the same commit: the
   memory figures table, the 500 MiB tier table, the "~303 bytes/turn" line, and
   the sentence "the next memory win, if one is ever needed, is accumulator
   retention, not framing", which this change resolves.

### 10.5 Two methodology fixes

- **The timing gate cannot sit on the full-pipeline number.** `BASELINE.md`
  records the metrics stage at 66.4 ms against 67.0 ms for a **no-op sink** at
  10 MiB — "within noise of no-op", so a ±5 % gate there tests serde. Measure the
  accumulator in isolation: pre-normalize a 10 MiB session once, then time
  `record()` over the event vector. Accept ≤ 1.30× the same isolated measurement
  on `main`; report run count and IQR. A regression points at the duration heap,
  the reorder ring, or `ActiveSegments`' insert path.
- **Attribute the 271 MB residual with no new production API.** Run the 500 MiB
  tier twice: once with the corpus's unique `message.id` per record, once with
  ids repeating every 64 records. The delta is
  `ClaudeStreamState::max_usage_by_message_id`. Report both numbers.

### 10.6 Sizing `SLOTS_PER_BUCKET` from data

The fixtures cannot justify the constant (`SLOTS_PER_BUCKET = 1` passes the
golden gate). Before stage 5, estimate the field turn-count distribution from
**file sizes alone** — `BASELINE.md` gives ≈ 305 bytes/turn for a dense
transcript — and record here: median and p95 turn counts, the fraction of
sessions in the exact regime, and the expected drifting-turn fraction and slot
footprint at `SLOTS_PER_BUCKET` ∈ {3, 4, 8, 32}. No transcript content is read.
Then pick the constant and record the reasoning.

**Implementation record:** no field transcript inventory was read during this
workflow. `SLOTS_PER_BUCKET = 3` was selected from the 180-point chart ratio and
the 640 KiB retained-state budget. The implementation uses the planned
active-position quantum and compacts only when all 540 cells are occupied. This
keeps every session with at most 540 metric turns exact. It does not establish a
fixed drift bound above that limit. Field-derived median, p95, and exact-regime
fractions remain unavailable and are not claimed.

### 10.7 Acceptance thresholds

| Measure | Accept | Blocker |
|---|---|---|
| `retained_bytes()` at 40 k vs 400 k saturated records | variation ≤ 32 KiB; both ≤ 640 KiB | larger variation or either over bound |
| Allocator-observed live bytes vs `retained_bytes()` | within 25 % | outside 25 % |
| Metrics retained at 500 MiB | ≤ 640 KiB (from 525.6 MB) | > 1 MiB |
| 50-turn session retained | ≤ 32 KiB | > 64 KiB |
| One parent + 20 sub-agents, summed | ≤ 1 MiB | > 4 MiB |
| Metrics-only peak at 500 MiB | ≤ 0.60× source, residual attributed by §10.5 | > 0.60× source |
| Isolated accumulator time at 10 MiB | ≤ 1.30× the `main` measurement, same machine | > 1.30× |
| Fully-disordered 10 MiB row | ≤ 1.30× the ordered row | > 1.30× |
| Goldens | zero modified files, checked at stages 4 and 5 | any modified file |
| New dependencies | zero | any |

---

## 11. Commands, changelogs, and commit hygiene

`.github/workflows/ci.yml` is the contract; `CONTRIBUTING.md` is a subset.

```bash
cd crates/antiburn-local
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo nextest run --locked --lib --tests     # what CI runs
cargo test                                   # also covers doctests
```

**Three desktop Rust workspaces, not one** (M-4). `classify-ci-changes.mjs` sets
`desktop_backend: true` for any `crates/antiburn-local/` change, and CI then runs
`fmt --check` plus `clippy --all-targets --locked -- -D warnings` and
`test --locked` for `apps/desktop/src-tauri`,
`apps/desktop/src-tauri/crates/trace` and `apps/desktop/src-tauri/crates/hud`, on
three platforms. Neither `trace` nor `hud` depends on `antiburn-local`, so they
should pass untouched — run them anyway:

```bash
for dir in apps/desktop/src-tauri apps/desktop/src-tauri/crates/trace apps/desktop/src-tauri/crates/hud; do
  (cd "$dir" && cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked)
done
```

Dependency gates — the `licenses` job fires on any engine change, and all five
pass only with **zero new dependencies**, which is a stage exit criterion:

```bash
cd crates/antiburn-local && cargo deny --locked check bans licenses
cd crates/antiburn-local && cargo deny --locked check advisories
cd apps/desktop/src-tauri && cargo deny --locked check licenses
cd apps/desktop/src-tauri && cargo deny --locked check advisories
pnpm notices:check
```

Slop gate — `.aislop/config.yml` sets `failBelow: 100`, `maxFileLoc: 1500`,
`maxFunctionLoc: 1000`, `maxNesting: 5`, `maxParams: 6`, and these rules as
**errors**: `complexity/file-too-large`, `complexity/function-too-long`,
`ai-slop/empty-function`, `ai-slop/narrative-comment`, `ai-slop/meta-comment`,
`ai-slop/hardcoded-id`, `ai-slop/hardcoded-url`,
`ai-slop/rust-non-test-unwrap`, `ai-slop/todo-stub` (M-3).
`narrative-comment` and `meta-comment` are the live risk, because §5.1 mandates a
comment on ~15 new constants and §13 mandates STE-100 docs on `Bucket`,
`merge_metrics` and every new type: each comment must state a **fact about the
bound**, not narrate the design. `maxNesting` and `maxParams` constrain the
fold → patch → project path: keep helpers small and pass a context struct rather
than six arguments.

```bash
pnpm run slop:all
pnpm run slop            # already "aislop ci --changes --base origin/main" (L-4)
pnpm run secrets
```

Before stage 0, run `pnpm run slop:all` on today's tree and record the result
here: `src/analysis/tests.rs` is 2,560 lines and `evidence_sink.rs` is 1,819,
both above `maxFileLoc`, yet the gate is green — so the rule counts something
narrower than raw lines. New tests go in `metrics_sink/tests.rs` regardless.

Desktop JS checks only if `apps/desktop/src` changes (this plan expects not; CI
would otherwise also run `format` and `knip`):

```bash
pnpm install
pnpm --filter @antiburn/desktop format
pnpm --filter @antiburn/desktop lint
pnpm --filter @antiburn/desktop type-check
pnpm --filter @antiburn/desktop knip
pnpm --filter @antiburn/desktop test
pnpm --filter @antiburn/desktop build
```

Benches (stage 7 and any stage touching the hot loop):

```bash
cd crates/antiburn-local
cargo bench --bench memory_baseline
cargo bench --bench pipeline_baseline
```

The engine job is a **Linux / Windows / macOS matrix**; the `f64` bit-equality
claims (§6.5) must hold on all three, and `cargo nextest` on the matrix — not the
single-machine benches — is that oracle.

Throughout: no dead-code or deprecated-lint suppression; ASD-STE100 comments;
synthetic fixtures only; `git commit -s`. This workflow does not commit, push, or
open a PR.

### Changelog files

`crates/antiburn-local/CHANGELOG.md`, `## [Unreleased]`:

- *Changed*: `SessionMetricsAccumulator` retains bounded state. Cell compaction
  can move values between progress buckets. Additive totals remain exact
  outside documented collection caps.
- *Changed*: `merge_metrics` returns the per-thread efficiency sum, and — under
  §7.2 option (b) — honours each parent-transcript turn's own `EventSource`.
- *Changed*: **capped collections** (H-4), each named with its cap:
  `skill_uses` 256 (so a session past 256 skill invocations reports a shorter
  list, which the desktop skill list and the export payload's `skills` show),
  `tool_calls_by_name` 256, `mcp_tool_calls` 128, `model_breakdown` and
  `model_runs` 32. `export.rs::FORMAT_VERSION` stays 2 because no field changes
  shape.
- *Changed*: `retained_turns()` is replaced by `observed_turns()`;
  `retained_bytes()` reports bounded accumulator state.
- *Added*: `NormalizedEvent::may_resolve_late_tool`.
- *Changed*: `ANALYZER_REVISION` is 6, so cached analyses recompute once.

`CHANGELOG.md` (root), `## [Unreleased]`, under *Changed* — decided, not
deferred: "Analysing a very large session now uses far less memory." Phrased as
the memory a user's machine spends, never as a bounded-memory claim.

`docs/plans/local-insights-followups.md` — append the §12 items using that
file's five-field entry schema (What was found / Found by seam / Why deferred /
Kind / Disposition); CH-013 requires every `file-issue` entry to carry a real
issue number, so file the issues first (L-3).

---

## 12. Follow-ups this plan does not fix

Each gets a filed issue linked from the PR body before stage 7. These draft
items do not enter `local-insights-followups.md` until they have issue numbers.
The issue-filing exit criterion remains unmet in this uncommitted repair tree.

1. `ClaudeStreamState::max_usage_by_message_id` — one entry per distinct
   `message.id`, uncapped. §10.5 measures its share of the residual. This decides
   whether a user sees an RSS improvement at all, because after this plan the
   peak is still ≈0.5× source and still O(n).
2. `ClaudeStreamState::pending_commands` — uncapped, and drained into
   `SessionSummary::late_tools`, so any cap also changes the **batch** path
   through `SessionCollector::into_session`. Needs its own fixture.
3. `SessionEvidenceAccumulator::seen_thread_uuids` — uncapped `HashSet<String>`.
   Any cap must downgrade coverage to `Partial` with a `CoverageReason`, because
   absence must never be inferred from a cap.
4. `analyze_sources_with` / `merge_subagent_events` — the batch path still
   materializes `Vec<NormalizedEvent>` and clones it into `from_parts`.
5. Delete `metrics_sink/reference.rs` one engine release after stage 5 ships.
   Owner: the stage-7 author.

---

## 13. Sign-off, then staging

### Decisions needing maintainer sign-off **before stage 0**

1. **§7.2 option (a) or (b)** — preserve the sidechain re-tag at ≈2× memory and
   complexity, or honour each parent turn's own `EventSource` and accept a
   user-visible merged-chart change. Recommendation: (b).
2. **§4.1 points 3, 4 and 7** — the drift contract above `SLOTS` turns and the
   named pre-compaction artefact, including "a changed golden is an escalation".
3. **§7.4** — the five merge divergences.
4. **§6.9** — `NormalizedEvent::may_resolve_late_tool`, against the two recorded
   alternatives.
5. **§5.8 caps** — especially `skill_uses`, which shortens a user-visible list —
   plus `MAX_ACTIVE_SEGMENTS`, and `SLOTS_PER_BUCKET`, which §10.6 sizes.
6. **§6.5** — the efficiency message-window degradation.

### Stages

| Stage | Content | Exit criterion (named) |
|---|---|---|
| **0** | `git mv` to `metrics_sink/`; today's finalize into `reference.rs` under `#[cfg(test)]`; LCG generator and differential harness; the six new fixtures with **today's** behaviour pinned; `#[path]` corpus wiring in `streaming_metrics_memory.rs`; publish the §5.9 byte arithmetic, the §11 slop baseline and the §9.1 default-tier runtime | No behaviour change; `streaming_metrics_match_every_golden` plus the new pinning tests pass |
| **1** | `ActiveSegments` (sorted insert, running min/max `duration_secs`); finalize derives `active_ms` and `cum_active` from it while `turns` still exists | `streaming_metrics_equal_the_shipped_batch_for_every_fixture` (which asserts `f64` bits, unlike the goldens), `active_time_excludes_idle_gaps`, `idle_gap_counts_only_when_every_stream_is_idle`, the out-of-order fixture test |
| **2** | `EfficiencyReducer`: FIFO window, id-less turns, model back-fill, reorder window | `efficiency.rs` unit tests; `streaming_metrics_equal_the_shipped_batch_for_every_fixture`; `duplicate_claude_message_ids_are_deduplicated`; the new efficiency boundary tests |
| **3** | Online cache detection, both modes, into the dual slot fields; deferred model resolution | `cache_rehydration_is_detected_when_the_prefix_stays_cached`, `rehydration_detection_ignores_subagent_turns`, `bucket_keeps_the_gap_that_enters_it_and_counts_user_prompts`, `streaming_metrics_match_every_golden` |
| **4** | `Interner`; capped collections (own commit, own sign-off, own changelog bullet); skill marks and the duration heap; `wrapper_tool`; `may_resolve_late_tool` **with `skip_serializing_if`**; late-tool candidates; the unattributed-model accumulator | Cap tests; late-skill ordering and duration tests; `delegated_model_missing`'s golden; `codex_exec_wrapper_recovers_current_tool_categories`; **`git diff --stat` over the goldens is empty** |
| **5** | `ReorderWindow` + `ProgressSlots` (both axes, the flip carriers, lazy growth, doubling merge, ordinal-decided fields, `project`) — **the turn buffer is deleted here**; `ANALYZER_REVISION` → 6; `retained_turns` → `observed_turns` at all four call sites | `bounded_reducer_matches_the_retained_reference_for_small_streams`, `bounded_reducer_drift_tier_preserves_exact_totals`, `metrics_before_finish_include_every_observed_turn`, `retained_state_stops_growing_once_saturated`, compaction and mode-signal unit tests, skill-use characterization tests, and **zero modified goldens** |
| **6** | `merge_metrics` as the bounded chronological merge; the §7.2 decision applied; merged efficiency as the per-thread sum; merge tests re-pinned | `bounded_merge_matches_reference_chronology_for_small_streams`, `bounded_merge_without_children_matches_parent_projection`, `merged_streaming_metrics_equal_the_merged_batch`, and shared-axis tests |
| **7** | Benches, `BASELINE.md`, §10.6's sizing note, `local-insights-architecture.md` (§8.9), both changelogs, filed follow-up issues, STE-100 docs | Every command in §11; §10.7's thresholds; no desktop behavior change |

---

## 14. Risks and rollback

| Risk | Likelihood | Mitigation |
|---|---|---|
| A golden changes | Low, and now for two separate reasons: §8.3's serde attribute for the field, §5.3/§5.4 for placement | Checked at stages 4 and 5; the drift tier of §9.1 is what tests scale |
| Out-of-order transcripts behave differently | Medium — the shipped fixture proves disorder is real | `ActiveSegments` is order-insensitive; the ring restores exact placement to disorder 63; the ladder tests 1, 63, 64; the flip has its own test |
| The §7.2 sidechain decision changes the shipped chart | Certain under option (b) | Stage-0 pinning fixture, explicit sign-off, changelog entry |
| `f64` bits move in `efficiency` or `cost` | Medium | Creation-order tie-break, model back-fill, id-less turns in the ring, unattributed-token fallback; stage 2 and stage 4 assert bit equality; the 3-OS matrix is the oracle |
| The duration heap or `ActiveSegments` insert costs measurable CPU | Medium | §10.5's isolated measurement plus the fully-disordered bench row, both gated at 1.30× |
| The saturated footprint exceeds the budget | Medium | The combined cap-saturation test fixes `SLOTS_PER_BUCKET = 3` below the 640 KiB threshold |
| Cap overflow degrades a visible metric | Low, and named | Counters stay exact past every cap; compaction and cache flags have no cap; the three exceptions in §4.1 point 1 each have a counter and a test |
| Sub-agent trees multiply the footprint | Medium | Lazy grid growth; §10.3 measures a realistic tree at a 1 MiB gate |
| `ANALYZER_REVISION` bump forces re-analysis of every cached session | Certain | Bounded, one pass per session on one permit at ~133 MiB/s; existing behaviour; stated in the PR |
| The PR overclaims "bounded pipeline memory" | Medium | §1, §10.5 and §12 fix the wording and attach a number |

### Rollback

- Stages 0–4 are individually revertable and leave output unchanged.
- Stage 5 and stage 6 are the behaviour-changing commits, and stage 6 depends on
  stage 5, so the revert order is **6 then 5**.
- A rollback is a two-commit recipe: `git revert` the stage commits, **and** bump
  `ANALYZER_REVISION` to 7 so analyses produced by the reverted code recompute.
  `reference.rs` is `#[cfg(test)]` and contributes nothing to the revert; it is a
  test oracle, not a fallback path.
- `SLOTS_PER_BUCKET` is a single constant: raising it shrinks the drifting
  fraction with no structural change.

---

## 15. Review suggestions considered and rejected

| Suggestion | Rejected because |
|---|---|
| Give every marked turn its own slot so marks and slots cannot disagree | Slots are addressed by `position / quantum`; "force a new slot" is not expressible in a position-keyed array. Moving compaction and cache state **into** the slot removes the divergence at its root and removes two caps |
| Keep mode-1/mode-2 resolutions in pending lists and select at finish | Unbounded in the record count (B2). Duplicated slot fields cost ~20 B per slot and are exact |
| Use frequency-preserving caps (Misra-Gries) for `tool_calls_by_name` | Adds an approximate-counting algorithm and its test surface to protect a field whose realistic cardinality is in the low hundreds. A 256 cap with a counter is smaller and exact below the cap |
| Cap `pending_commands` in `vendors/claude.rs` defensively | It feeds `SessionCollector::into_session`, so a cap changes the batch path's normalized output. Moved to §12 as its own tracked change |
| Reroute the non-Claude desktop path through `merge_metrics` | A larger desktop diff for no user-visible gain, and it would erase `merge_subagent_events`' only test coverage |
| Add a `retained_bytes()` accessor to `ClaudeStreamState` for attribution | New production API whose only consumer is a bench; §10.5's repeating-`message.id` corpus measures the same quantity with no new code |
| Assert strictly flat `retained_bytes()` from the first record | Incompatible with lazy grid growth, which is what keeps small sub-agent sessions from regressing. §10.2 asserts saturation equality plus a small-session ceiling instead |
| Estimate `SLOTS` from the characterization fixtures | They are 2–15 records; they cannot distinguish `SLOTS_PER_BUCKET = 1` from 32. §10.6 sizes it from file-size-derived turn counts |
| Run the extended seed tiers nightly | The repository has no scheduled workflow, and adding one puts `.github/` in the diff and flips CI classification to `full`. They run on demand at stage 5 review (M-2) |
| Keep a per-model pending map to keep mode-2 counters exact past a cap | Over-engineered: `same_known_model` is `true` whenever either side is `None`, so deferral spans only the transition to the first model-bearing turn. A backstop of 8 with a counter is enough |
