# Pipeline measurement baseline (issue #224)

Indicative local baseline from one machine — **not CI-enforced thresholds**.
Collected with `cargo bench --bench pipeline_baseline` over the deterministic
synthetic corpus generator in `tests/support/corpus.rs` (seeded, fictional
content only; nothing reads a real transcript).

## Machine context

| | |
|---|---|
| CPU | Apple M3 Pro (11 cores, arm64) |
| RAM | 18 GiB |
| OS | macOS 26.2 |
| Toolchain | rustc 1.97.0, bench profile (optimized) |
| Provenance | Issue #245 rerun on the original identity-free corpus; the 500 MiB identity profile is opt-in. |

## Stage coverage

In-crate stages measured: source reading, framing, parsing/normalization,
metrics accumulation, evidence accumulation, report reduction. Provider-DB-
backed sources are also in-crate: a raw `RawSource::Sqlite` resolves to the
generic schema-agnostic SQLite table walk (`vendors/sqlite.rs`, the OpenCode
fallback), covered functionally in `tests/pipeline_corpus.rs` and timed
below over a synthetic seeded database. The remaining stages the master plan
names (discovery, queue wait, persistence, report query, IPC) live in the
desktop app (`apps/desktop/src-tauri`) and need a desktop-side harness —
those stages (not the provider-DB read path) are the desktop-side half of
the measurement follow-up.

## Timing baseline

### Framing throughput (`BoundedJsonlReader`)

| Scenario | Time (median) | Throughput |
|---|---|---|
| 6 MiB of small lines (~22k records) | 2.75 ms | ~2.30 GiB/s |
| One near-8 MiB line (just under the bound) | 3.13 ms | ~2.48 GiB/s |
| One oversized line (> 8 MiB, skipped) | 2.99 ms | ~2.64 GiB/s |

### Full reparse cost vs session size (the append-only question)

Claimed path with `AppendOnlyGuarantee::Absent`: pin → stream through the
composite metrics+evidence sink → full recheck. This is what the worker pays
for every source change today.

| File size | Time (median) | Throughput |
|---|---|---|
| 1 MiB | 8.57 ms | ~126 MiB/s |
| 10 MiB | 83.6 ms | ~129 MiB/s |
| 50 MiB | 424 ms | ~127 MiB/s |

The current corpus keeps its original identity-free shape. The 10 MiB result is
1.15× the 72.5 ms pre-change baseline. The UUID and chained-parent evidence
profile is opt-in and does not alter these rows. The tiers remain linear.

### Per-stage split at 10 MiB (in-memory source)

| Stage composition | Time (median) | Share |
|---|---|---|
| Framing only | 4.59 ms | ~7 % |
| + parse/normalize (no-op sink) | 65.7 ms | ~82 % |
| + metrics accumulation | 79.7 ms | +14.0 ms |
| + metrics and evidence together | 82.8 ms | +3.1 ms |
| Metrics accumulator only, pre-normalized events | 14.3 ms | ~755 MiB/s |
| Fully disordered metrics accumulator | 16.9 ms | 1.18× ordered |

The bounded reducer adds slot, cache, efficiency, active-time, and local reorder
folds in the hot loop. The full metrics stage is 1.20× its 66.4 ms pre-change
baseline. The isolated row excludes event cloning and uses ten Criterion
samples. No pre-change isolated row exists, so the required isolated
before-and-after gate was not evaluated. The full-stage and disorder ratios are
both below 1.30×, but they do not replace that missing gate.

### Issue #229 remeasurement

The structural unknown-record scan was remeasured before the issue #229 commit.
The 10 MiB full reparse median was 72.8 ms, compared with 72.5 ms above.
Criterion detected no change for the 10 MiB metrics-and-evidence stage against
the immediately preceding run. The eventless-record walk caused no measurable
pipeline regression.

### Fork-job `Inline` materialization proxy at 10 MiB

| Path | Time (median) |
|---|---|
| Stream from file | 82.6 ms |
| `read_to_string` then inline visit | 83.4 ms |

Materializing the whole transcript first costs ~1.0 % extra time in this run.
It also adds one transient full-source allocation (10 MiB here).

