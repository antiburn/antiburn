# Synthetic Pi characterization fixtures

These fixtures are hand-authored synthetic Pi records. They come from
aggregate structural observation of a local installation. That observation
records field names, JSON types, and structural invariants only. No captured
session value appears here.

The adapter treats the top-level timestamp as authoritative. It accounts for
only the four disjoint usage buckets. It never reads or stores `customType`
payload values. Diagnostics can store only bounded native row, role, and
content-block discriminators when a structural check fails closed. Persisted
evidence and metrics do not retain content, identifiers, paths, provider
metadata, or API metadata.

Pi supports request occupancy, cache writes when the selected API reports
them, timestamps, tool calls, model identity, token classes, thinking levels,
compaction boundaries, and thread identity. It does not claim tool catalogs,
MCP attribution, speed or service tiers, subagent links, quota events, or a
harness version.

Every entry after the `session` header carries a top-level `id` and
`parentId`. Exactly one entry per file has `parentId: null` — the thread
root. Every fixture below gives its rows a realistic `id` / `parentId` chain
unless the fixture's own purpose is to be malformed, in which case the
missing or broken chain is the point.
