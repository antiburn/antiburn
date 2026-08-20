# Local Insights: architecture and sequential implementation guide

**Status:** Reviewed implementation proposal

**Goal:** Build a useful on-device Hygiene and Efficiency report from local coding-agent sessions while keeping raw transcript reading bounded, results truthful, processing restart-safe, and work minimally disruptive to the user's active work.

## Executive decision

Local Insights should be built as two streaming pipelines separated by compact, versioned per-session evidence in Antiburn's SQLite database.

```text
Provider transcript processing

session discovery
  → source versioning and durable work state
  → bounded source-record stream
  → provider-specific normalization
  → multiple analysis accumulators
      ├── existing SessionMetrics
      └── new SessionEvidence
  → atomic session_analysis + session_evidence update

Cross-session report processing

read-only SQLite snapshot
  → stream session + ready evidence rows
  → composite report accumulators and detector checks
  → in-memory Insights/Hygiene report
  → desktop IPC and UI
```

The design intentionally does **not** persist raw transcripts or complete canonical sessions. A normalized record exists only long enough to update the relevant accumulators and is then discarded.

The bounded-memory guarantee applies to raw source framing, normalized-record lifetime, `SessionEvidence`, and report reduction. It does not initially apply to every retained collection needed for exact existing `SessionMetrics` output. The current analysis already grows in memory with transcript size; the first streaming implementation removes the worse whole-file `String` and complete canonical-event materialization while accepting that compact metrics state may still grow with metric-bearing events.

The implementation should proceed provider by provider:

1. add streaming transcript processing to the provider's existing session-metrics generation;
2. attach session-evidence processing to that same stream;
3. complete and persist truthful evidence for the provider;
4. make the provider's evidence available to the shared streaming report reducer;
5. repeat for each additional provider.

The first provider should be one JSONL provider, preferably Claude Code. This proves the shared architecture before adding more provider formats or generality.

## Product scope

### Canonical Hygiene and Efficiency categories

The local report should preserve the complete set of finding categories represented by Cadence `main` under My Work → Insights → Hygiene and Efficiency.

| Category | What it finds | Current remediation concept |
|---|---|---|
| **Sessions Over Depth** | Individual requests whose context exceeds the evidence-driven autocompact cap. This is request depth, not merely a long-running session. | Lower the autocompact window, compact at task boundaries, or move open-ended exploration into a subagent. |
| **Model Overthinking** | Claude Code or Codex sessions using reasoning/thinking tiers above the reviewed recommended cap. | Lower the tier with the provider's effort/model controls. Prompt keywords are not inferred as explicit reasoning-tier evidence. |
| **Overpowered Subagents** | Premium main-loop models silently spawning subagents on the same premium tier. | Configure a cheaper default or per-agent subagent model while retaining premium subagents when deliberately justified. |
| **Unused MCP Servers** | MCP definitions loaded into eligible sessions but never directly invoked. | Remove the server, deny the connector, or scope it to projects that use it. |
| **Unused Built-In Tools** | Native harness tools whose definitions consume context but are not used. | Disable only where the tool, lost capability, and safe disable mechanism are known. Otherwise keep the result audit-only. |
| **Unused Skills** | Skills repeatedly loaded but never invoked, grouped by installed, project, plugin, or bundled origin. | Remove, narrow, disable, or uninstall the relevant skill/plugin where a safe mechanism exists. |
| **Old Model Usage** | Use of a curated deprecated model after its reviewed replacement became available. | Select the replacement model or update the provider's default model. |
| **Overuse of Fast Mode** | Fast-tier usage in delegated work or left enabled as a standing default. | Use fast mode deliberately for latency-sensitive interactive work and disable it for delegation-heavy or non-urgent work. |
| **Cache Churn** | Tokens paid or resent because of idle expiry, compaction, model switching, or provider cache eviction. | Finish or hand off before expiry, compact at appropriate boundaries, start fresh when context is unnecessary, and avoid needless model changes. |

Each category always has one detector status:

- findings;
- clean;
- not assessed with a structured reason.

An empty findings list does not prove a detector ran cleanly.

### Additional local category: subscription/quota limit pressure

Antiburn should also support an explicitly local extension for subscription or quota-limit pressure. This remains separate from the nine-category Cadence compatibility contract because provider limits may represent:

- rolling five-hour usage;
- weekly usage;
- model-specific allocation;
- weighted usage rather than raw tokens;
- rate-limit errors without an exposed numeric quota.

The report may combine:

- transcript-observed limit errors attributable to a session;
- Antiburn's existing account/provider usage snapshots;
- reset times and utilization where exposed.

The account-level evidence is a separate optional report input rather than being forced into `SessionEvidence` when it cannot be attributed to one session.

## Non-negotiable correctness semantics

### No false absence

Detector-critical evidence must distinguish:

```text
Complete(value)       the capability was supported and completely observed
Partial(value, reason) some evidence was observed, but absence cannot be concluded
Unsupported           this provider/source/schema cannot expose the evidence
```

A complete zero or empty collection means an event did not happen. `Unsupported` and `Partial` never mean zero.

During development of the first provider only, a temporary debug-only state may be used:

```rust
pub enum EvidenceValue<T> {
    #[cfg(debug_assertions)]
    Unimplemented,
    Unsupported,
    Partial {
        observed: T,
        reason: CoverageReason,
    },
    Complete(T),
}
```

`Unimplemented` is implementation scaffolding. It must be removed entirely before the first provider implementation is complete. Any remaining use should make a release build fail while the variant is debug-only; after removal, remaining debug references should also fail.

### Capabilities and coverage are separate

The system records two distinct concepts:

1. **Source/parser capabilities:** what a provider, source format, and observed schema/version can reliably expose.
2. **Per-session evidence coverage:** whether this particular source was parsed completely enough to conclude presence or absence.

Discovery carries cheap provider/source/version hints. The parser returns the definitive capabilities, provenance, coverage, and diagnostics after inspecting source records.

### Findings are policy; evidence is fact

Persist facts such as token quantities, tool counts, model usage, context depths, and observed transitions. Do not persist conclusions such as `overthinking = true` or final savings.

This allows thresholds, detector rules, remediation text, and pricing to change without reparsing transcripts.

## Existing Antiburn capabilities to reuse

### Discovery

`crates/antiburn-local/src/discovery/mod.rs` already provides:

- `Explorers::discover_recent_sessions`;
- `Explorers::discover_recent_sessions_with_progress`;
- `SessionLog`;
- `SessionSource::{File, Inline, ProviderDb}`;
- parallel provider fan-out;
- native/WSL discovery and deduplication;
- bounded metadata previews;
- `AgentExplorer::provider_db_fingerprint`;
- point source location APIs.

Discovery currently materializes vectors of descriptors. That is acceptable initially because descriptors are much smaller than transcript contents.

### Desktop session index

`apps/desktop/src-tauri/src/scan.rs::pass` already:

- discovers sessions;
- reads bounded metadata;
- gates unsupported/subagent rows;
- calls `Store::upsert_sessions`;
- records scan state;
- calls `top_up_analysis` for recent session metrics.

`apps/desktop/src-tauri/src/store/schema.rs` already contains:

- `session`;
- `session_analysis`;
- `session_relation`;
- `scan_state`.

### Parsing and analysis

`crates/antiburn-local/src/analysis/` already provides:

- `RawSource` and `SessionInput`;
- `VendorAdapter` and provider adapters;
- `NormalizedSession` and `NormalizedEvent`;
- token/model/tool normalization;
- `SessionMetrics`;
- context, timing, phase, compaction, skill, and cost calculations;
- initial-context skill/MCP/instruction attribution.

These semantics should be reused rather than importing Cadence's dependency graph or building a separate parsing engine.

## Current limitations to replace

- File-backed analysis calls `std::fs::read_to_string` without a total session limit.
- `NormalizedSession.events` materializes a complete `Vec<NormalizedEvent>`.
- `analyze_sources_with` materializes all normalized sessions before aggregation.
- Initial-context attribution reparses the raw transcript.
- Desktop file fingerprints are currently second-resolution `mtime:size`.
- Provider DB and inline sources do not currently receive equivalent desktop cache fingerprints.
- `session_analysis` has no source generation, parser revision, analyzer revision, or metrics schema revision.
- `top_up_analysis` processes sessions sequentially inside the scan pass.
- The current `Store` has one persistent SQLite connection behind a mutex, serializing all app-database access.
- There is no existing 512 MiB general analysis cap. Smaller provider-specific limits protect metadata, subprocess output, or discovery paths and should not be conflated with transcript analysis.

