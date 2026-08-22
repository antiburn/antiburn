# Claude characterization fixtures

These fixtures freeze the Claude JSONL shapes that the first Local Insights slice supports. The integration test reads them through the public normalization and analysis APIs.

**Every file here is synthetic.** Each file was authored by hand from format knowledge and the current parser. No real session, user, machine, organization, or repository is represented. The fictional user is `avery`. The fictional project is `/home/avery/projects/demo-app`. The `orbit-tracker` and `atlas-notes` skills are invented.

Format knowledge and Claude version history come from the private reference repository, file `crates/harness-kb/facts/claude.json`. That file is a versioned harness knowledge base. It contains per-version facts and capture provenance. It is not a transcript.

No captured provider session log ever enters this repository. Do not copy one. Do not redact one into a fixture. Redaction is not sufficient.

| File | Proves |
| --- | --- |
| `records_all_kinds.jsonl` | User and assistant records preserve usage, model, tool use, error tool results, thinking, and compaction. The fixture also provides positive initial-context and skill-description signals. |
| `timestamps_repeated_and_out_of_order.jsonl` | Repeated and non-monotonic timestamps produce stable duration, active time, and buckets. |
| `malformed_between_valid.jsonl` | The parser skips one malformed line and keeps valid records on both sides. CH-004 must also assert `Partial` coverage because the current API exposes no coverage value. |
| `incomplete_final_record.jsonl` | The parser does not commit the truncated final record. The next source generation can pick up the record after its terminating newline arrives. |
| `unrecognized_type.jsonl` | A `telemetry_ping` record without a role or message produces no event. A record with the same type and a recognized assistant role produces one event. Valid neighbours survive both records. |
| `parent_with_task_spawn.jsonl` | A parent transcript records a `Task` tool spawn without adding child events to the parent. |
| `subagent_child.jsonl` | The child source has its own session ID and metrics. An analysis call over both files does not count either source twice. |

The checked-in goldens serialize `NormalizedSession` and the complete `SessionMetrics` value for each fixture. The tests compare parsed JSON values, so a field addition fails until a reviewer accepts the golden change.

The large-source tests generate their JSONL in memory. They do not commit a large transcript-shaped blob.

## Claude capability and coverage matrix

CH-009 fills this matrix after all Claude evidence groups have complete coverage semantics.
