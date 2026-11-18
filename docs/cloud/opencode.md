# OpenCode desktop capture

Antiburn Cloud is not live. [Sign up for the waiting list](https://antiburn.com/cloud/).
This contract describes development-only capture; public builds keep Cloud disabled.

OpenCode capture preserves session-owned SQLite values for later document
projection. It does not use normalized metrics, start OpenCode, read external
attachment files, or download URLs. See [desktop capture](capture.md) for consent,
private spooling, fixed chunks, and saved retries, and
[SQLite native format](sqlite-native.md) for the row stream.

The explicit non-default `cloud` feature enables this library path. Capture is
Unix-only. The library has no network client and does not enable desktop uploads.

## Source selection

The host authorizes an exact database path before calling `discover_opencode`.
Discovery does not search home directories or infer source permission from a
recorded project path. It reads schema and bounded session IDs, not message,
event, instruction, job, or media payloads.

Each native ID is an independent source, including children, detached children,
forks, inactive sessions, and archived sessions. Discovery does not apply a
recency filter or combine children with a root. An exceeded discovery bound is
an error, not a complete-looking partial list.

Native keys contain `opencode`, the database filename, and the exact session ID.
Filename and ID components use unpadded URL-safe base64 to prevent ambiguous path
separators. The host adds its stable account and authorized-root scope. The
database file's inode is an admission fence, not a logical session key.

Discovery and capture are synchronous blocking operations. Hosts must run them
on bounded blocking workers, not UI or async runtime threads. A saved desktop-host
source request is not captured payload or permission to execute a later read.

Project permission remains separate from database access. The optional
`Candidate::project_evidence()` lookup reads only the exact session's bounded
`session_v2.directory` value and schema. It does not inspect messages, tool paths,
project tables, or another session. The value must be a valid absolute path of at
most 4,096 UTF-8 bytes. A missing column, null, wrong SQLite type, invalid path, or
oversized value leaves the project unknown.

Use `ProjectSelection::admit` to filter by account-approved project/CWD folders
and descendants. It matches any proved owned CWD in the source evidence.
Sibling path prefixes do not match. A database path or provider
root never grants project permission. Children and forks need their own recorded
CWD; parent and fork links grant nothing. Worktrees outside the selected folder
need separate selection. See [project folder selection](capture.md#project-folder-selection)
for bounded Claude lookup and directory alias rules.

`reopen_opencode` admits one saved native ID without enumerating other sessions.
The host must re-authorize the database, add its stable source scope, and apply
current project permission before new capture. Capture freezes that approved root
policy and checks it in the native export snapshot, after reservation. A changed
directory still matches when it remains in an approved folder tree. An unknown
or nonmatching CWD cannot publish an uploadable capture.
The native stream still contains the full selected session, including stored
content that names files outside the selected folder. Selection does not change
SQLite types, native bytes, stream versions, or saved retry bytes.

Local metadata reads are separate from full capture and ongoing upload consent.
An authorized local Claude metadata scan can read bounded transcript content
before reservation to find all recorded owned CWDs. No metadata scan uploads
content. The host owns bounded metadata caches and project/worktree grouping;
this library performs no Git reads or background discovery.

## Structural V2 support

Capture selects by proved schema capabilities, not an installed release
allowlist. Its required shape is `session_v2` and `session_message`, with native
session/message keys, exact session ownership, message type, and unique native
session sequence. Extra ordinary columns remain source, including unknown
columns and their original SQLite value types.

Known optional tables enter capture when present and valid. Absence is different
from a present malformed table. Unknown record kinds and unknown JSON fields
remain opaque source; their text is not parsed and reserialized.

Unknown session-linked tables stop capture with `UnsupportedSource`. Detection
uses schema names, apparent ownership columns, and declared links into known
source tables. Detection does not grant permission to export those tables.
Schedules and retained V1 `session`, `message`, and `part` tables are explicit
exclusions. Capture never falls back to them.

This supports additive changes within the selection contract. It does not
promise lossless support for an unknown future storage redesign. Views, virtual
tables, generated columns, and selected `WITHOUT ROWID` tables are unsupported.
A column named `rowid`, in any letter case, is also unsupported in a known source
table. It would hide the physical row locator used by selection and BLOB reads.
Other ordinary columns, including `_rowid_` and `oid`, remain supported when
`rowid` is unshadowed.
Reference-bearing JSON uses the characterized UTF-8 database encoding; a
converted database with a different encoding fails instead of reinterpreting
its dependency values.

## Session-owned rows

Every comparison that assigns source to a session uses binary equality.
Selection retains all columns of each selected row.

| Table | Scope | Stable order |
| --- | --- | --- |
| `session_v2` | Exact selected `id` | `id` |
| `session_message` | Exact `session_id` | `seq`, `id` |
| `session_part` | Exact `session_id`; its message must belong to the same session | `message_id`, `ordinal` |
| `event_sequence` | Exact `aggregate_id` | `aggregate_id` |
| `event` | Exact `aggregate_id` | `seq`, `id` |
| `session_inbox` | Exact `session_id` | `enqueued_seq`, `id` |
| `session_pending` | Earlier V2 pending table, exact `session_id` | `admitted_seq`, `id` |
| `instruction_entry` | Exact `session_id`, including removed entries | `key` |
| `instruction_state` | Exact `session_id` | `session_id` |
| `job` | Exact owner `session_id`, including output, errors, and child references | `notification_id` |
| `blob_ref` | Exact `session_id`, including retained references to absent messages | `seq`, `message_id`, `hash` |
| `instruction_blob` | Only proved instruction hashes from selected source | `hash` |
| `blob` | Only proved in-store media hashes from selected source | `hash` |

The archive keeps inline assistant content and split `session_part` rows without
merging them. Native ordering fields let a later reader reconstruct the supported
message view. Table order is not a universal event timeline.

Stored event types include their native schema version, such as
`session.instructions.updated.2`. Reference selection recognizes that suffix
without changing the exported cell or calling it an agent runtime version.

An owned job can contain a child summary. Its `child_session_id` does not make
that row child-owned or grant access to the child's transcript. Schedules,
credentials, account state, global settings, unrelated jobs, and unrelated
content-addressed rows are excluded.

## References and relationships

Reference extraction reads defined fields through bounded buffers. It does not
build whole JSON trees or retain large message text, keys, or data URIs.

Instruction hashes come from:

- `instruction_state.initial_values` and `current_values`.
- Retained `session.instructions.updated` event deltas, including older values
  that no longer appear in current state.
- A selected fork event's recorded instruction baseline.

Removed instruction entries retain their native values. Their contents are not
treated as arbitrary references.

Media hashes come from selected `blob_ref` rows and typed file fields in user
messages, assistant tools, split tools, queued inputs, and retained tool,
content-update, and inbox events. Event reference fields must name the selected
session. Only a tool's terminal `completed` or `error` content proves a tool file
reference. Arbitrary strings, metadata, paths, MIME labels, and hash-like text
are not reference authority.

The extractor preserves native JSON bytes separately. It can skip large text
and opaque provider state without copying them into strings. Invalid known
dependency structure, duplicate reference-bearing fields, or contradictory
ownership is an error. An unknown opaque record remains native source.

A proved reference whose native target row is absent remains unresolved. Capture
does not invent content or fetch it elsewhere. A present large target is subject
to limits; it is never silently omitted.

Each selected `blob_ref.message_id` is checked against existing messages, inbox
items, and earlier pending inputs. An existing target must belong to the selected
session. A genuinely absent target remains valid retained source. Contradictory
ownership fails before the exporter emits bytes.

`parent_id` and `fork_session_id` come from the capture snapshot. A parent can be
absent. Self references or conflicting direct relationships fail. Relationships
do not assign owners, grant access, or combine captures. Capture never reads a
parent's messages to fill child or fork content.

## In-store media and external attachments

Proved in-store BLOB bytes are native source. OpenCode can move user files and
tool file results from inline JSON into its `blob` table. Such files can contain
text. Capture therefore does not filter BLOBs by MIME type.

External attachment files, filesystem paths, HTTP URLs, and arbitrary linked
files remain outside capture. Consent must distinguish these extra reads from
content already stored in the authorized native database.

## Runtime version

The native `session_v2.version` cell stays intact. It does not prove the runtime
that produced every message: creation records a version, forks can inherit it,
and imports record the importing runtime.

Captured historical runtime version remains `null`. OpenCode capture does not
run an executable, inspect another session, or apply Claude's version-scanning
rules to fill that value. History completeness remains unknown.

## Snapshot, access, and limits

Capture starts only after the spool saves `Preparing` and obtains its
reservation. It opens a read-only, private-cache SQLite connection and uses one
transaction for schema, relationships, ownership, dependencies, and values.
All BLOB handles, the transaction, and the connection close before export
returns to spool publication or transfer.

Retained directory handles and recorded device/inode identity pin the source
path. Metadata checks use no-follow directory-relative lookups without opening
the database or its sidecars. Closing a raw database or SHM descriptor could
clear SQLite's process-wide POSIX locks, including another host connection's
locks. The provider therefore retains directory handles only.

SQLite opens the normal filename with no-follow protection so WAL and SHM keep
their correct context. Checks verify both the admitted path and SQLite's
open-file identity.
Main-database replacement fails even when a replacement is later removed.
Normal WAL commits and checkpoints do not fail merely because timestamps change.

SQLite reads committed WAL content from its snapshot. Concurrent later writes
remain outside that capture. Read-only WAL access can involve shared-memory
coordination; it is not a promise of zero filesystem activity. Capture never
uses `immutable=1`, changes journal mode, checkpoints, migrates, repairs, attaches
databases, enables extensions, or executes source DDL.

| Bound | Limit |
| --- | --- |
| Discovery sessions | Configured limit; 512 by default |
| Ancestor depth | 64 |
| Native IDs and database filename | 128 UTF-8 bytes |
| Selected rows across all tables | 100,000 |
| Inspected schema metadata | 1 MiB |
| Columns per table and inspected indexes per key check | 256 |
| Distinct media and instruction hashes combined | 16,384 |
| Keys per instruction map | 16,384 |
| Reference JSON nesting | 128 |
| One referenced media cell | 64 MiB |
| All referenced media cells | 256 MiB, also within `source_bytes` |
| SQLite busy wait | 250 ms |
| Discovery or capture deadline | 30 seconds |

The expanded native stream, including JSONL framing and base64 overhead, must fit
`Limits.source_bytes`. Capture and spool totals still apply. SQLite cache sizing
is a target, not a hard process-memory guarantee.
Text-size guards use SQLite's `octet_length`, available in the pinned bundled
SQLite 3.46.0. It reads the stored byte count before returning native text rather
than casting an oversized cell to a BLOB.

`Busy`, `Limit`, `InvalidSource`, `UnsupportedSource`, and `SourceChanged` are
closed errors. Diagnostics contain no native values, paths, or raw SQLite
messages. An error cannot publish a clipped capture. Interrupted bytes remain
private under the spool's normal recovery rules.

For a temporary lock or deadline, retry with a new capture after explicitly
cancelling the interrupted one. For unsupported schema, keep the native store
unchanged and use a build with an explicit supported selection rule. Queued
captures resume from saved bytes without reopening SQLite.

## Public source evidence

The [synthetic fixtures](../../fixtures/cloud/opencode/README.md) characterize
selection and fidelity boundaries. They are not live certification or a claim
about every historical producer.

- OpenCode V2 `v2.0.32`, commit
  [`843a6c514e606b436a7fe713422ace0a93ad3d61`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/core/src/session/sql.ts):
  earlier session/message layout, pending inputs, and instruction storage.
- OpenCode `v2.3.0`, commit
  [`a6b15d33daf6ab7d35569f6d711fdb630b9f04e7`](https://github.com/anomalyco/opencode/blob/a6b15d33daf6ab7d35569f6d711fdb630b9f04e7/packages/core/src/session/sql.ts):
  split parts, inbox, instruction storage, and in-store media.
- The same current pin's
  [history reader](https://github.com/anomalyco/opencode/blob/a6b15d33daf6ab7d35569f6d711fdb630b9f04e7/packages/core/src/session/history.ts#L90-L123),
  [blob rules](https://github.com/anomalyco/opencode/blob/a6b15d33daf6ab7d35569f6d711fdb630b9f04e7/packages/core/src/session/blob.ts),
  [event schema](https://github.com/anomalyco/opencode/blob/a6b15d33daf6ab7d35569f6d711fdb630b9f04e7/packages/schema/src/session-event.ts),
  [versioned event storage](https://github.com/anomalyco/opencode/blob/a6b15d33daf6ab7d35569f6d711fdb630b9f04e7/packages/core/src/bus.ts#L417-L428),
  and [job ownership](https://github.com/anomalyco/opencode/blob/a6b15d33daf6ab7d35569f6d711fdb630b9f04e7/packages/core/src/job.ts#L191-L193)
  define the selected reference locations and ownership rules.

Focused checks:

```sh
cargo nextest run --manifest-path crates/antiburn-cloud-capture/Cargo.toml --features cloud --locked --lib -E 'test(opencode::)'
cargo nextest run --manifest-path crates/antiburn-cloud-capture/Cargo.toml --features cloud --locked --test opencode_capture
```
