# GH-227 — Codex through the streaming insights pipeline

Branch `feat/227-codex-provider` starts at `origin/main` commit `e5fcf8a`.

## Public format authority

Research uses the public `openai/codex` repository at commit `e9a446d`.

- `codex-rs/history/src/lib.rs`: `RolloutLine` has a timestamp, an optional ordinal, and a flattened `RolloutItem`.
- `codex-rs/history/src/rollout_payload.rs`: the wire representation uses `{timestamp,type,payload}`.
- `codex-rs/rollout/src/policy.rs`: `is_persisted_rollout_item`, `should_persist_response_item`, and `should_persist_event_msg` define what reaches a rollout.
- `codex-rs/protocol/src/protocol.rs`: `TokenUsage`, `TokenUsageInfo`, `TokenCountEvent`, `TurnContextItem`, and `SessionMeta` define usage and session facts.
- `codex-rs/protocol/src/items.rs`: `TurnItem` and `SubAgentActivityItem` define paginated and delegated activity facts.

Codex writes rollout JSONL under `~/.codex/sessions/YYYY/MM/DD/`. Its state SQLite database is separate and is not a rollout input. The public rollout writer creates and appends JSONL records. It exposes no rollout compaction or in-place rewrite operation. Antiburn still declares `AppendOnlyGuarantee::Absent`: external behavior cannot be proven by synthetic fixtures, so every claimed Codex read gets a full source recheck.

The repository fixtures are hand-authored. No captured transcript or local key value enters the repository.

## Capability decision

`SourceCapabilities::codex()` ships this matrix:

| Capability | State | Reason |
| --- | --- | --- |
| Request context tokens | true | The adapter extracts the latest request input and cached-input counts from `token_count`. |
| Cache-write tokens | false | The public type has a defaulted field, but rollouts do not reliably emit it. |
| Timestamps and order | true | Every public rollout line has a timestamp. Invalid or missing event timestamps degrade coverage. |
| Tool invocations | true | The adapter extracts persisted response tool variants. Unknown and paginated variants degrade the tools group. |
| Skill and MCP attribution | false | Legacy tool records do not reliably identify the loaded source. |
| Tool definitions | false | Rollouts do not carry a complete tool catalogue. |
| Model identity | true | The adapter extracts `turn_context.model`. Missing attribution makes models partial. |
| Token classes | true | Input, cached input, output, and reasoning output are distinct source fields. |
| Reasoning effort tier | true | The adapter extracts `turn_context.effort`. Missing effort on a used model degrades coverage. |
| Fast tier | false | No equivalent speed signal exists. |
| Service tier | false | The adapter does not extract the sparse setting in this slice. |
| Subagent relationships | false | Discovery relates files, but the evidence adapter does not publish those relations. |
| Subagent models | false | A subagent activity item identifies a thread and path, not its model. |
| Compaction boundaries | true | The adapter extracts top-level and legacy compaction records and deduplicates sibling markers. |
| Thread identity | false | Codex records have no per-record parent identity chain. `cache.previousTurn` is Unsupported. |
| Quota incidents | false | The sink has no quota incident ingestion path in this slice. |
| Harness version | false | The sink has no harness-version ingestion path in this slice. |

The last two flags stay false even though the source can carry related fields. Advertising them without populated evidence would violate the capability contract.

Only Model Overthinking and Old Model Usage satisfy every capability prerequisite. Sessions Over Depth remains not assessed because it requires thread identity. Cache Churn, Overpowered Subagents, fast-mode, MCP, skills, and built-in-tool detectors also remain not assessed. Tests freeze this detector table.

## Streaming design

`CodexAdapter::visit` uses `BoundedJsonlReader` for file and in-memory JSONL sources. It emits unusable records for malformed, oversized, incomplete-tail, and unrecognized records. Unrecognized diagnostics use one fixed technical discriminator and retain no source text. `visit_claimed` uses `PinnedSource`; `AppendOnlyGuarantee::Absent` selects `recheck_full`. A changed source never calls `finish`, so neither metrics nor evidence can publish.

The desktop has one `stream_vendor` path for Claude and Codex. It derives capabilities from each input's vendor label and calls the selected adapter through `VendorAdapter`. No Codex copy of the Claude function exists. This path serves `analyze_for_evidence`, the evidence test pass, and `analyze_subagent`. A Codex publication test asserts the evidence matrix is exactly `SourceCapabilities::codex()`.

The fork replay predicate runs incrementally. While a child fork is pending, inherited `token_count` rows do not reach either accumulator. The addressed child marker resolves ownership. If the marker never arrives, inherited usage remains excluded and an end-of-stream attribution gap makes coverage partial. A fixture verifies resolved streaming equals the existing whole-file parser. Another fixture verifies unresolved usage remains zero.

The adapter reads `session_meta.payload.timestamp` as the provider start, with the rollout envelope timestamp as its fallback. The desktop uses this start before the earliest normalized event. It never uses `first_seen_at`.

## Discovery and durable queue

`AgentKind::Codex.slug()` is `codex`, which also matches its adapter vendor label and stored `SessionKey.agent`. `evidence_cohort()` contains `claude-code` and `codex` discovery slugs.

A v12 store migration inserts missing `session_evidence` rows for every already-discovered Codex session. Conflict handling preserves any existing ready, pending, processing, unsupported, or failed row. Future scans queue new Codex generations through the normal `upsert_sessions` path.

The worker rejects unknown stored slugs as unsupported. It never defaults an unknown slug to Claude. Provider database source reconstruction receives an already-validated `AgentKind`.

Discovery marks Codex rollout files as `RecordStream`. Existing path discovery and source fingerprinting remain unchanged.

## Characterization and source safety

`tests/fixtures/codex_characterization` covers supported records, malformed records between valid neighbors, an incomplete active-writer tail, a fixed unknown type, missing model and effort attribution, resolved fork replay, and unresolved fork replay. Checked-in goldens include normalized events, metrics, evidence, and coverage reasons.

The active-writer test requires `IncompleteTail` and `EvidenceCoverage::Partial(IncompleteTail)`. The changed-source test requires `VisitOutcome::SourceChanged` and no finished session. The source-guarantee test pins Codex to `Absent`.

Capability contract tests run for both Claude and Codex. A true capability must expose its evidence group. A false capability must remain Unsupported or must contain no claimed values in its represented field. Claude characterization and exact evidence serialization tests remain unchanged.

## Revision decision

`PARSER_REVISION = 3`, `ANALYZER_REVISION = 5`, and `EVIDENCE_SCHEMA_REVISION = 2` remain unchanged. The implementation adds a provider constructor and internal streaming summary facts. It does not change extraction or serialized evidence for an existing provider. Quota and harness evidence remain unsupported, so no schema field gains new meaning. The existing exact Claude literals remain byte-identical.
