# Native capture v1

`antiburn-native-capture-v1` preserves exact native source streams for Cloud
processing. Cloud owns semantic parsing. The desktop does not turn its local
normalized session model into an upload payload. Local parsing and Burn Check
coverage remain governed by [session coverage](../session-coverage.md) and
[check coverage](../check-coverage.md).

This public contract is unreleased. The standalone
[`antiburn-cloud-capture`](../../crates/antiburn-cloud-capture) crate has no network
client. Its default feature set is empty. All capture code requires the explicit
non-default Rust feature `cloud`. The library does not enable desktop sync,
select a service URL, grant consent, or enable production.

## Source ownership and capture

Discovery accepts one explicitly authorized Claude `projects/<project>` directory.
It never finds a home directory. Tests read only public synthetic fixtures and
files they create in temporary directories. The host owns current source access
policy, selected folders, consent, and scheduling.

The host assigns one stable `source_scope_id` per account and device-local source
root, then calls `Candidate::in_source_scope` before capture. Replacing a directory
access grant must not change that scope. It is not a grant ID or a repository ID.
The library permits unscoped candidates for synthetic fixtures; a host must not
use unscoped keys across accounts or roots.

Discovery reads directory entries and metadata, not native content. It admits:

- Direct `<session-id>.jsonl` transcripts, excluding `audit.jsonl`.
- Independent `<session-id>/subagents/agent-<id>.jsonl` transcripts.
- Only an admitted child's exact same-stem `agent-<id>.meta.json` companion.

Each root, child, orphan child, or fork has its own capture, reservation, and retry
state. A parent capture does not merge child transcripts or fork replay sources.
A child can remain available when its parent's transcript is absent. Project
files, job state, audit logs, arbitrary JSON files, referenced attachments, and
paths named inside transcripts are outside this source scope.

Discovery keeps admitted directory handles for enumeration, child traversal,
companion checks, and later native file opens. Native opens use `O_NOFOLLOW`.
A renamed ancestor cannot redirect capture to another source. Symlinked sources
are unavailable. Native inputs must be regular files with one hard link.

Default bounds are 4,096 directory entries and 512 candidates. Discovery fails
when it exceeds a bound; it does not return a truncated list as complete.
Candidates retain at most one source-directory handle each and share handles
when possible.

Capture pins file identity, size, modification time, and change time before
reservation. It saves a `preparing` queue entry, then calls `Transport::reserve`
immediately before reading native content. It checks pinned files before and
after streaming. Mutation, reservation failure, or interruption leaves an
interrupted entry. It cannot resume native reads under the same capture ID.
Explicit cancellation and a new ID are required to capture a later generation.

Repeating `capture` with an existing identity returns the saved entry. New source
bytes or metadata sections cannot replace that pending capture. Native bytes
retain invalid JSON, original line endings, unknown fields, and an incomplete
final record. Capture does not repair or clip them.

## Independent stream layout

The manifest contains ordered streams. It does not contain a concatenated
transcript/companion/metadata byte stream.

| Stream ID | Role | Schema | Bytes |
| --- | --- | --- | --- |
| `transcript` | `native_transcript` | `claude-jsonl` | Exact native JSONL |
| `native-companion` | `native_companion` | `claude-agent-meta-json` | Exact owned companion, when present |
| `capture-metadata` | `capture_metadata` | `antiburn-capture-metadata-v1` | Compact UTF-8 metadata JSON |

Each stream records its stable ID, role, schema, full expanded byte length,
lowercase SHA-256, and ordered chunks. These schema names define framing, not a
claim that every historical native producer has the same semantic schema.
The generated metadata stream is always present and nonempty.

Chunk boundaries restart for every stream. Each non-final chunk expands to
exactly 8,388,608 bytes (8 MiB). Each final chunk expands to 1 through 8,388,608
bytes. A small companion or metadata sidecar has its own chunk. An empty native
stream has length zero, SHA-256 of the empty byte string, and zero chunks. It
does not create an empty gzip member.

