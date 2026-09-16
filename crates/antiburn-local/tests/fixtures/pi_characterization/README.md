# Synthetic Pi characterization fixtures

These fixtures are hand-authored synthetic Pi records. They come from
the pinned Pi V3 session contract and the reviewed example extension. They use
only invented values and contain no captured session data.

An accepted Pi V3 source starts with one `type: "session"` record with
`version: 3` and a valid timestamp. The reader rejects headerless, malformed,
unsupported-version, and duplicate-ID input before it can emit Pi evidence.
The `headerless_*` fixtures exist only to prove that rejection.

The adapter treats the top-level timestamp as authoritative. It accounts for
only the four disjoint usage buckets. It never reads or stores `customType`
payload values. Diagnostics can store only bounded native row, role, and
content-block discriminators when a structural check fails closed. Persisted
evidence and metrics do not retain transcript content, paths, or extension
payload values. They retain bounded session identity and provider/API/model
facts when those facts are required for evidence.

Pi supports request occupancy, cache writes when the selected API reports
them, timestamps, tool calls, model identity, token classes, thinking levels,
compaction boundaries, record identity, and thread identity. It does not claim
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
- `excess_cache_rehydration_finding.jsonl` pairs a model switch with paid cache writes on both turns, giving Excess Context Reprocessing a finding.
