# Synthetic Pi characterization fixtures

These fixtures are hand-authored synthetic Pi records. They come from
the pinned Pi session contract, its V1/V2 migrations, and the reviewed example
extension. They use only invented values and contain no captured session data.

An accepted source starts with one `type: "session"` record and a valid
timestamp. The reader accepts the official V1 form with no `version`, V2, and
V3. It applies Pi's documented V1 linear-ID and V2 `hookMessage` migrations
in memory. It rejects headerless, malformed, unsupported-version, and
duplicate-ID input before it can emit Pi evidence. The `headerless_*` fixtures
exist only to prove that rejection.

The adapter treats the top-level timestamp as authoritative. It accounts for
only the four disjoint usage buckets. It never reads or stores `customType`
payload values. Diagnostics can store only bounded native row, role, and
content-block discriminators when a structural check fails closed. Persisted
evidence and metrics do not retain transcript content, paths, or extension
payload values. They retain bounded session identity and provider/API/model
facts when those facts are required for evidence.

Pi supports request occupancy, cache writes when the selected API reports
them, timestamps, tool calls, model identity, token classes, thinking levels,
compaction boundaries, record identity, and thread identity. Cache findings
also require a recovered miss episode on one reviewed route. It does not claim
tool catalogs, MCP attribution, speed or service tiers, quota events, or a
harness version. The reviewed example extension can provide finding-only
subagent evidence. Arbitrary extensions remain fail closed.

Every entry after the `session` header carries a top-level `id` and
`parentId`. Exactly one entry per file has `parentId: null` — the thread
root. Every fixture below gives its rows a realistic `id` / `parentId` chain
unless the fixture's own purpose is to be malformed, in which case the
missing or broken chain is the point.

- `session_overdepth_finding.jsonl` reports one turn's input tokens above the Sessions Over Depth cap, giving that badge a finding.
- `model_overthinking_finding.jsonl` sets `thinkingLevel` to `max`, giving Model Overthinking a finding.
- `excess_cache_rehydration_finding.jsonl` preserves an older model-switch and cache-write shape; it does not establish a current cache-rehydration finding.
- `cache_continuous_transient_miss.jsonl` records a same-route hit, miss, and recovery during continuous activity; accounting remains visible without a rehydration finding.
- `overthinking_aborted_zero_usage_content.jsonl` carries content and an above-cap effort setting on an aborted message with zero usage; it must not establish an effort finding.
