# Claude native capture fixtures v1

Package version: `claude-native-capture-fixtures-v1`.
Capture contract: `antiburn-native-capture-v1`.

Every native file is synthetic and authored for these tests. No real provider
folder, captured transcript, user identity, credential, or private repository
content appears here. The version strings define test shapes, not a claim that
all historical releases produce this shape.

`manifest.json` pins each input and reference file with its byte length and
SHA-256. Consumers can pin this package by repository commit and the manifest's
SHA-256. `reference/main.capture.json` is the complete capture manifest for
`project/main.jsonl`; `reference/main.chunk-00000.gz` is its saved wire part.
The reference identity uses `fixture-main`, `synthetic-account`, and
`synthetic-destination`.

| Input | Purpose |
| --- | --- |
| `project/main.jsonl` | Exact standalone native JSONL with recorded historical version `2.1.309`. |
| `project/fork.jsonl` | An independent fork with direct native `forkedFrom.sessionId` and recorded version `2.1.278`. |
| `project/main/subagents/agent-worker.jsonl` and `.meta.json` | An independent child and its exact same-stem native companion. Unknown companion fields must survive. |
| `project/main/subagents/agent-fork.jsonl` and `.meta.json` | An independent fork with a mixed historical version set and a proven parent edge in its companion. |
| `project/unknown.jsonl` | Unknown historical version, malformed JSON, and a partial native record. Capture retains all bytes. |
| `project/audit.jsonl`, `project/unowned.txt` | Negative discovery controls. Neither enters a capture. |

See [the public contract](../../../docs/cloud/capture-v1.md) for validation and
the explicit reference regeneration command. Regenerate only after reviewing an
intentional byte-contract change.
