# Claude native capture fixtures v1

Package version: `claude-native-capture-fixtures-v1`.
Capture contract version: `1`. Manifest and metadata versions are integers.

Every native file is synthetic and authored for these tests. No real provider
folder, captured transcript, user identity, credential, or private repository
content appears here. The version strings define test shapes, not a claim that
all historical releases produce this shape.

`manifest.json` pins each input and reference file with its byte length and
SHA-256. Consumers can pin this package by repository commit and the manifest's
SHA-256. `reference/main.capture.json` is the complete capture manifest for
`project/main.jsonl` with the typed examples from `sections.json`.
`reference/main.transcript.chunk-00000.gz` and
`reference/main.capture-metadata.chunk-00000.gz` are independent saved stream
parts. The metadata stream contains no operation identity.
The reference identity uses `fixture-main`, `synthetic-account`, and
`synthetic-destination`.

| Input | Purpose |
| --- | --- |
| `project/main.jsonl` | Exact standalone native JSONL with recorded historical version `2.1.309`. |
| `project/fork.jsonl` | An independent fork with direct native `forkedFrom.sessionId` and recorded version `2.1.278`. |
| `project/main/subagents/agent-worker.jsonl` and `.meta.json` | An independent child and its exact same-stem native companion. Unknown companion fields must survive. |
| `project/main/subagents/agent-fork.jsonl` and `.meta.json` | An independent fork with a mixed historical version set and a proven parent edge in its companion. |
| `project/main/subagents/agent-inherited-only.jsonl` and `.meta.json` | A fork replays a worker's versioned record and appends its own unversioned record. The fork's source version stays null. |
| `project/main/subagents/agent-replay-only.jsonl` and `.meta.json` | Negative ownership control. Only another worker's record exists. The companion alone cannot establish a new capture identity. |
| `project/main/subagents/agent-conflicting-fork.jsonl` and `.meta.json` | Conflicting owned native fork fields deny a fork relationship, even when the companion asserts a parent. All bytes remain available. |
| `project/unknown.jsonl` | Unknown historical version, malformed JSON, and a partial native record. Capture retains all bytes. |
| `project/audit.jsonl`, `project/unowned.txt` | Negative discovery controls. Neither enters a capture. |
| `sections.json` | A native-bound synthetic allowance contribution and a capture-time synthetic checkout/worktree observation. Both include schema, producer, observation, and required evidence. |

The allowance example does not report a current provider allowance. The checkout
example describes observation time, not the session's historical checkout.
Neither example needs a real provider or Git read. All labels are synthetic.
Unknown section fields must survive serialization.

See [the public contract](../../../docs/cloud/capture.md) for validation and
the explicit reference regeneration command. Regenerate only after reviewing an
intentional byte-contract change.