### Provider-DB-backed source (generic SQLite walk)

Raw `RawSource::Sqlite` through the composite metrics+evidence sink. This
path is batch, not streaming: the walk materializes every extracted event
before the bounded metrics accumulator sees them. The batch event vector,
not the metrics accumulator, retains memory proportional to the session.

| Rows (records) | DB size | Time (median) | Throughput |
|---|---|---|---|
| 2,000 | ~0.6 MiB | 3.63 ms | ~175 MiB/s |
| 20,000 | ~6.2 MiB | 37.1 ms | ~170 MiB/s |

Linear, and faster per byte than the JSONL path (SQLite hands the
walk whole text cells; no newline scanning).

### Report reduction vs cohort size (`EfficiencyReportAccumulator`)

| Sessions | Time (median) | Per session |
|---|---|---|
| 10 | 8.73 µs | ~0.87 µs |
| 65 (field cohort, issue #222) | 48.6 µs | ~0.75 µs |
| 100 | 75.1 µs | ~0.75 µs |
| 500 | 361 µs | ~0.72 µs |

Linear at ~0.72–0.75 µs/session — reduction over a 30-day window is
microseconds, not milliseconds.

## Memory figures

| Figure | Value | Bound / note |
|---|---|---|
| Framing high-water, 10 MiB of small lines | 446 bytes | ≤ `SCAN_QUANTUM_BYTES × 4` |
| Framing high-water, one near-8 MiB line | 8,323,291 bytes | ≤ `MAX_RECORD_BYTES` (8,388,608) — bound respected |
| Metrics accumulator, 10 MiB source | 34,361 observed turns, 326,591 bytes of derived state retained | Bounded below the 640 KiB derived-state contract; exact identity strings are additional |
| Serialized evidence per session (report query row proxy) | ~4.5 KB | a 500-session reduction reads ~2.3 MB of evidence rows; the accumulator itself holds capped folds and examples, except for the tracked thread-identity set |

### Peak heap: streaming vs whole-file materialization

Measured with a counting global allocator in `benches/memory_baseline.rs`
(`cargo bench --bench memory_baseline`). Peak growth over the live baseline
for the full pipeline (parse → metrics → evidence → serialize) on the same
on-disk session, streamed vs `read_to_string` first:

| Source | Streaming peak | Inline peak | Inline / streaming |
|---|---|---|---|
| 1 MiB | 0.82 MB (0.74× source) | 1.93 MB (1.73× source) | 2.35× |
| 10 MiB | 3.81 MB (0.34× source) | 15.1 MB (1.34× source) | 3.96× |
| 50 MiB | 27.8 MB (0.49× source) | 84.5 MB (1.49× source) | 3.04× |

What this says: the streaming reader and metrics accumulator are bounded.
Streaming peak stays below 0.50× source at the 10 MiB and 50 MiB tiers.
Materializing first adds one full source copy. The residual proportional state
is in the Claude adapter's message-id de-duplication map, not metrics framing.

### 500 MiB tier: which accumulator owns the peak

The same binary runs a 500 MiB tier (generated on demand; the repository
stores no large blob) and measures the metrics accumulator alone against the
full composite:

| Measurement | Value |
|---|---|
| Metrics-only peak | 220.6 MB (0.40× source), down from 797.0 MB |
| Composite (metrics + evidence) peak | 317.8 MB (0.58× source) — evidence adds 97.2 MB |
| Metrics derived state retained at end of stream | 241,599 bytes after 1,269,150 turns, down from 525.6 MB |
| Allocator-observed metrics live bytes | 241,988 bytes (0.16% above the estimate, including identity strings) |
| Peak residual outside metrics | 220.3 MB |
| Repeated-64-ID metrics peak | 9.99 MB |
| Unique-ID peak delta | 210.6 MB |
| Serialized evidence | 4,461 bytes |

The metrics state is below 640 KiB and agrees with allocator observation within
one percent. Reusing 64 message ids removes 210.6 MB from the 220.3 MB metrics
residual, which attributes most metrics-side peak growth to
`ClaudeStreamState::max_usage_by_message_id`. The UUID-bearing composite adds
97.2 MB, exercising the tracked unbounded
`SessionEvidenceAccumulator::seen_thread_uuids` state. This change does not
claim a bounded full-pipeline peak.

### Saturated metrics state

The synthetic saturation profile fills the slot grid, efficiency contributions,
skill and late-tool lists, tool, model, thinking-mode, speed, and last-tool
interners, model runs, and name maps.

| Records | Estimated retained | Allocator-observed live |
|---:|---:|---:|
| 40,000 | 366,413 bytes | 375,455 bytes |
| 400,000 | 395,789 bytes | 404,831 bytes |

Sparse occupied-cell capacity varies by 29,376 bytes across the tenfold turn-
count increase. Both measurements stay below the 640 KiB contract.

### Held session tree

One 10 MiB parent plus twenty 50-turn children retained 867,833 bytes of
derived state at merge time. Allocator-observed live growth was 936,678 bytes;
peak growth, including the temporary merged output, was 3,808,726 bytes.

## Active-writer `SourceChanged` rates

2 MiB source (~13 ms read window), full-reprocess claim, `Absent` guarantee,
15 attempts per row; a synthetic writer appends one ~300-byte record per
interval:

| Append interval | Rejected (SourceChanged) | Accepted |
|---|---|---|
| 2 ms | 15/15 | 0 |
| 20 ms | 0/15 | 15 |
| 200 ms | 0/15 | 15 |
| quiescent control | 0/15 | 15 |

A rejection needs a write to land inside the read window. Once the append
interval exceeds the read duration, the observed rate drops to zero; the
steady-state rejection probability is approximately
`read_window / write_interval`. Real agent sessions write on the order of
seconds apart, and the read window for typical session sizes is milliseconds.

## What the numbers say — the four folded questions

1. **Phase-13 optimizations (report caching, relational evidence
   projections, read pooling)** — *dismiss with the number.* Report
   reduction costs ~0.72–0.75 µs/session (361 µs for a 500-session cohort,
   48.6 µs for the 65-session field cohort). No stage shows a bottleneck that
   caching or projections would relieve; the only material cost is serde
   parsing at ~127 MiB/s, and even a 50 MiB session completes in 424 ms.

2. **Claude append-only guarantee evidence** — *defer with evidence; likely
   never justified.* A full reprocess is 8.57 ms at 1 MiB, 83.6 ms at 10 MiB,
   and 424 ms at 50 MiB — linear and cheap. The rejection rate is bounded by
   read-window/write-interval and reached zero in every measured scenario
   with realistic write spacing. The followups entry's own words apply:
   "the work may never be justified." Incremental (byte-offset) parsing
   would save at most a fraction of 424 ms per pathological session.

3. **Fork-job `Inline` materialization** — *defer.* Materializing costs
   ~0.9 % extra time in this run and one transient full-source buffer per fork
   job. With the worker's single source permit, peak transient memory equals
   the largest single session. The metrics accumulator stays bounded, so the
   inline source copy is the material memory difference. Revisit if fork jobs
   become common or field sessions grow well beyond 50 MiB.

4. **Popover-hidden staleness (Locked Decision 15 review)** — *reaffirm; no
   change proposed.* Catch-up drain time once the popover opens is
   N × per-session full-pipeline cost: the 65-session field cohort at
   typical sizes (≤ a few hundred KB) drains in well under one second on
   one permit; even a pathological 65 × 10 MiB backlog drains in ~5 s.
   Staleness while hidden is therefore bounded by the pause itself, and
   recovery is fast enough that no scheduling change is warranted.

**Worker concurrency and leases (Part 3 tuning clause):** keep 1 CPU /
1 source / 1 provider-DB permit and `LEASE_SECS = 300`,
`LEASE_RENEW_SECS = 60` — now a measured outcome. The worst measured
per-session cost (424 ms at 50 MiB) is two orders of magnitude below the
lease renewal interval, and single-permit throughput (~133 MiB/s) clears
realistic backlogs in seconds, so extra permits would only add contention
with the reader's live agent sessions.