## Ownership boundaries

### `antiburn-local` owns

- provider discovery;
- source identity and provider-aware source version calculation;
- stable bounded source opening;
- provider-specific record parsing;
- normalized evidence-bearing records;
- shared record-level classification;
- `SessionMetrics` accumulation;
- `SessionEvidence` accumulation;
- source capabilities, provenance, coverage, and diagnostics;
- pure detector logic;
- pure cross-session report accumulation.

### Desktop application owns

- Antiburn's application SQLite schema and migrations;
- durable pending/processing/ready state;
- work claiming, leases, retries, and wake-up events;
- CPU, source, provider-DB, and memory admission limits;
- report-population queries and read-only database connection;
- Tauri commands/events;
- settings-pane UI;
- clear/delete integration.

`antiburn-local` must remain storage-neutral and must not depend on Tauri or Antiburn's application database schema.

## Target source and processing contracts

### Source descriptor and version

Candidate storage-neutral types in `antiburn-local`:

```rust
pub struct SourceDescriptor {
    pub agent: AgentKind,
    pub session_id: String,
    pub environment: DiscoveryEnvironment,
    pub source: SessionSource,
    pub updated_at_epoch: Option<i64>,
}

pub struct SourceVersion {
    pub fingerprint: String,
    pub estimated_bytes: Option<u64>,
    pub streamability: Streamability,
}

pub enum Streamability {
    RecordStream,
    DatabaseRows,
    WholeDocumentFallback,
    InlineMaterialized,
}
```

Fingerprint policy:

- file: stable identity where available, byte size, and high-resolution modification/change time;
- provider DB: reuse and strengthen provider-specific fingerprints;
- inline: hash the already-materialized content or mark always-refresh where hashing is unavailable;
- format is internal and versioned.

### Bounded normalized-record visitor

The existing compatibility API remains available:

```rust
normalize_source(&SessionInput) -> Result<NormalizedSession>
```

The normal metrics/evidence path should use a lower-level visitor:

```rust
pub trait NormalizedRecordSink {
    fn observe(&mut self, record: NormalizedRecord) -> ControlFlow<()>;
}

pub trait VendorAdapter: Sync {
    fn visit_source(
        &self,
        input: &SessionInput,
        sink: &mut dyn NormalizedRecordSink,
        diagnostics: &mut ParseDiagnostics,
        control: &ProcessingControl,
    ) -> anyhow::Result<SessionProvenance>;
}
```

`normalize_source` can later be implemented as a collector over this visitor for tests, tools, and views that truly need a complete normalized session.

### Normalized record model

A provider-neutral record enum may be more honest than forcing every detector fact into message-shaped `NormalizedEvent`:

```rust
pub enum NormalizedRecord {
    Event(NormalizedEvent),
    ModelTurn(NormalizedModelTurn),
    ContextSource(ContextSourceObservation),
    ToolDefinition(ToolDefinitionObservation),
    Subagent(SubagentObservation),
    Compaction(CompactionObservation),
    UsageLimit(UsageLimitObservation),
}
```

Only add variants or fields required by an implemented metric, detector, or established cross-session view.

### Composite per-session processing

One parsed record feeds multiple bounded accumulators:

```text
bounded source record
  → provider normalization
  → normalized record
      ├── SessionMetricsAccumulator
      └── SessionEvidenceAccumulator
```

At end of stream:

```rust
pub struct ProcessedSession {
    pub metrics: SessionMetrics,
    pub evidence: SessionEvidence,
    pub provenance: SessionProvenance,
    pub diagnostics: ParseDiagnostics,
    pub processed_source_version: SourceVersion,
}
```

The source line, parsed JSON value, and normalized record are discarded before the next source record is retained.

### Rich metrics and retained state

Some existing `SessionMetrics` fields require finalization after the complete event sequence:

- active-time normalization;
- timeline segments;
- phase buckets;
- timestamp ordering;
- per-invocation skill data.

Exact parity with the current metrics contract can require retained state proportional to the number of metric-bearing events. Some output collections, including phase segments and skill uses, can themselves grow with session size. The first implementation accepts this existing unbounded metrics behavior while making it substantially less expensive: retain compact timestamp/phase/skill facts only where exact finalization needs them, and do not retain raw text, whole JSON values, tool-output payloads, the complete transcript `String`, or complete normalized events.

A later measured improvement may move growing metric details into generation-scoped child/staging tables or another disk-backed spool and retain only bounded summaries in memory. That optimization may require changing the rich metrics output contract or finalization path and is not required for the first streaming provider.

## JSONL source policy

### Full forward processing

Antiburn should process the complete JSONL stream by default, even when the file exceeds 512 MiB. Streaming removes the memory reason for a total-file cap, though CPU/I/O budgets and cancellation still matter.

### Bounded newline framing

Plain `BufRead::read_until` is insufficient by itself because its destination buffer can grow without bound. Implement a bounded newline-framed reader using `fill_buf` or equivalent chunk scanning:

1. scan buffered bytes for `\n`;
2. retain chunks only until the configured record limit;
3. if the record exceeds the limit, stop retaining and drain through the next newline;
4. record an oversized-record diagnostic;
5. continue at the next record;
6. mark affected capabilities/session coverage partial.

The provider controls row size; Antiburn controls how much it retains and parses.

The maximum individual record size should be selected from real provider fixtures and benchmarks. It is a safety valve, not an assertion that providers cannot emit larger rows.

### Malformed and trailing records

- Parse each complete line independently.
- A malformed line does not discard valid surrounding records.
- An incomplete final line is not treated as committed evidence.
- Skipped records update diagnostics and evidence coverage.
- Cancellation is checked between records or bounded byte intervals.

### Source mutation and actively growing transcripts

- Capture source version before reading.
- Recheck source identity/version after processing.
- If the source changed, return `SourceChanged` and do not publish stale projections.
- The next generation remains pending.

A transcript that is still receiving appends must not enter an immediate discard/retry loop or monopolize worker capacity. The first implementation should:

- reuse `ACTIVE_SESSION_WINDOW_SECS` as the quiet-period debounce and avoid claiming file sources still considered active;
- set `next_attempt_at` instead of immediately retrying `SourceChanged` work;
- apply bounded backoff when a source repeatedly changes during processing;
- claim eligible rows fairly so one hot session cannot prevent stable sessions from progressing;
- retain the last completed `analyzed_generation` and evidence payload while a newer generation is pending;
- exclude stale/pending evidence from a clean current report and count the active session as not assessed.

Marking a new generation pending must not erase its last completed evidence. A session that remains active indefinitely may remain pending in the first version, but it must not starve the rest of the queue.

A later optimization may process a captured append-only prefix: record file identity and starting length, read only complete records in that prefix, publish that older completed generation, and leave appended work pending. Do not add this until provider append-only behavior and measured need justify it.

### Selective deserialization

Provider-specific deserializers should avoid retaining huge prompt/tool-output fields when only metadata, usage, tool names, or mode fields are needed. Use ignored-field/visitor techniques where justified by fixtures. Temporary-file spooling remains a fallback only if a legitimate evidence-bearing record exceeds the chosen memory bound.

## Provider SQLite source policy

For supported provider databases:

1. open a dedicated read-only connection;
2. begin a read transaction for one consistent source snapshot;
3. query only the target session cluster and required columns;
4. order rows deterministically;
5. normalize one row at a time into the same record sink;
6. discard each provider row before stepping to the next;
7. close the statement, transaction, and connection immediately after the session fold.

Do not render the complete provider session into a synthetic JSONL `String`.

Provider database connections are distinct from the Antiburn application database connection. Provider journal mode is not controlled by Antiburn, so concurrent readers should be conservatively limited, initially to one unless provider-specific tests justify more. Do not use SQLite immutable mode for a database the provider may still be writing.

## Source capabilities, coverage, and provenance

The parser returns a `SessionProvenance` containing:

- provider;
- source kind and format;
- observed harness/schema version;
- parser revision;
- supported capabilities;
- ordering guarantee;
- malformed/skipped/oversized record counts;
- unknown schema variants;
- source mutation/truncation observations.

Capabilities should cover at least:

- model identity;
- input/output/cache token classes;
- request context depth;
- timestamps and canonical ordering;
- reasoning/effort tier;
- fast/service tier;
- tool definitions and invocations;
- skill/MCP source attribution;
- compaction boundaries;
- subagent relationships/models;
- transcript-observed quota incidents.