Each chunk is one independent gzip member: level 6, modification time zero, OS
byte 255, and no filename or comment. The compression name is
`gzip-mtime0-os255-level6`. Each chunk records a zero-based index within its
stream, expanded offset, expanded and compressed lengths, and both digests.
`ChunkKey { stream_id, index }` identifies a chunk. All offsets are stream-local.

The lockfile and synthetic wire references pin this implementation's compressor
output. Resume sends saved compressed bytes. An upgrade must not recompress a
pending capture. No stream ID, source path, or payload field is an extraction
path.

The manifest digest is SHA-256 of compact JSON serialization of `CaptureManifest`
in its declared field order. Operation identity appears in this manifest, not in
the stable metadata stream.

## Stable metadata and source fingerprint

`CaptureMetadata` has schema version 1, capture version, agent `claude-code`,
source format `ClaudeJsonl`, native filename session and agent IDs, version
evidence, relationship, history completeness, and optional typed sections.
It has no capture ID, account, destination, automatic timestamp, or queue state.
These operation values must not change the source bytes for an unchanged
native source and unchanged meaningful metadata.

`metadata_fingerprint` is SHA-256 of compact, sorted-key metadata JSON after
removing only `unix_millis` from each typed `ObservedAt` observation envelope.
`CaptureMetadata::stable_fingerprint()` computes it. The projection retains
observation kind, source positions, producer versions, evidence, data, and all
unknown fields. A data field named `unix_millis` is not removed. The saved metadata
stream retains the actual observation time and its exact wire digest.

`source_fingerprint` hashes compact JSON with fields in this order:
`capture_version`, `metadata_fingerprint`, and `streams`. The last field contains
only ordered native stream descriptors, each with `stream_id`, `role`, `schema`,
`expanded_bytes`, and `expanded_sha256` in that order. It excludes operation
identity, compressed representation, and the physical metadata stream descriptor.
Consumers must verify the metadata fingerprint against expanded metadata and
verify every stream's wire digests separately.

A timestamp-only observation change does not mark a source changed. A meaningful
metadata change does, even when native JSONL is unchanged. The manifest digest
still binds the actual metadata stream bytes, including observation timestamps.
Freeze observation boundaries and every saved byte for retries.

Only an integrity-accepted entry exposes
`QueueEntry::accepted_source_fingerprint()`. The host may store this value in its
durable source state. It binds the stable fingerprint to the account,
destination, and device-local source key. `matches_source(manifest)` compares
those bindings while ignoring the new capture ID.

This comparison is not authorization or a cancellation operation. It never
deletes, replaces, or releases a pending capture. An unchanged-source check must
not discard pending or fenced retries. The host must retain immutable request
and consent state, honor current permissions and deletion state, and continue
reconciliation for saved captures. A prior fingerprint does not prove that
remote data remains available.

### Historical producer versions

`source_agent_version` comes only from records owned by the captured source.
It never comes from an installed executable or another agent's replayed record.
A root record needs its exact session ID, no child agent ID, and no true
sidechain marker. A child record needs its exact parent session ID and its
filename agent ID. Missing or different ownership fields do not become owned
version evidence.

When records name another owner and none names the filename owner, capture fails
with `InvalidSource`. A companion's fork edge cannot establish a new transcript
identity on its own. The interrupted entry remains private and cannot upload;
explicit cancellation releases its saved bytes. Empty or unknown-shape sources
can retain bytes without claiming an owned runtime version.

The scanner keeps at most 16 distinct version strings, plus their bounded
`owned`, `inherited`, or `unknown` observations. Here `inherited` means that a
record names another logical owner; it does not itself prove a fork edge.
`recorded_agent_versions` retains the observed set without attributing it to the
current producer.

The source version is non-null only when a complete bounded scan finds owned
records, every owned record has a valid recorded version, and all owned versions
agree. Otherwise it is JSON null. `owned_version_complete` and
`version_scan_complete` expose those separate limits. Each inspected JSONL record
is bounded to 64 KiB. Malformed, oversized, or incomplete records deny a complete
scan but remain intact in the native stream.

