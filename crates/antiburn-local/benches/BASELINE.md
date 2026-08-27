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
| Date | 2026-02 baseline run |

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
| 6 MiB of small lines (~22k records) | 3.05 ms | ~2.07 GiB/s |
| One near-8 MiB line (just under the bound) | 4.04 ms | ~1.92 GiB/s |
| One oversized line (> 8 MiB, skipped) | 3.42 ms | ~2.30 GiB/s |

### Full reparse cost vs session size (the append-only question)

Claimed path with `AppendOnlyGuarantee::Absent`: pin → stream through the
composite metrics+evidence sink → full recheck. This is what the worker pays
for every source change today.

| File size | Time (median) | Throughput |
|---|---|---|
| 1 MiB | 7.38 ms | ~146 MiB/s |
| 10 MiB | 72.5 ms | ~149 MiB/s |
| 50 MiB | 351 ms | ~154 MiB/s |

The cost curve is linear at ~150 MiB/s end to end.

### Per-stage split at 10 MiB (in-memory source)

| Stage composition | Time (median) | Share |
|---|---|---|
| Framing only | 4.64 ms | ~7 % |
| + parse/normalize (no-op sink) | 67.0 ms | ~98 % |
| + metrics accumulation | 66.4 ms | within noise of no-op |
| + metrics and evidence together | 68.4 ms | +~2 ms |

The pipeline is dominated by JSON parsing (serde deserialization inside the
adapter); both accumulators together add only ~2–3 % on top.

### Fork-job `Inline` materialization proxy at 10 MiB

| Path | Time (median) |
|---|---|
| Stream from file | 68.8 ms |
| `read_to_string` then inline visit | 73.2 ms |

Materializing the whole transcript first costs ~6 % extra time plus one
transient full-source allocation (10 MiB here).

### Provider-DB-backed source (generic SQLite walk)

Raw `RawSource::Sqlite` through the composite metrics+evidence sink. This
path is batch, not streaming: the walk materializes every extracted event
before the sink sees them, so retained memory here is proportional to the
session, like the metrics accumulator itself.

| Rows (records) | DB size | Time (median) | Throughput |
|---|---|---|---|
| 2,000 | ~0.6 MiB | 2.98 ms | ~214 MiB/s |
| 20,000 | ~6.2 MiB | 30.7 ms | ~205 MiB/s |

Linear, and slightly faster per byte than the JSONL path (SQLite hands the
walk whole text cells; no newline scanning).

### Report reduction vs cohort size (`EfficiencyReportAccumulator`)

| Sessions | Time (median) | Per session |
|---|---|---|
| 10 | 8.98 µs | ~0.9 µs |
| 65 (field cohort, issue #222) | 49.2 µs | ~0.76 µs |
| 100 | 75.4 µs | ~0.75 µs |
| 500 | 367 µs | ~0.73 µs |

Linear at ~0.75 µs/session — reduction over a 30-day window is microseconds,
not milliseconds.

## Memory figures

| Figure | Value | Bound / note |
|---|---|---|
| Framing high-water, 10 MiB of small lines | 446 bytes | ≤ `SCAN_QUANTUM_BYTES × 4` |
| Framing high-water, one near-8 MiB line | 8,323,291 bytes | ≤ `MAX_RECORD_BYTES` (8,388,608) — bound respected |
| Metrics accumulator, 10 MiB source | 34,361 turns, 10.4 MB retained (~303 bytes/turn) | CH-005's "proportional growth" quantified: ≈ 1.0× source bytes for a dense transcript |
| Serialized evidence per session (report query row proxy) | ~4.5 KB | a 500-session reduction reads ~2.2 MB of evidence rows; the accumulator itself holds only capped folds and examples |

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
   reduction costs ~0.75 µs/session (367 µs for a 500-session cohort, 49 µs
   for the 65-session field cohort). No stage shows a bottleneck that
   caching or projections would relieve; the only material cost is serde
   parsing at ~150 MiB/s, and even a 50 MiB session completes in 0.35 s.

2. **Claude append-only guarantee evidence** — *defer with evidence; likely
   never justified.* A full reprocess is 7 ms at 1 MiB, 73 ms at 10 MiB,
   351 ms at 50 MiB — linear and cheap. The rejection rate is bounded by
   read-window/write-interval and reached zero in every measured scenario
   with realistic write spacing. The followups entry's own words apply:
   "the work may never be justified." Incremental (byte-offset) parsing
   would save at most a fraction of ~0.35 s per pathological session.

3. **Fork-job `Inline` materialization** — *defer.* Materializing costs
   ~6 % extra time and one transient full-source buffer per fork job. With
   the worker's single source permit, peak transient memory equals the
   largest single session. The metrics accumulator already retains ≈ source
   size for dense transcripts, so inlining changes the constant, not the
   order of magnitude. Revisit only if field sessions well beyond 50 MiB
   appear in the affected-source volume.

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
per-session cost (0.35 s at 50 MiB) is two orders of magnitude below the
lease renewal interval, and single-permit throughput (~150 MiB/s) clears
realistic backlogs in seconds, so extra permits would only add contention
with the reader's live agent sessions.