Pricing is not a source capability. The source provides model identity and billable token quantities; a versioned report-time catalog determines whether those quantities are priceable.

## `SessionEvidence` contract

Candidate shape:

```rust
pub struct SessionEvidence {
    pub schema_revision: u32,
    pub identity: SessionEvidenceIdentity,
    pub time_range: EvidenceValue<SessionTimeRange>,
    pub eligibility: EvidenceValue<EligibilityEvidence>,
    pub context: EvidenceValue<ContextEvidence>,
    pub models: EvidenceValue<ModelEvidence>,
    pub tools: EvidenceValue<ToolEvidence>,
    pub context_sources: EvidenceValue<ContextSourceEvidence>,
    pub subagents: EvidenceValue<SubagentEvidence>,
    pub cache: EvidenceValue<CacheEvidence>,
    pub compactions: EvidenceValue<CompactionEvidence>,
    pub quota_incidents: EvidenceValue<SessionQuotaEvidence>,
    pub capabilities: SourceCapabilities,
    pub coverage: EvidenceCoverage,
    pub diagnostics: ParseDiagnostics,
}
```

The exact grouping may evolve while implementing the first provider. The durable requirements are:

- facts are rule-neutral;
- evidence state is not ambiguous;
- maps and examples are bounded;
- strings and diagnostics are capped;
- provider/source provenance is retained;
- schema revision is explicit.

Persist model-attributed token quantities, not only current prices. Pricing changes should not require transcript reparsing.

## Database changes

### Shared source version on `session`

Add source truth and a queryable analyzed start time to the existing `session` row:

```text
source_fingerprint TEXT
source_generation  INTEGER NOT NULL DEFAULT 0
started_at_epoch   INTEGER
```

When discovery first obtains a reusable fingerprint, generation becomes 1. When the fingerprint changes, generation increments. Re-observing the same fingerprint is idempotent.

Antiburn does not currently store a queryable session start on `session`; the earliest parsed timestamp exists only as `SessionMetrics.first_ts_ms` inside `metrics_json`. Streaming analysis should populate `started_at_epoch` from trustworthy provider start metadata or the earliest normalized timestamp. `first_seen_at` is discovery time and must not be used as a session-start fallback.

### Version existing `session_analysis`

Add:

```text
analyzed_generation      INTEGER NOT NULL DEFAULT 0
parser_revision          INTEGER NOT NULL DEFAULT 1
analyzer_revision        INTEGER NOT NULL DEFAULT 1
metrics_schema_revision  INTEGER NOT NULL DEFAULT 1
```

Existing `source_fingerprint` remains useful diagnostic evidence. Freshness requires generation and revisions to match current values.

### New `session_evidence` table

The table stores stable relational lifecycle/version columns plus a versioned JSON evidence payload. It does not create one SQL column for every map-valued fact.

```sql
CREATE TABLE session_evidence (
    environment_key          TEXT NOT NULL,
    agent                   TEXT NOT NULL,
    session_id              TEXT NOT NULL,

    status                  TEXT NOT NULL,
    analyzed_generation     INTEGER,
    processed_fingerprint   TEXT,

    parser_revision         INTEGER,
    analyzer_revision       INTEGER,
    evidence_schema_revision INTEGER,

    evidence_json           TEXT,
    diagnostics_json        TEXT,

    retry_count             INTEGER NOT NULL DEFAULT 0,
    claim_fence             INTEGER NOT NULL DEFAULT 0,
    claimed_at              TEXT,
    lease_expires_at        TEXT,
    next_attempt_at         TEXT,
    analyzed_at             TEXT,
    last_error              TEXT,

    PRIMARY KEY (environment_key, agent, session_id),
    FOREIGN KEY (environment_key, agent, session_id)
      REFERENCES session (environment_key, agent, session_id)
      ON DELETE CASCADE,

    CHECK (status IN (
      'pending', 'processing', 'ready', 'unsupported', 'failed'
    ))
) STRICT;

CREATE INDEX session_evidence_status
    ON session_evidence (status, next_attempt_at, lease_expires_at);
```

Work lifecycle and evidence quality remain separate:

- lifecycle: pending, processing, ready, unsupported, failed;
- evidence quality: complete or detector-specific partial/unsupported fields inside `evidence_json`.

### Dirty marking

When discovery observes a new source generation:

```text
session.source_generation += 1
session.source_fingerprint = observed fingerprint
session_evidence.status = pending
session_evidence.last_error = null
session_evidence.evidence_json remains the last completed generation until replacement
```

The session upsert and evidence transition happen in one short transaction. Worker notification occurs only after commit.

### Revision-driven requeue

A source can require reprocessing even when its fingerprint and generation did not change. On startup and before normal worker claiming, reconcile enabled-provider rows against current projection revisions. Mark or treat a row as pending when any transcript-derived projection is stale:

```text
session_analysis.analyzed_generation != session.source_generation
OR session_analysis.parser_revision != CURRENT_PARSER_REVISION
OR session_analysis.analyzer_revision != CURRENT_ANALYZER_REVISION
OR session_analysis.metrics_schema_revision != CURRENT_METRICS_SCHEMA_REVISION
OR session_evidence.analyzed_generation != session.source_generation
OR session_evidence.parser_revision != CURRENT_PARSER_REVISION
OR session_evidence.analyzer_revision != CURRENT_ANALYZER_REVISION
OR session_evidence.evidence_schema_revision != CURRENT_EVIDENCE_SCHEMA_REVISION
```

Do not increment `session.source_generation` for a code/schema revision; the source did not change. Requeue stale projections lazily through the normal bounded worker so an application update cannot create an uncontrolled processing spike.

Invalidation policy:

| Change | Reprocess transcript |
|---|---:|
| Parser revision | Yes |
| Analyzer revision | Yes |
| Metrics schema revision | Yes |
| Evidence schema revision | Yes unless an explicit data-only migration can upgrade it safely |
| Detector thresholds/rules | No |
| Remediation/catalog wording | No |
| Pricing catalog | No for evidence/report facts |
| Deprecated-model replacement catalog | No |

### Claim fencing and atomic completion

Source generation is not a worker-claim token: two workers may process the same generation after a lease expires. Every initial claim or reclaim must atomically increment `claim_fence` and return the new value. The worker carries both its claimed source generation and fence token.

Lease renewal, success, failure, and retry transitions must match:

```text
status = processing
source generation = claimed generation
claim_fence = claimed fence
```

A reclaimed worker receives a newer fence, making every late transition from the older worker a no-op.

At completion, write `session.started_at_epoch`, `session_analysis`, and `session_evidence` in one short transaction only if the claimed generation still matches `session.source_generation` and the current processing row still carries the worker's claim fence. Conceptually:

```sql
UPDATE session_analysis ... analyzed_generation = ?claimed_generation ...;
UPDATE session_evidence ... status = 'ready', analyzed_generation = ?claimed_generation ...;
```

If either guard fails, commit neither projection. A source change leaves the newer generation pending; a fence mismatch means another claim owns the work.

### Persistence policy

- Persist compact derived facts only.
- Do not persist complete canonical events or raw transcript text.
- Start with serde JSON consistent with existing `metrics_json` conventions.
- Include evidence schema revision.
- Use normalized child tables only if measured query/update requirements justify them later.
- Include `session_evidence` in clear/delete and local-data documentation.

## Durable worker model

SQLite is the durable queue. An in-process event only wakes the worker.

### Worker lifecycle

```text
scan transaction marks pending
  → commit
  → wake worker
  → worker claims one generation and increments/receives its claim fence
  → release Antiburn DB lock/transaction
  → acquire resource permits
  → process source in blocking job
  → reacquire Antiburn DB briefly
  → conditional atomic completion
```

On startup, reconcile revision-stale projections and reclaim abandoned `processing` rows whose leases expired. Every reclaim increments the fence before work starts. Duplicate wake-ups are harmless.

### Database connection rule

For the Antiburn application database:

- hold the store mutex/transaction only for short claims, state transitions, reads, and writes;
- never hold it across `await`, `spawn_blocking`, transcript I/O, parsing, analysis, or detector work;
- release statements and guards before doing unrelated work.

The current `Store` may retain its physical writer connection; releasing the guard/transaction is the relevant concurrency boundary.

### Resource limits

Use separate controls rather than one dynamically resized thread pool:

- CPU/job concurrency;
- open source/file concurrency;
- provider SQLite reader concurrency;
- memory-weight permits for materialized fallbacks;
- maximum retained record bytes;
- maximum evidence cardinality;
- cancellation interval.