In particular, a fork can replay a worker's versioned records and then append an
owned record without a version. Its source version stays null. Version
observations do not establish complete retained history. History completeness
is always `unknown`.

### Relationships

`unknown` means that capture has no accepted direct relationship proof. `child`
names the parent source key from the exact subagents layout. A root `fork` needs
an owned native `forkedFrom.sessionId`. A child `fork` needs its exact companion's
`isFork: true` and unprefixed `parentAgentId`. Self-references and conflicting
owned fork evidence do not prove a fork edge. Similar text, timestamps, or
repository association do not establish lineage. A parent edge does not imply
that the parent was captured.

## Typed optional metadata sections

`sections` is a namespaced map with at most eight entries and 32 KiB of compact
JSON in total. Each section is bounded to 16 KiB and requires:

- A schema identifier and producer name and version.
- An observation: exact native source position, or observation time.
- One through eight evidence items.
- A JSON data value.

Unknown fields round-trip in section, producer, observation, evidence, and data
envelopes. They remain subject to the same size bounds. Reserved-field
collisions are invalid.

`SourcePosition` binds a stream ID and expanded byte offset strictly inside a
native stream. It requires `NativeStream` evidence with the exact stream ID and
full expanded digest. Evidence cannot bind to the generated metadata stream.
`ObservedAt` records positive Unix milliseconds and requires `LocalObservation`
evidence with the method and SHA-256 of compact JSON data. A local observation
does not prove a historical native fact.

The synthetic `antiburn.allowance-contribution` example binds declared
contribution data to a native stream position and digest. It is not a provider
request or a claim about a current allowance. Consumers must respect its schema,
source limits, and producer trust.

The synthetic `antiburn.capture-checkout` example records a checkout/worktree
snapshot at observation time. It explicitly does not identify the session's
historical checkout. The library performs no automatic Git, worktree, or provider
reads. A host that adds such sections must obtain appropriate consent and keep
their evidence and observation boundary frozen for retries.

Metadata can disclose private project associations, identifiers, provider usage,
or repository state. Keep sections minimal, use reviewed schemas, and omit
credentials, signed URLs, and unneeded local paths. Unknown fields are not
permission to collect more data. An accepted envelope proves consistent framing
and digest binding, not the truth of an arbitrary producer claim.

## Frozen negotiation and confirmed uploads

`Transport` exposes `reserve`, `negotiate`, `status`, `upload_part`, and `complete`.
The host owns authentication, consent fences, endpoint and redirect policy,
timeouts, bounded HTTP decoding, and scheduling. A destination ID is not a URL.
The host must preserve the verified account, destination, source, capture ID,
and consent binding across refresh and retries.

Reservation and completion are idempotent for the fixed capture ID.
`UploadPlan` binds the capture identity, manifest digest, and plan ID. It contains
one ordered `Upload` or `Reuse` decision for every stream chunk, with its exact
compressed digest. The plan separately declares required transfer bytes and
reused bytes. The library verifies these sums and saves the plan before upload.
Every later negotiation must return the exact same plan. A changed plan fails
closed without changing saved bytes or progress.

`ServerStatus` binds the same identity, manifest digest, and plan ID. Its
`uploaded_parts` confirm uploaded chunks only. A reuse decision is not an upload
receipt. Unknown keys, duplicates, reused chunks in uploaded receipts, wrong
hashes, or regressed confirmations fail closed. Confirmation never comes from
local bytes sent or a successful HTTP upload alone.

Each negotiated logical chunk can map to one remote object. Its complete saved
gzip body can be the single final multipart part for that object's upload.
Chunk indices are not multipart part numbers. Small compressed chunks do not
need artificial splitting to satisfy non-final multipart size limits.
The host must bind object/upload identifiers to the frozen plan and use
server-confirmed status after a lost response. Library transport tests do not
prove an HTTP adapter or hosted multipart behavior.