Acquire permits before scheduling a blocking job. Streaming JSONL receives a small fixed memory weight. Whole-document fallbacks receive weight based on estimated size and run alone when they consume the budget.

Start conservatively with one or two processing jobs and one provider-database reader. Tune only from profiling.

## Cross-session report reduction

### Dedicated read-only Antiburn connection

The current `Store` serializes one persistent connection behind a mutex. Report generation should open a separate read-only connection to the same WAL database.

In `spawn_blocking`:

1. open the read-only/query-only connection with a sensible busy timeout;
2. begin a read transaction to pin one consistent snapshot;
3. execute the thirty-day evidence query;
4. deserialize one compact row;
5. update the report accumulator;
6. discard the row before stepping to the next;
7. finalize the report;
8. drop the statement, transaction, and connection promptly.

WAL permits the normal writer connection to continue committing while the report sees its original snapshot. The report must remain bounded and must not await, read raw transcripts, or perform unrelated long-running work while the snapshot is open.

### Report population

The first report is a **session-start cohort**, not an event-time slice. Select sessions whose trustworthy `started_at_epoch` falls within the trailing thirty days, then consume each selected session's complete evidence. Because every selected session began inside the window, its normal event history is also inside the cohort window.

This deliberately excludes a session that began before the window even if it remained active or was updated recently. The UI/report wording must say “sessions started in the last 30 days,” not imply that it includes every event from every session active during the period.

Select:

- `session.started_at_epoch >= window_start` and `< window_end`;
- current machine/environment scope;
- ready evidence;
- `session_analysis`/`session_evidence` generations matching `session.source_generation` where needed;
- current parser/analyzer/evidence revisions.

A session without a trustworthy start time is not eligible for the cohort and is counted as not assessed rather than assigned discovery time. Account-level quota snapshots continue to use their own observation timestamps within the report window.

Count pending, actively growing/debounced, processing, failed, unsupported, stale, unknown-start, and ready rows separately.

### Candidate query shape

```sql
SELECT
    s.environment_key,
    s.agent,
    s.session_id,
    s.started_at_epoch,
    s.updated_at_epoch,
    s.cwd,
    s.surface,
    e.evidence_json,
    e.diagnostics_json
FROM session s
JOIN session_evidence e
  USING (environment_key, agent, session_id)
WHERE s.started_at_epoch >= ?window_start
  AND s.started_at_epoch < ?window_end
  AND e.status = 'ready'
  AND e.analyzed_generation = s.source_generation
  AND e.parser_revision = ?parser_revision
  AND e.analyzer_revision = ?analyzer_revision
  AND e.evidence_schema_revision = ?evidence_schema_revision
ORDER BY s.started_at_epoch, s.session_id;
```

The final query may join compact `session_analysis` fields where a report requires them, but it must not load canonical transcripts.

### Provider-neutral accumulator

Candidate module:

```text
crates/antiburn-local/src/insights/
  mod.rs
  report.rs
  status.rs
  quota.rs
  detectors/
```

Candidate API:

```rust
pub struct EfficiencyReportAccumulator {
    // bounded shared cross-session state
}

impl EfficiencyReportAccumulator {
    pub fn observe_session(&mut self, session: SessionReportEvidence);
    pub fn observe_usage_limit(&mut self, evidence: UsageLimitEvidence);
    pub fn finish(self, context: ReportContext) -> EfficiencyReport;
}
```

The report engine is implemented once. Each provider maps its source into the shared `SessionEvidence` contract rather than receiving provider-specific report code.

### Report contract

Include:

- computed time;
- window start/end and explicit session-start cohort semantics;
- source/evidence snapshot information;
- discovered count;
- ready/usable count;
- pending/processing/failed/unsupported/stale counts;
- eligible count per detector;
- findings;
- one status per detector;
- quota-limit pressure section/status;
- parser/analyzer/evidence/detector/pricing/catalog revisions;
- coverage and diagnostics summary.

Partial coverage may still support observed findings. A detector may report clean only when its required capabilities and coverage are sufficient.

## Detector evidence requirements

### Sessions Over Depth

- request/turn context depth;
- model/harness context semantics;
- thread/sidechain identity;
- canonical ordering;
- compaction boundaries where relevant.

### Model Overthinking

- explicit reasoning/effort tier;
- provider and model;
- turn/session counts;
- confidence that the setting was directly observed.

### Overpowered Subagents

- parent/main-loop model;
- child identity/model;
- relationship confidence;
- observed override information where available.

### Unused MCP Servers

- loaded server/tool definitions;
- normalized direct invocation names;
- source scope and eligible sessions;
- attribution coverage.

### Unused Built-In Tools

- built-in definitions for the observed provider/version;
- normalized invocations;
- curated disable/remediation support;
- fleet/local validation status.

### Unused Skills

- loaded names;
- installed/project/plugin/bundled origin;
- normalized invocations;
- eligible-session and attribution coverage.

### Old Model Usage

- model identity per turn/session;
- timestamp;
- token/turn quantities;
- replacement catalog revision.

### Overuse of Fast Mode

- explicit fast/service-tier signal;
- main-loop versus delegated work;
- persistent/default signal where available;
- provider-specific impact semantics.

### Cache Churn

- canonical turn order;
- thread/sidechain identity;
- timestamps and idle gaps;
- model changes;
- cache read/write/fresh-input quantities;
- compaction boundaries;
- user-controlled versus provider-eviction confidence.

### Subscription/quota limit pressure

Session-attributable evidence:

- provider;
- observed time;
- limit kind;
- hard hit versus warning;
- reset time/utilization where available;
- model/session attribution;
- confidence.

Account-level evidence:

- provider usage snapshots;
- limit/window type;
- utilization/remaining/reset values;
- incident deduplication.

## Desktop IPC and UI

Candidate commands:

- `get_local_insights_report`;
- `get_local_insights_processing_status`;
- an explicit retry/refresh command only if the product needs it.

The first UI remains Settings → Insights.

Show:

- loading/report calculation state;
- evidence backlog status;
- coverage summary;
- findings;
- clean/not-assessed states;
- quota-limit pressure;
- local-only/freshness wording.

Do not emit raw prompts, tool inputs, transcript content, or unnecessary local paths through IPC or logs.

## Privacy and local-data policy

- Persist derived facts, not complete prompts or canonical events.
- Bound and sanitize examples, names, strings, and diagnostics.
- Avoid tool inputs unless a narrowly defined detector requires a redacted field.
- Never place transcript content in logs, Tauri events, or errors.
- Include evidence in clear/delete behavior.
- Never modify or delete provider source transcripts.
- Update the schema data-policy comment for new retained evidence.

# Super-detailed sequential implementation checklist

The checklist is deliberately ordered. Do not begin a later provider before the first provider has completed its streaming metrics, evidence, persistence, and release-readiness gates.

## Phase 0 — freeze scope and capture the baseline

### Provider and feature scope

- [ ] Select Claude Code as the first JSONL provider unless fixture review identifies a concrete blocker.
- [ ] Record the exact provider/source variants included in the first slice.
- [ ] Keep all other providers on the existing analysis path during the first streaming slice.
- [ ] Confirm that the first implementation rebuilds a changed session from the beginning.
- [ ] Confirm that there is no total 512 MiB JSONL processing cap.
- [ ] Confirm that append-tail checkpoints are out of scope.
- [ ] Confirm that actively written file sources are debounced using `ACTIVE_SESSION_WINDOW_SECS` and cannot monopolize the queue.
- [ ] Confirm that canonical sessions/events will not be persisted.
- [ ] Confirm that the existing session UI must remain behaviorally unchanged.

### Characterization fixtures

- [ ] Inventory existing Claude JSONL fixtures under `crates/antiburn-local/src/analysis/` tests.
- [ ] Add fixture coverage for user, assistant, usage, model, tool, skill, compaction, error, and thinking records.
- [ ] Add repeated timestamps and out-of-order timestamp cases matching current supported behavior.
- [ ] Add malformed JSON between valid lines.
- [ ] Add an incomplete final JSONL record.
- [ ] Add a generated many-record fixture without committing an enormous static file.
- [ ] Add a generated single-oversized-line fixture.
- [ ] Capture expected `NormalizedSession` output for compatibility tests.
- [ ] Capture expected `SessionMetrics` output field by field.
- [ ] Capture expected initial-context and skill-description behavior.
- [ ] Capture expected parent/subagent metrics behavior for Claude where supported.
- [ ] Run and record the existing Rust test commands before changes.

### Completion gate

- [ ] Existing tests pass before implementation.
- [ ] Golden metrics cover every currently displayed `SessionMetrics` field.
- [ ] The selected first-provider scope is written into the plan/implementation PR.

## Phase 1 — add source generations and projection revisions

### Schema migration

- [ ] Append a new migration constant to `apps/desktop/src-tauri/src/store/schema.rs`.
- [ ] Add `source_fingerprint` to `session` with migration-safe null/default behavior.
- [ ] Add `source_generation` to `session` with an initial zero generation.
- [ ] Add nullable `started_at_epoch` to `session`; do not backfill it from `first_seen_at`.
- [ ] Add `analyzed_generation` to `session_analysis`.
- [ ] Add `parser_revision` to `session_analysis`.
- [ ] Add `analyzer_revision` to `session_analysis`.
- [ ] Add `metrics_schema_revision` to `session_analysis`.
- [ ] Do not add `session_evidence` yet in this phase.

### Store model and query updates

- [ ] Extend `SessionRecord` with source fingerprint, generation, and nullable analyzed start time where appropriate.
- [ ] Extend `AnalysisRecord` with analyzed generation and revisions.
- [ ] Update every `session` select/insert/upsert mapping.
- [ ] Update every `session_analysis` select/insert/upsert mapping.
- [ ] Preserve `first_seen_at` behavior.
- [ ] Preserve current clear/delete behavior.
- [ ] Keep existing rows readable after migration.

### Provider-aware source versioning

- [ ] Add storage-neutral `SourceDescriptor`/`SourceVersion` types in `antiburn-local`.
- [ ] Move or supersede desktop-only file `mtime:size` fingerprinting.
- [ ] Include high-resolution metadata and stable identity where available.
- [ ] Reuse `Explorers::provider_db_fingerprint` for database-backed sources.
- [ ] Define inline-source hashing/always-refresh behavior.
- [ ] Ensure fingerprint strings are treated as opaque.

### Discovery integration

- [ ] Compute source version from each discovered `SessionLog` without reading the complete transcript.
- [ ] In the session upsert transaction, compare the observed fingerprint with the stored fingerprint.
- [ ] Increment source generation only when the fingerprint changes.
- [ ] Keep generation unchanged when the same fingerprint is rediscovered.
- [ ] Set first reusable fingerprint to generation 1.
- [ ] Ensure WSL/native identities remain distinct.

### Tests

- [ ] Test fresh migration.
- [ ] Test migration from every current schema version.
- [ ] Test first fingerprint observation.
- [ ] Test same-fingerprint idempotence.
- [ ] Test changed-fingerprint generation increment.
- [ ] Test nullable `started_at_epoch` migration and query mapping.
- [ ] Test provider DB fingerprint mapping.
- [ ] Test inline-source behavior.
- [ ] Test serialization/query mappings for new fields.

### Completion gate

- [ ] Existing scan behavior remains functional.
- [ ] Existing analysis cache records have explicit generation/revision semantics.
- [ ] No complete transcript is read to decide whether a source changed.

## Phase 2 — build the bounded JSONL framing primitive

### Module and API

- [ ] Add a focused bounded JSONL reader module in `antiburn-local`.
- [ ] Keep it synchronous so it can run inside one blocking processing job.
- [ ] Define a record result for complete, malformed, oversized, incomplete-tail, cancelled, and I/O-error cases.
- [ ] Add processing control/cancellation hooks.
- [ ] Add bounded diagnostics counters.

### Bounded framing implementation

- [ ] Use `BufRead::fill_buf` or equivalent chunk scanning.
- [ ] Find newline boundaries without allocating the whole file.
- [ ] Retain bytes only up to the configured maximum individual record size.
- [ ] Drain an oversized record through its newline without retaining the remainder.
- [ ] Resume correctly at the next record.
- [ ] Do not treat an incomplete final record as committed.
- [ ] Do not use unbounded `read_until` into a growing `Vec`.
- [ ] Check cancellation between records or bounded byte intervals.
- [ ] Preserve byte/record positions needed for diagnostics without retaining content.

### Safety and behavior tests

- [ ] Parse a normal multi-line fixture.
- [ ] Recover after one malformed line.
- [ ] Recover after one oversized line.
- [ ] Ignore/diagnose an incomplete final line.
- [ ] Process a generated source larger than the retained-memory budget.
- [ ] Assert retained line buffer never exceeds its configured bound.
- [ ] Assert diagnostics do not contain transcript content.
- [ ] Assert cancellation stops before the next record.

### Completion gate

- [ ] The framing primitive can scan an arbitrarily large JSONL file with bounded retained record memory.
- [ ] No provider semantics have changed yet.

## Phase 3 — add the normalized-record visitor for Claude

### Interface

- [ ] Add `NormalizedRecordSink` or the smallest equivalent visitor interface.
- [ ] Add `VendorAdapter::visit_source` for the first-provider path.
- [ ] Keep `VendorAdapter::normalize` and `normalize_source` working.
- [ ] Avoid a generic async stream unless a current requirement demands it.
- [ ] Keep provider parsing inside provider modules.

### Claude parser conversion

- [ ] Route Claude file input through the bounded JSONL reader.
- [ ] Parse one JSON object per retained line.
- [ ] Extract current normalized event fields.
- [ ] Emit normalized records immediately.
- [ ] Drop the `serde_json::Value` before reading the next record.
- [ ] Preserve malformed-record tolerance.
- [ ] Preserve model and context-window semantics.
- [ ] Preserve tool categorization and normalized naming.
- [ ] Preserve compaction markers.
- [ ] Preserve sidechain/subagent identity where supported.
- [ ] Emit source/parser capability and coverage observations.

### Compatibility collector

- [ ] Implement a collector sink that builds `NormalizedSession` for compatibility tests/tools.
- [ ] Compare collector output with the legacy Claude normalizer fixture by fixture.
- [ ] Document any intentional difference before changing a golden result.

### Source consistency

- [ ] Capture the source version before opening.
- [ ] Recheck source metadata/version after the final record.
- [ ] Return `SourceChanged` when the source changed during processing.
- [ ] Ensure partial final writes by the provider do not become committed evidence.

### Completion gate

- [ ] Claude normalization works record by record.
- [ ] Compatibility collector matches the existing normalized model.
- [ ] Legacy full-file normalization remains available only where still required.

## Phase 4 — stream existing Claude `SessionMetrics`

### Metrics accumulator

- [ ] Add `SessionMetricsAccumulator` using shared normalization/classification helpers.
- [ ] Update event count online.
- [ ] Update token totals online.
- [ ] Update billable token classes online.
- [ ] Update model breakdown online.
- [ ] Update peak context online.
- [ ] Update tool counts/mix online.
- [ ] Update error/disruption counts online.
- [ ] Update skill uses online.
- [ ] Update compaction observations online.
- [ ] Retain compact timing/phase/skill points needed for exact finalization, accepting that these collections may grow with metric-bearing events in the first implementation.
- [ ] Finalize duration and active time at end of stream.
- [ ] Finalize buckets and phase segments at end of stream.
- [ ] Finalize pattern score/signals at end of stream.
- [ ] Apply existing pricing semantics without changing displayed results.

### Initial-context integration

- [ ] Identify every Claude raw record currently consumed by the separate initial-context pass.
- [ ] Emit context-source observations during the same provider parse.
- [ ] Accumulate skill/MCP/instruction attribution without rereading the transcript.
- [ ] Accumulate skill descriptions without rereading the transcript.
- [ ] Preserve tracked/partial/unavailable semantics.

### Parent/subagent integration

- [ ] Stream each parent/child source independently.
- [ ] Preserve the existing roster and relationship outputs.
- [ ] Retain only compact timing data needed to calculate spawn progress.
- [ ] Avoid reparsing the parent solely to obtain spawn positions.

### Desktop generation path

- [ ] Route Claude `analytics::analyze` through the streaming processor.
- [ ] Keep CPU work inside `spawn_blocking`.
- [ ] Write `session_analysis.analyzed_generation` from the claimed/current source generation.
- [ ] Derive `session.started_at_epoch` from trustworthy provider start metadata or `SessionMetrics.first_ts_ms`; leave it unknown when neither exists.
- [ ] Write parser/analyzer/metrics revisions.
- [ ] Keep other providers on the existing path.

### Equivalence tests