## Durable queue and bounded execution

Unix spool directories use mode `0700`; files use `0600`, belong to the effective
user, and have one hard link. The queue rejects shared modes and symlinks.
Operations stay relative to admitted directory handles, including iteration,
creation, record replacement, and deletion. An exclusive lock admits one
executor across processes. Other platforms return `UnsupportedPlatform`;
Windows compilation does not establish Windows capture support.

Chunks and queue records are synced before publication. Record updates use
write, file sync, atomic rename, and directory sync. Interrupted publication and
terminal deletion have distinct recoverable names. Fully saved captures become
`queued` and survive restart without reopening sources.

Default bounds are 512 MiB per transcript, 64 KiB per companion and generated
metadata stream, 1 GiB expanded capture size, 512 queue entries, and 2 GiB of
spool bytes. A record cannot exceed 512 KiB. Streaming reads use 64 KiB buffers.
Before native reads, capture reserves conservative compressed size and room for
two bounded queue records. Limits may be lowered within the supported bounds.

`resume_step` reconciles status, hashes at most one selected saved chunk, and
sends at most that chunk. It does not scan every saved part on each turn.
Completion uses a later step after required uploads are confirmed. The host can
round-robin pending captures and keep uncaptured selections in its own durable
state. `resume` is a synchronous draining helper, not a fair scheduler.

One spool permits one executor and at most one synchronous part upload at a
time. The host must also bound its root list, discovery workers, and pending
selections. Per-root discovery limits are not an aggregate limit across roots.
Run these synchronous calls on a native worker, not the UI thread.

Progress separates total compressed bytes, required transfer bytes, reused
bytes, and server-confirmed upload bytes. Before negotiation,
`negotiated: false` means the transfer/reuse split is unknown. These counters do
not establish integrity acceptance. Plan and confirmed status survive restart.

Completion intent is durable before submission. A lost completion response may
repeat only the same idempotent request. Once submission succeeds, later steps
poll its status rather than resubmitting it. HTTP 202, logout, auth loss,
executor stop, or process exit never release bytes. Only status with exact
bindings, all required upload receipts, and `integrity_accepted: true` permits
automatic release. Every reused chunk remains local until this same acceptance.
Explicit capture cancellation is the only other byte-release path.

After integrity acceptance, the spool releases chunk files but keeps the bounded
accepted receipt, manifest, plan, and final status. Restart retains this receipt.
The host must durably save its accepted fingerprint and terminal request state,
then call `acknowledge_accepted`. Only that call removes the accepted queue entry.
`cancel` rejects an accepted entry with `AlreadyAccepted`; acknowledgment rejects
a pending entry with `NotAccepted`.

The host generates globally unique capture IDs, never reuses terminal IDs, and
keeps terminal request and accepted-source state durably. The spool is not an
unbounded ID registry. Local cancellation does not delete remote content.

Transport errors use closed values. Filesystem errors expose only their error
kind. Content, credentials, paths, signed URLs, and raw service errors must not
enter logs, analytics, or renderer status.

## Synthetic fixtures and checks

[`fixtures/cloud/capture-v1`](../../fixtures/cloud/capture-v1) is the public
`claude-native-capture-fixtures-v1` package. Its manifest pins each native input,
typed section example, and independent reference stream part by length and
SHA-256. These synthetic shapes do not certify every historical Claude release.

```sh
cargo fmt --manifest-path crates/antiburn-cloud-capture/Cargo.toml --check
cargo clippy --manifest-path crates/antiburn-cloud-capture/Cargo.toml --all-targets --features cloud --locked -- -D warnings
cargo nextest run --manifest-path crates/antiburn-cloud-capture/Cargo.toml --features cloud --locked
```

The explicit reference writer reads only this synthetic package:

```sh
cargo run --manifest-path crates/antiburn-cloud-capture/Cargo.toml --features cloud --locked --example write_fixture
```