- [ ] Compare every `SessionMetrics` scalar.
- [ ] Compare model breakdown and costs.
- [ ] Compare tool mix and counts.
- [ ] Compare skills and descriptions.
- [ ] Compare context availability/fraction/window.
- [ ] Compare buckets, phase distributions, and segments.
- [ ] Compare pattern scores and signals.
- [ ] Compare parent/subagent behavior.
- [ ] Explain and approve any intentional output difference.

### Performance tests

- [ ] Demonstrate no whole-file `String` in the Claude metrics path.
- [ ] Demonstrate no complete `Vec<NormalizedEvent>` in normal Claude metrics generation.
- [ ] Measure retained accumulator state on a generated large session and document its proportional growth.
- [ ] Confirm the improvement claim is limited to bounded raw-record retention and removal of whole-file/canonical-event materialization, not bounded rich metrics memory.
- [ ] Measure processing time relative to the legacy path.
- [ ] Confirm active UI behavior remains unchanged.

### Completion gate

- [ ] Existing Claude `session_analysis` records are generated from a one-record-at-a-time source pass.
- [ ] Existing user-visible metrics remain equivalent.
- [ ] The old Claude full-read metrics path is removed or restricted to explicit compatibility tests/tools.
- [ ] The implementation and documentation explicitly accept that exact rich metrics accumulation remains unbounded in the first version.

## Phase 5 — add one in-memory evidence data point

### Evidence type shell

- [ ] Add the complete anticipated `SessionEvidence` grouping/type shell.
- [ ] Add source capabilities, coverage, provenance, and diagnostics fields.
- [ ] Add the temporary debug-only `Unimplemented` variant.
- [ ] Initialize not-yet-implemented groups as `Unimplemented` in debug development code.
- [ ] Do not add persistence in this phase.
- [ ] Do not expose evidence through production IPC in this phase.

### First real evidence field

- [ ] Choose `max_request_context_tokens` as the first rule-neutral evidence value.
- [ ] Feed `SessionEvidenceAccumulator` from the same Claude normalized-record stream.
- [ ] Update the maximum online.
- [ ] Distinguish unsupported context evidence from complete zero and partial observation.
- [ ] Attach parser/source coverage.
- [ ] Finalize a complete in-memory `SessionEvidence` object at end of stream.

### Composite sink

- [ ] Ensure one normalized record updates both metrics and evidence accumulators.
- [ ] Ensure the line and parsed value are still dropped before the next record.
- [ ] Ensure adding evidence does not require a second source pass.
- [ ] Ensure `SessionMetrics` output remains unchanged.

### Tests

- [ ] Assert one fixture's maximum request depth.
- [ ] Assert malformed/oversized relevant records produce partial coverage.
- [ ] Assert unsupported source semantics.
- [ ] Assert a complete observed absence is distinguishable from unsupported.
- [ ] Assert evidence object serde shape in debug tests.

### Completion gate

- [ ] One Claude source pass produces both existing metrics and one correct in-memory evidence value.
- [ ] All other evidence groups remain visibly temporary and unshipped.

## Phase 6 — add durable `session_evidence` storage

### Migration

- [ ] Append the `session_evidence` table migration.
- [ ] Add `claim_fence`, `lease_expires_at`, and `next_attempt_at` work-state columns.
- [ ] Add lifecycle/version indexes.
- [ ] Add composite foreign key with delete cascade.
- [ ] Preserve immutable prior migrations.

### Store models and methods

- [ ] Add `SessionEvidenceRecord` with identity, lifecycle, generation, revisions, JSON, and diagnostics.
- [ ] Add a transactional method to create/mark pending evidence when source generation changes without deleting the last completed payload.
- [ ] Add revision reconciliation that requeues stale metrics/evidence without changing source generation.
- [ ] Add an atomic claim method that increments and returns `claim_fence`.
- [ ] Add conditional completion guarded by source generation and claim fence.
- [ ] Guard failure, retry, and lease-renewal transitions with the same fence.
- [ ] Add failed/unsupported transitions.
- [ ] Add abandoned-processing reclamation that increments the fence before reassignment.
- [ ] Add bounded retry count and local error summary.
- [ ] Update session deletion.
- [ ] Update clear-local-session-data.

### Development persistence

- [ ] Serialize the current complete `SessionEvidence` shell into `evidence_json` in debug development builds.
- [ ] Allow temporary debug-only `Unimplemented` values while this provider is actively being completed.
- [ ] Remember that debug builds use the separate Antiburn debug database.
- [ ] Do not claim production readiness while any provider field remains `Unimplemented`.

### Atomic dual projection write

- [ ] Finalize `SessionMetrics` and `SessionEvidence` from the same stream.
- [ ] Recheck source version.
- [ ] Begin one short Antiburn-store transaction.
- [ ] Verify claimed generation equals current `session.source_generation`.
- [ ] Verify status is processing and the worker's claim fence is still current.
- [ ] Update `session.started_at_epoch` from finalized analysis when trustworthy.
- [ ] Update `session_analysis`.
- [ ] Update `session_evidence`.
- [ ] Commit both or neither.
- [ ] Drop stale output when the generation changed.

### Tests

- [ ] Fresh evidence row starts pending.
- [ ] Same generation does not duplicate work.
- [ ] Only one initial claim succeeds.
- [ ] Reclaim increments the fence.
- [ ] A late worker with an old fence cannot complete, fail, renew, or retry the work.
- [ ] Completion writes both projections.
- [ ] Stale generation writes neither projection.
- [ ] Parser/analyzer/metrics/evidence revision changes requeue unchanged sources.
- [ ] Detector, remediation, pricing, and model-replacement catalog changes do not reparse transcript evidence.
- [ ] Evidence JSON round-trips.
- [ ] Marking a newer generation pending preserves the prior analyzed generation and evidence payload.
- [ ] Delete cascades.
- [ ] Clear removes evidence/work state.
- [ ] Abandoned processing is reclaimed.

### Completion gate

- [ ] Claude processing can persist metrics and the current evidence shell atomically.
- [ ] No canonical records or raw transcript content are persisted.

## Phase 7 — decouple processing from scan and add the durable worker

### Worker module

- [ ] Add `apps/desktop/src-tauri/src/insights_worker.rs` or the smallest matching module.
- [ ] Make SQLite pending rows the durable queue.
- [ ] Add a post-commit wake-up mechanism.
- [ ] Start/recover the worker with the desktop application.
- [ ] Reconcile projection revision mismatches before normal claiming.
- [ ] Claim one generation at a time initially and carry its returned fence through every transition.
- [ ] Release the Antiburn store guard before source processing.
- [ ] Run parsing/analysis in `spawn_blocking`.
- [ ] Reacquire the store only for short completion/failure transitions.

### Scan integration

- [ ] Make discovery upsert metadata and mark new generations pending for the currently enabled streaming-provider cohort.
- [ ] Do not create an endlessly retrying pending backlog for providers that have not entered the streaming implementation loop.
- [ ] Emit worker wake-up after commit.
- [ ] Do not make scan completion wait for the evidence backlog.
- [ ] Decide how the existing `top_up_analysis` entry point delegates to or coexists with the shared worker during transition.
- [ ] Avoid processing the same generation through both paths.

### Resource controls

- [ ] Add a small CPU/job semaphore.
- [ ] Add a source/file semaphore.
- [ ] Add a provider-DB semaphore.
- [ ] Exclude file sources updated within `ACTIVE_SESSION_WINDOW_SECS` from claiming and set their next eligible attempt time.
- [ ] Order eligible claims fairly by `next_attempt_at` and stable tie-breakers rather than repeatedly selecting the hottest session.
- [ ] Add memory weighting only for materialized fallbacks.
- [ ] Acquire permits before scheduling blocking work.
- [ ] Check cancellation between source records.
- [ ] Stop/release cleanly on app shutdown.

### Failure and recovery

- [ ] Retry transient source-busy/read failures with bounded backoff and `next_attempt_at`.
- [ ] Treat `SourceChanged` as deferred active work, not an immediate tight-loop retry.
- [ ] Increase backoff when the same source repeatedly changes during processing.
- [ ] Continue claiming other eligible sessions while an active source is deferred.
- [ ] Do not retry unsupported provider/schema forever.
- [ ] Reclaim stale processing leases on startup by issuing a new fence.
- [ ] Reject every late transition carrying an older fence.
- [ ] Ensure errors contain no transcript content.
- [ ] Keep a newer generation pending when an older job finishes.
- [ ] Test that a continuously growing transcript is deferred rather than immediately reclaimed.
- [ ] Test that stable sessions continue processing while a growing transcript is deferred.
- [ ] Test that a deferred session becomes claimable after the quiet period.
- [ ] Test that deferral preserves its last completed evidence payload.

### Completion gate

- [ ] Discovery and processing are independent.
- [ ] A process restart cannot lose pending work.
- [ ] Normal user work is not blocked by the Antiburn database mutex during transcript processing.

## Phase 8 — complete Claude evidence one field group at a time

For every group below, perform the same sequence before moving on:

- [ ] identify exact Claude source records/fields;
- [ ] add or reuse normalized record fields;
- [ ] implement bounded accumulator state;
- [ ] define `Complete`/`Partial`/`Unsupported` semantics;
- [ ] add capability declaration;
- [ ] add malformed/schema-drift behavior;
- [ ] add golden fixtures;
- [ ] persist and reload the evidence;
- [ ] verify existing `SessionMetrics` remain unchanged;
- [ ] replace the group's temporary `Unimplemented` value.

### Group 8.1 — context and eligibility

- [ ] Implement request context depths, not only session peak.
- [ ] Implement request/session counts needed for eligibility.
- [ ] Retain bounded top-depth examples without prompt text.
- [ ] Capture context-window provenance.
- [ ] Capture thread/sidechain distinctions.
- [ ] Capture ordering/compaction coverage.
- [ ] Replace context/eligibility `Unimplemented` states.

### Group 8.2 — named tool usage

- [ ] Normalize tool names consistently with existing metrics.
- [ ] Count invocations by normalized name.
- [ ] Classify built-in, MCP, skill, and other tools where evidence supports it.
- [ ] Bound unique-name cardinality.
- [ ] Mark cap breach partial.
- [ ] Preserve complete empty usage only when invocation coverage is complete.
- [ ] Replace tool `Unimplemented` state.

### Group 8.3 — loaded skills and MCP sources

- [ ] Parse loaded skill names.
- [ ] Parse skill origin where available.
- [ ] Parse loaded MCP servers/tool definitions.
- [ ] Match loaded sources with normalized invocations.
- [ ] Move initial-context source extraction into the one-pass parser observations.
- [ ] Preserve unattributed/partial semantics.
- [ ] Bound source-name cardinality and descriptions.
- [ ] Replace context-source `Unimplemented` state.

### Group 8.4 — model usage and old-model facts

- [ ] Record model identity by turn/session.
- [ ] Record token quantities by normalized model.
- [ ] Record timestamps needed for replacement availability rules.
- [ ] Preserve unknown model identity as unsupported/partial, not an invented model.
- [ ] Keep replacement policy out of persisted evidence.
- [ ] Replace model-usage `Unimplemented` fields that are now complete.

### Group 8.5 — reasoning effort

- [ ] Identify explicit Claude reasoning/effort fields.
- [ ] Do not infer settings from prompt keywords.
- [ ] Count observed tiers.
- [ ] Record capability by harness/schema version.
- [ ] Mark sessions without an exposed tier correctly.
- [ ] Replace reasoning `Unimplemented` state.

### Group 8.6 — subagent relationships and models

- [ ] Preserve parent/subagent identity from discovery/parser evidence.
- [ ] Record main-loop model.
- [ ] Record child model.
- [ ] Record relationship confidence/provenance.
- [ ] Avoid double-counting sidechain transcripts.
- [ ] Bound child/example collections.
- [ ] Replace subagent `Unimplemented` state.

### Group 8.7 — fast/service tier

- [ ] Identify explicit Claude fast-mode/service-tier evidence.
- [ ] Distinguish main-loop and delegated work.
- [ ] Record observations, not current policy conclusions.
- [ ] Mark unavailable default-persistence evidence honestly.
- [ ] Replace fast-mode `Unimplemented` state.

### Group 8.8 — compaction and cache churn

- [ ] Preserve canonical turn order.
- [ ] Maintain previous turn per thread/sidechain.
- [ ] Record cache read/write/fresh-input quantities.
- [ ] Record model transitions.
- [ ] Record timestamps/idle gaps.
- [ ] Record compaction boundaries.
- [ ] Bound thread cardinality and mark overflow partial.
- [ ] Separate observed user-controlled churn from provider-eviction estimates.
- [ ] Replace cache/compaction `Unimplemented` states.

### Group 8.9 — built-in tool validation facts

- [ ] Record built-in definitions exposed by the harness/version.
- [ ] Join definitions to named invocation evidence.
- [ ] Record provider/version capability.
- [ ] Keep curated remediation/disable knowledge outside session evidence.
- [ ] Mark fleet-validation limitations separately.
- [ ] Replace built-in-tool `Unimplemented` state where Claude can support it.

### Group 8.10 — transcript-attributable quota incidents

- [ ] Identify explicit rate/quota error shapes.
- [ ] Record provider, limit kind, observed time, model/session, and reset/utilization when exposed.
- [ ] Distinguish warning and hard hit.
- [ ] Deduplicate repeated messages within one incident.
- [ ] Preserve confidence and unavailable fields.
- [ ] Replace session-quota `Unimplemented` state.

### Claude evidence completion gate

- [ ] Review every `SessionEvidence` field.
- [ ] Convert each field to truthful `Complete`, `Partial`, or `Unsupported` behavior.
- [ ] Remove the debug-only `Unimplemented` variant entirely.
- [ ] Remove all temporary/fake provider values.
- [ ] Run debug tests after removing the variant.
- [ ] Run `cargo check --release` and the release build/test commands.
- [ ] Confirm release compilation finds no remaining placeholder use.
- [ ] Confirm evidence persisted by the completed provider contains no implementation placeholders.
- [ ] Publish the Claude capability/coverage matrix in the plan or fixtures.

## Phase 9 — build the streaming report reducer

### Read-only Antiburn connection

- [ ] Add a dedicated read-only connection opener using the Antiburn database path.
- [ ] Configure read-only/query-only behavior and a busy timeout.
- [ ] Keep the normal writer connection in WAL mode.
- [ ] Run report work in `spawn_blocking`.
- [ ] Begin one read transaction for a consistent snapshot.
- [ ] Do not use the writer connection's mutex for the report scan.

### Row-streaming query

- [ ] Select session identity/metadata and current ready evidence only for sessions whose trustworthy start falls inside the thirty-day cohort.
- [ ] Exclude and count sessions with unknown start time rather than substituting discovery time.
- [ ] Filter source/analyzed generations and revisions.
- [ ] Order deterministically.
- [ ] Deserialize one `SessionEvidence` row at a time.
- [ ] Feed it into the report accumulator immediately.
- [ ] Drop it before stepping to the next row.
- [ ] Do not collect canonical sessions.
- [ ] Do not read provider transcripts.
- [ ] Drop statement/transaction/connection promptly after finalization.

### Coverage counts

- [ ] Count discovered sessions in the window.
- [ ] Count ready/current sessions.
- [ ] Count pending sessions.
- [ ] Count actively growing/debounced sessions separately where the source state is known.
- [ ] Count processing sessions.
- [ ] Count failed sessions.
- [ ] Count unsupported sessions.
- [ ] Count stale generation/revision sessions.
- [ ] Count unknown-start sessions excluded from the cohort.
- [ ] Expose per-detector eligible/assessed counts.

### Composite accumulator

- [ ] Add provider-neutral shared aggregates.
- [ ] Bound maps and supporting examples.
- [ ] Avoid cloning one evidence vector per detector.
- [ ] Allow finalization after denominators are known.
- [ ] Keep detector logic pure and deterministic.
- [ ] Apply pricing and remediation catalogs at report time.

### Nine detectors

- [ ] Implement Sessions Over Depth status/findings.
- [ ] Implement Model Overthinking status/findings.
- [ ] Implement Overpowered Subagents status/findings.
- [ ] Implement Unused MCP Servers status/findings.
- [ ] Implement Unused Built-In Tools status/findings.
- [ ] Implement Unused Skills status/findings.
- [ ] Implement Old Model Usage status/findings.
- [ ] Implement Overuse of Fast Mode status/findings.
- [ ] Implement Cache Churn status/findings.
- [ ] Assert exactly one status per detector.
- [ ] Assert incomplete absence never produces clean.

### Subscription/quota pressure

- [ ] Add account-level `UsageLimitEvidence` input from existing provider usage snapshots.
- [ ] Combine account-level and session-attributable incidents.
- [ ] Deduplicate incidents.
- [ ] Report limit kind, hit count, time blocked where defensible, affected sessions/models, and reset behavior.
- [ ] Mark provider coverage not assessed when neither evidence source exists.
- [ ] Keep this section separate from the nine compatibility categories.

### Concurrency integration test

- [ ] Open the normal writer connection.
- [ ] Continuously update evidence from a test writer.
- [ ] Open a separate read-only report connection.
- [ ] Pin a report snapshot.
- [ ] Stream a deterministic report while writes continue.
- [ ] Assert the report sees one consistent snapshot.
- [ ] Assert the writer is not blocked for the duration of unrelated analysis work.
- [ ] Assert the read transaction is dropped after finalization.

### Completion gate

- [ ] The thirty-day report is computed from compact database evidence only.
- [ ] Peak report memory is bounded by accumulator state plus one evidence row.
- [ ] Concurrent evidence updates continue through WAL.
- [ ] Coverage counts and detector statuses are truthful.

## Phase 10 — expose the report through desktop IPC and UI

### Commands and DTOs

- [ ] Add report request/response DTOs.
- [ ] Add `get_local_insights_report`.
- [ ] Add processing-status data required by the pane.
- [ ] Deduplicate concurrent report requests if needed.
- [ ] Ensure request IDs do not substitute for actual cancellation.
- [ ] Keep transcript content out of DTOs.

### Progress and cancellation

- [ ] Expose report calculation state.
- [ ] Expose evidence pending/processing counts.
- [ ] Allow pane closure/app shutdown to cancel report work promptly.
- [ ] Do not cancel durable evidence state incorrectly when the UI disappears.

### Settings UI

- [ ] Register the Insights settings pane.
- [ ] Add loading state.
- [ ] Add evidence-backlog/coverage state.
- [ ] Add findings rendering.
- [ ] Add clean/not-assessed rendering per category.
- [ ] Add local quota-pressure section.
- [ ] Add local/freshness explanation.
- [ ] Avoid dismissals/history/notifications in the first version.
- [ ] Keep session-level cards out of this implementation.

### Completion gate

- [ ] A user can open a thirty-day local report.
- [ ] The report explains incomplete provider/session coverage.
- [ ] No unsupported detector appears clean.
- [ ] Findings include defensible evidence and remediation.

## Phase 11 — finish first-provider operational hardening

### Resource behavior

- [ ] Measure discovery, queue wait, source reading, parsing, metrics accumulation, evidence accumulation, persistence, report query, reduction, and IPC separately.
- [ ] Measure peak retained line buffer.
- [ ] Measure compact metrics/evidence accumulator memory.
- [ ] Measure peak report memory.
- [ ] Measure impact while a representative Claude session is actively writing.
- [ ] Tune worker concurrency from measurements.
- [ ] Tune provider DB concurrency separately when applicable.

### Privacy and lifecycle

- [ ] Review every persisted evidence field for content sensitivity.
- [ ] Cap/sanitize supporting examples.
- [ ] Verify errors/logs contain no transcript content.
- [ ] Verify clear/delete behavior.
- [ ] Update schema data-policy comments.
- [ ] Update any local-data UI/documentation.

### Release gate

- [ ] Run formatting and linting.
- [ ] Run all Rust tests.
- [ ] Run release compilation/tests.
- [ ] Verify no `Unimplemented` placeholder remains.
- [ ] Verify no fake evidence remains.
- [ ] Verify first-provider capability matrix.
- [ ] Verify migration and downgrade/recovery expectations.
- [ ] Verify report output on synthetic and manually inspected real sessions.

## Phase 12 — repeat the provider implementation loop

For each additional JSONL provider:

- [ ] inventory source shape and existing adapter behavior;
- [ ] add provider characterization fixtures;
- [ ] implement bounded record streaming;
- [ ] prove normalized compatibility;
- [ ] prove `SessionMetrics` parity;
- [ ] feed the shared metrics/evidence composite sink;
- [ ] implement each evidence group with truthful capability states;
- [ ] persist both projections atomically;
- [ ] add provider report fixtures;
- [ ] add provider capability/coverage matrix;
- [ ] pass debug and release completion gates;
- [ ] enable the provider in durable processing.

For each provider SQLite source:

- [ ] inventory schema and consistency semantics;
- [ ] implement a read-only ordered row iterator;
- [ ] hold only the provider read transaction while consuming rows;
- [ ] normalize one row at a time;
- [ ] avoid synthetic full-session JSONL;
- [ ] test concurrent provider writes where feasible;
- [ ] prove metrics parity;
- [ ] complete all supported evidence groups;
- [ ] persist and report through the shared contracts;
- [ ] pass the provider completion gate.

Do not add provider-specific branches to the report reducer where a normalized evidence fact can express the difference. Provider-specific semantics belong in parsing, capability, provenance, and catalog layers.

## Phase 13 — measured future optimizations only

- [ ] Profile before adding append-tail processing.
- [ ] Evaluate captured-prefix processing for proven append-only providers before implementing resumable checkpoints: process complete records up to the claimed starting length, publish that completed generation, and leave newer appended work pending.
- [ ] Add byte-offset checkpoints only for providers with safe append semantics and measured need.
- [ ] Include file identity, complete-line offset, boundary hash, parser/analyzer revisions, and accumulator continuation state in any checkpoint.
- [ ] Rebuild on truncation, replacement, or mismatch.
- [ ] Profile before adding report caching.
- [ ] Include source generations and all relevant revisions in any report cache identity.
- [ ] Profile exact rich-metrics retained collections before moving them to generation-scoped child/staging SQL tables or another disk-backed spool.
- [ ] Offload growing metrics details only when measurements justify the extra write/finalization path; keep partial generations invisible until published.
- [ ] Profile before normalizing evidence into child SQL tables.
- [ ] Add additional read connections/pooling only if compact report reads contend measurably.
- [ ] Keep background processing lazy/low-impact unless product evidence justifies stronger eagerness.

## File-level implementation map

### `crates/antiburn-local`

Likely additions:

```text
src/discovery/source_version.rs
src/analysis/jsonl_reader.rs
src/analysis/stream.rs
src/analysis/evidence.rs
src/analysis/process.rs
src/insights/mod.rs
src/insights/report.rs
src/insights/status.rs
src/insights/quota.rs
src/insights/detectors/*.rs
```

Likely modifications:

```text
src/discovery/mod.rs
src/analysis/interface.rs
src/analysis/model.rs
src/analysis/vendors/*.rs
src/analysis/initial_context.rs
src/analysis/engine.rs
src/analysis/mod.rs
src/lib.rs
```

### Desktop Rust application

Likely additions:

```text
apps/desktop/src-tauri/src/insights_worker.rs
```

Likely modifications:

```text
apps/desktop/src-tauri/src/store/schema.rs
apps/desktop/src-tauri/src/store/model.rs
apps/desktop/src-tauri/src/store/mod.rs
apps/desktop/src-tauri/src/scan.rs
apps/desktop/src-tauri/src/analytics.rs
apps/desktop/src-tauri/src/commands.rs
Tauri application setup/state registration
clear/delete/export/privacy paths
```

### Desktop frontend

Likely modifications/additions:

```text
apps/desktop/src/lib/settingsPanes.ts
apps/desktop/src/views/SettingsView.tsx
Insights pane components
Tauri command types/hooks
coverage/finding/status views
```

## Kent Beck delivery framing

### Make It Work

- Complete Phases 0–10 for Claude.
- Rebuild changed sessions from the beginning.
- Keep concurrency conservative.
- Persist compact evidence.
- Render the full category contract with honest not-assessed states.
- Test whether recommendations are useful.

### Make It Good/Right

- Complete first-provider hardening and capability matrices.
- Remove repeated raw passes.
- Stabilize source/parser/evidence revisions.
- Prove restart, race, deletion, and privacy behavior.
- Add providers one at a time through the same completion gate.

### Make It Fast

- Tune measured resource budgets.
- Add safe append-tail processing only when justified.
- Add report caching or relational evidence projections only when compact row streaming misses the agreed budget.

## Decisions still required before implementation

1. Exact first-provider fixture cohort and supported Claude schema versions.
2. Maximum retained JSONL record bytes based on observed legitimate records.
3. Initial worker CPU/source concurrency and processing lease duration.
4. Whether opening Insights accelerates pending thirty-day work.
5. Exact evidence fields retained as bounded supporting examples.
6. Parser, analyzer, metrics, evidence, detector, pricing, and remediation revision constants.
7. Which partial evidence permits a finding and which prevents a clean status for each detector.
8. The second provider selected after Claude; choosing a provider SQLite source early would validate the row-streaming path.
9. Acceptable first-open evidence backlog and report-readiness experience.
