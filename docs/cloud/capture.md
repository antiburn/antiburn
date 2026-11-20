# Desktop capture

Antiburn Cloud is waitlisted. [Sign up for the waiting list](https://antiburn.com/cloud/).
This contract describes development-only capture; public builds keep Cloud disabled.

Desktop capture preserves provider-native source without using the local normalized session model.
Claude, Codex, and Pi streams retain exact file bytes. OpenCode streams retain typed
session-owned SQLite cells from one read snapshot, not a database copy.
Here native refers only to the provider's original session formats, bytes,
schemas, and recorded identities, not the desktop capture operation.

This public contract is unreleased. The standalone
[`antiburn-cloud-capture`](../../crates/antiburn-cloud-capture) crate has no network
client. Its default feature set is empty. All capture code requires the explicit
non-default Rust feature `cloud`. The library does not enable desktop sync,
select a service URL, grant consent, or enable production.

`CaptureManifest.version` and `CaptureMetadata.version` are the integer `1`.
Strings, missing versions, and unsupported numbers are invalid. The synthetic
fixture package has its own identity, separate from this numeric format version.

## Capture and local analysis

`antiburn-cloud-capture` owns bounded native source discovery and capture,
private disk spooling, and upload reconciliation through a host-supplied
`Transport`. It does not depend on `antiburn-local` or calculate local session
metrics, cost estimates, or Burn Check results.

[`antiburn-local`](../../crates/antiburn-local) owns local session discovery,
parsing, and analysis. Its parsed evidence and metrics support the desktop's local
session views, API-equivalent cost estimates, and Burn Checks. A capture package
contains native source streams and capture metadata, not those local results.
Cloud owns semantic parsing of uploaded source.
Successful capture does not prove local parser or Check support. Local parsing
and Burn Check coverage remain governed by [session coverage](../session-coverage.md)
and [check coverage](../check-coverage.md).

The desktop host must obtain Cloud content consent separately from local analysis
and model-provider requests. Local discovery or a configured check provider does
not authorize capture. Cloud content consent does not authorize model-provider
requests. Enabling this library does not connect the desktop to a capture or
upload transport; the desktop host owns that integration.

## Source ownership and capture

Discovery accepts one explicitly authorized source: a Claude `projects/<project>`
directory, a Codex rollout file, a Pi session file, or an OpenCode database.
It never finds a home directory. Tests read only public synthetic fixtures and
files they create in temporary directories. The host owns current source access
policy, selected sources, consent, and scheduling.

The host assigns one stable `source_scope_id` per account and device-local source
root, then calls `Candidate::in_source_scope` before capture. Replacing a directory
access grant must not change that scope. It is not a grant ID or a repository ID.
The library permits unscoped candidates for synthetic fixtures; a host must not
use unscoped keys across accounts or roots.

`Candidate::source_kind()` identifies the source contract. `source_path()` is a
display locator for the transcript or database; many OpenCode sessions share one
path. Use `native_session_id()` and `source_id()` to identify selections, not that
path alone. The host scopes candidates before assigning capture identities.

`discover_claude_page` returns bounded pages in native source-key order, including
independent child transcripts. Each call checks the bounded directory listing and
retains one page plus one candidate. Bind cursors to the same authorized root and
policy. Restart a sweep to find new entries before its cursor.

After current full-content consent and project admission, the host can use
`Candidate::opencode_content_digest(limits, selection)` to check an approved
session for changes. The required `ProjectSelection` contains the host's approved
folder trees. The export rechecks the recorded CWD in its read snapshot before
reading session payload or dependencies. A nonmatching CWD returns `SourceChanged`;
cached metadata cannot authorize the read, and no unrestricted fallback exists.
The host checks current account and content-consent authority immediately before
calling. The method hashes the same bounded native-row stream as capture,
including proved owned dependencies, without saving or uploading bytes. Unrelated
database writes do not change this digest. The check creates no reservation or
capture; it does not replace capture's fresh snapshot or integrity checks.

### Project folder selection

Source access and project permission are separate. A provider storage directory
or database can contain sessions for excluded projects. Granting source access
does not permit upload of every session in that store.

After the host obtains local source access, `Candidate::project_evidence()`
streams recorded project/CWD evidence. This local lookup can read session content
before reservation. It can read the full bounded Claude transcript, not just a
prefix, so it can find CWD changes late in a session. It never spools or uploads
content, reads companions or external files, or grants ongoing upload consent.
Full capture still requires reservation first. The host must separately obtain
and enforce current account-bound full-content upload consent.

Deleted recorded CWDs do not match a selected tree. Another existing owned CWD
can still admit the session. Missing approved roots, aliases, and permission
failures remain errors rather than implicit permission.

Claude reads through the admitted directory handle and keeps at most one
`PROJECT_RECORD_BYTES` (8 MiB) record and 256 distinct CWD paths of at most
4,096 UTF-8 bytes. It streams up to `Limits.source_bytes` (512 MiB by default)
with a 30-second deadline. `project_evidence_with_limits` permits lower byte
and schema bounds. An exceeded byte, path-count, or time bound returns `Limit`,
not a complete-looking partial set. It inspects source-owned top-level `cwd`
and discards message fields.
Reads use 64 KiB buffers. Large tool results below the record bound do not hide
other CWDs or prevent admission. A record above 8 MiB still makes the scan
incomplete and blocks folder-selected capture; the library does not clip it.
An owned Claude root needs its exact filename session ID, no child agent ID,
and no true sidechain marker. A child needs its exact parent session ID and
filename agent ID. Replayed records cannot supply the current source's CWD.
OpenCode reads only bounded schema and the exact session's
`session_v2.directory` cell, not messages or other payload tables.
That cell supplies current recorded metadata, not a complete historical CWD list.
Codex and Pi use only the recorded CWD from the bounded first owned header.

`ProjectEvidence::project_paths()` returns sorted distinct proved paths.
Multiple owned CWDs are valid; they do not conflict. Missing or invalid CWDs
grant nothing. For Claude, malformed, oversized, unfinished, or ambiguous
unowned records make `scan_complete()` false. Such a result can help local
discovery, but cannot admit capture.
The encoded Claude directory name, provider storage path, project identifier,
parent or fork links, tool CWD, and touched file paths never supply permission.
The scan does not infer a repository root or perform Git reads.

The desktop host owns metadata caching, source-change checks, and eviction.
Claude's `source_digest()` reports the full streamed transcript SHA-256 as a
local cache value, not permission, freshness proof, or a capture fingerprint.
Codex, Pi, and OpenCode return no whole-source digest because their lookups do not
hash the full source. The library owns no global cache or background metadata service.

`ProjectSelection::new` accepts up to 100 explicitly approved project folders.
`matches` includes the selected path and descendants by path component:
`/work/app` includes `/work/app/src`, but not `/work/app-other`. Paths must be
existing absolute directories without lexical aliases or symlink components.
Unavailable or aliased paths fail closed. A separate worktree outside the
selected tree needs its own folder selection.

`ProjectSelection::admit` returns an independent candidate when any proved owned
CWD matches an approved folder tree. It freezes the approved root policy, not one
exact CWD. `admitted_project_paths()` returns all initially matched CWDs.
The host owns project/worktree grouping and must keep each approved worktree path
in that policy. A group label does not merge captures or grant access to another
path. The single-path getters return the first path only; use the plural getters
for discovery and grouping.
The host binds folder choices to the current account, destination, and explicit
full-content consent. Empty selections permit nothing.

Capture rechecks the frozen approved root policy after reservation. OpenCode
checks the same snapshot used for payload export. Claude checks the full stream
as it captures. Codex and Pi recheck the first header in the pinned file before
spooling its bytes. CWD changes remain valid when any proved owned CWD still matches
the approved trees. If none matches, or the full check is incomplete, capture
cannot publish uploadable bytes. Preserve the source and inspect it locally;
do not weaken folder permission or clip the transcript to retry.
Sources without proved owned CWDs remain unavailable for folder-selected capture.

A matched session uploads in full under the existing source contract. Folder
selection does not remove messages, tool results, or file text that names paths
outside that folder. It also retains records from other CWDs in that session.
External files remain outside capture.

`reopen_claude` and `reopen_opencode` re-admit one saved native locator without
enumerating other sessions or reading payload. They do not restore a saved
project permission. The host must authorize source access again, retain its
stable account/source scope, and admit the reopened candidate against current
folder choices before a new capture. All these calls block and need a bounded
blocking worker. Saved upload retries continue from immutable spool bytes.

### Claude files

Discovery reads directory entries and metadata, not native content. It admits:

- Direct `<session-id>.jsonl` transcripts, excluding `audit.jsonl`.
- Independent `<session-id>/subagents/agent-<id>.jsonl` transcripts.
- Only an admitted child's exact same-stem `agent-<id>.meta.json` companion.

Non-UTF-8 filenames cannot name admitted sources. Discovery ignores them but
counts them toward its directory-entry bound. Spool names remain strict UTF-8.

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
interrupted entry, except for the proved capacity refusal described below.
Other failures cannot resume native reads under the same capture ID.
Explicit cancellation and a new ID are required to capture a later generation.

Repeating `capture` with an existing identity returns the saved entry. New source
bytes or metadata sections cannot replace that pending capture. Native bytes
retain invalid JSON, original line endings, unknown fields, and an incomplete
final record. Capture does not repair or clip them.

### Codex rollout files

[`reopen_codex`](codex.md) admits an explicitly authorized rollout path with an
expected native session ID. The bounded first `session_meta` header must prove
that ID. Later copied headers retain their bytes but do not supply authority.
The library exports the exact rollout with separate capture metadata. It does
not discover rollouts for the desktop host or enable Codex uploads in the app.

### Pi session files

[`reopen_pi`](pi.md) admits one explicitly authorized Pi JSONL file. The exact
filename suffix and bounded first `session` header must match the expected
native session ID. The library exports the whole file, including recorded
branches and tool results, without assigning worker IDs or following parent
references.

### OpenCode SQLite

`discover_opencode` reads bounded schema and session identities, not session
payload. It returns independent root, child, and fork candidates from the
OpenCode V2 `session_v2` source contract. V1 stores do not select a fallback.
Native keys include the provider, database namespace, and session ID before the
host's stable source scope is added.

After reservation succeeds, capture opens a read-only snapshot and exports the
selected session's owned rows and proved native dependencies. Ordinary additive
columns retain their typed values. Known optional tables, native blob data,
structural limits, and unsafe-source rejection are defined in the
[OpenCode source guide](opencode.md). Capture does not copy database or WAL files,
read another session's content through a parent edge, follow external file paths,
or fetch URLs.

The [SQLite native format](sqlite-native.md) preserves table, column, key, order,
and cell information without exporting DDL or indexes. It retains integer values,
real-value bits, text bytes, and blob bytes with bounded fragments. Base64 and
envelope overhead count toward the expanded source bound. The connection closes
before capture returns.

OpenCode reserves `limits.source_bytes + METADATA_BYTES` expanded bytes before
payload reads, even when the selected source is smaller. Admission also reserves
the conservative compressed bound and both queue record slots. The exporter and
spool sink both enforce the expanded bound; compressed writes remain bounded.
Failures leave an interrupted entry, not a smaller successful capture.

## Independent stream layout

Claude captures contain transcript, optional companion, and metadata streams in
that order. Codex and Pi captures contain transcript and metadata only. OpenCode
captures contain native rows and metadata only. These are ordered independent
streams, not one concatenated upload body.

| Stream ID | Role | Schema | Bytes |
| --- | --- | --- | --- |
| `transcript` | `native_transcript` | `claude-jsonl` | Exact native JSONL |
| `transcript` | `native_transcript` | `codex-jsonl` | Exact native Codex rollout JSONL |
| `transcript` | `native_transcript` | `pi-jsonl` | Exact native Pi session JSONL |
| `native-companion` | `native_companion` | `claude-agent-meta-json` | Exact owned companion, when present |
| `native-rows` | `native_rows` | `antiburn-sqlite-native` | Typed OpenCode SQLite cells; numeric format version inside its header |
| `capture-metadata` | `capture_metadata` | `antiburn-capture-metadata-v1` | Compact UTF-8 metadata JSON |

Each stream records its stable ID, role, schema, full expanded byte length,
lowercase SHA-256, and ordered chunks. These schema names define framing, not a
claim that every historical native producer has the same semantic schema.
The generated metadata stream is always present and nonempty.

Chunk boundaries restart for every stream. Each non-final chunk expands to
exactly 8,388,608 bytes (8 MiB). Each final chunk expands to 1 through 8,388,608
bytes. A small companion or metadata sidecar has its own chunk. An empty Claude
file stream has length zero, SHA-256 of the empty byte string, and zero chunks.
It does not create an empty gzip member. An OpenCode stream is nonempty and
includes the SQLite native header and end record.

Each chunk is one independent gzip member: level 6, modification time zero, OS
byte 255, and no filename or comment. The compression name is
`gzip-mtime0-os255-level6`. Each chunk records a zero-based index within its
stream, expanded and compressed lengths, and both digests.
`ChunkKey { stream_id, index }` identifies a chunk. The expanded offset is
`index * 8,388,608` after validating consecutive indices, full non-final chunks,
and the stream's total expanded length. Offsets are stream-local and are not
serialized on chunks.

The lockfile and synthetic wire references pin this implementation's compressor
output. Resume sends saved compressed bytes. An upgrade must not recompress a
pending capture. No stream ID, source path, or payload field is an extraction
path.

The manifest digest is SHA-256 of compact JSON serialization of `CaptureManifest`
in its declared field order. Operation identity appears in this manifest, not in
the stable metadata stream.

## Stable metadata and source fingerprint

`CaptureMetadata` has integer `version: 1`, native identity, provider version
evidence, relationship, history completeness, and optional typed sections.
Claude uses agent `claude-code`, source format `ClaudeJsonl`, and native filename
session and child agent IDs. OpenCode uses agent `opencode`, source format
`OpenCodeSqliteV2`, and its native session ID. OpenCode personas do not set
`native_agent_id`.
Codex uses agent `codex`, source format `CodexJsonl`, and its first owned header's
native session ID. Codex leaves `native_agent_id` null.
Pi uses agent `pi`, source format `PiJsonl`, and its first owned header's native
session ID. Pi leaves `native_agent_id` null.
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
`version` with integer value `1`, `metadata_fingerprint`, and `streams`. The last
field contains only ordered native stream descriptors, each with `stream_id`, `role`, `schema`,
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

### Claude historical producer versions

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
`version_observations` retains these values and their ownership without
attributing inherited or unknown versions to the current producer. Consumers
can derive a distinct version set from these observations.

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

### OpenCode historical producer versions

`source_agent_version` is null. A native session creation or import version does
not prove the runtime that produced its full history. Those cells remain in the
native row stream. `version_observations` is empty, and `version_scan_complete`
and `owned_version_complete` are false because the Claude scanner does not apply.
History completeness remains `unknown`.

### Relationships

`unknown` means that capture has no accepted direct relationship proof. For
Claude, `child` names the parent source key from the exact subagents layout. A root `fork` needs
an owned native `forkedFrom.sessionId`. A child `fork` needs its exact companion's
`isFork: true` and unprefixed `parentAgentId`. Self-references and conflicting
owned fork evidence do not prove a fork edge. Similar text, timestamps, or
repository association do not establish lineage. A parent edge does not imply
that the parent was captured.

OpenCode takes `parent_session_id` and `fork_session_id` from the same snapshot as
its native rows. A valid fork takes precedence when both edges exist; both raw
columns remain in source. Parent keys stay in the selected database namespace and
host scope. Missing columns, absent edges, and self-references do not prove a
relationship. A relationship never reads parent content or grants access to it.

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
Resume validates and uses the saved plan without negotiating again. If a response
is lost before the plan is saved, negotiation repeats with the same manifest and
must return the original plan. Current status must bind that exact manifest and
plan before upload or acceptance. Invalid bindings fail closed without changing
saved bytes or progress.
Plan digests register the expected saved bodies. They do not prove that stored
bytes have passed integrity verification.

`ServerStatus` binds the same identity, manifest digest, and plan ID. Its
`uploaded_parts` confirm uploaded chunks only. A reuse decision is not an upload
receipt. Unknown keys, duplicates, reused chunks in uploaded receipts, and wrong
hashes fail closed. Before integrity acceptance, storage expiry can remove
provisional parts from current status. The library keeps the same saved bytes,
manifest, and plan, uploads those missing parts again, then resubmits the exact
finalization request. A restart preserves this recovery state. Integrity
acceptance cannot regress. Confirmation never comes from local bytes sent or a
successful HTTP upload alone.

Each `PartReceipt.compressed_sha256` is the registered expected saved-body digest.
It binds the service's stored-part acknowledgment to the expected chunk in the
manifest and frozen plan. It does not claim that storage returned SHA-256 or
independently verified those bytes. A transfer receipt confirms the stored part
and its request identity, not full capture integrity. Optional storage checksum
metadata can be unavailable without changing this distinction. Never use an ETag
as SHA-256 or describe an unavailable checksum as verified. The library still
requires an exact digest match against the bound request.

Only `integrity_accepted: true` confirms that the service has read and verified
the complete stored capture. Verification covers compressed and expanded chunk
lengths and digests, stream lengths and digests, and manifest and metadata
bindings, including reused chunks. Transfer receipts and storage checksum
metadata do not replace those full-byte checks or permit automatic spool release.

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

Default bounds are 512 MiB per transcript or expanded OpenCode native-row stream,
64 KiB per companion and generated metadata stream, 1 GiB expanded capture
size, 512 queue entries, and 2 GiB of spool bytes. A record cannot exceed 512 KiB.
File streaming reads use 64 KiB buffers; SQLite uses bounded cell fragments.
Before reading native content for full capture, the spool reserves conservative
compressed size and room for two bounded queue records. Limits may be lowered
within the supported bounds.

`SpoolCapacity` distinguishes a queue or disk admission refusal before publication
and reservation. The host may save a capacity wait and retry the same ID after
fresh consent, source, and project checks. A missing entry with unknown delivery
history does not establish this permission.

`resume_step` negotiates only when no plan is saved, reconciles current status,
hashes at most one selected saved chunk, and sends at most that chunk. It validates
every status response but does not rewrite or sync the queue record when status
and capture state are unchanged. It does not scan every saved part on each turn.
Completion uses a later step after required uploads are confirmed. The host can
round-robin pending captures and keep uncaptured selections in its own durable
state. `resume` is a synchronous draining helper, not a fair scheduler.

One spool permits one executor and at most one synchronous part upload at a
time. The host must also bound its root list, discovery workers, and pending
selections. Per-root discovery limits are not an aggregate limit across roots.
Run these blocking calls on bounded blocking workers, never on UI or async runtime
threads. The library does not own an executor or require a Tokio runtime.

Progress separates total compressed bytes, required transfer bytes, reused
bytes, and server-confirmed upload bytes. Before negotiation,
`negotiated: false` means the transfer/reuse split is unknown. These counters do
not establish integrity acceptance. Plan and confirmed status survive restart.

Completion intent is durable before submission. A lost completion response may
repeat only the same idempotent request. Once submission succeeds, later library
steps poll its status rather than resubmitting it. After validated status still
reports pending integrity, the host may replay the original completion request
with bounded backoff. This replay must keep the same capture identity, reservation,
manifest, and plan. It does not reset saved completion state or prove acceptance.
HTTP 202, logout, auth loss, executor stop, or process exit never release bytes.
Only status with exact bindings, all required upload receipts, and
`integrity_accepted: true` permits automatic release. Every reused chunk remains
local until this same acceptance.
Explicit capture cancellation is the only other byte-release path.

After integrity acceptance, the spool releases chunk files but keeps the bounded
accepted receipt, manifest, plan, and final status. Restart retains this receipt.
The spool saves verified status and accepted state in one atomic record update
before releasing chunks. Opening also recovers a valid accepted status saved
with a pending state. This recovery preserves the receipt and denies cancellation.
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

### Caller and transport ownership

The desktop host owns source grants, current consent, credentials, durable request
selection, and accepted-source state. The spool owns its records and saved bytes.
Returned entries are snapshots, not a storage API. Renderer code must not choose
upload locations, hold credentials, or run synchronous capture and upload calls.

`Transport` keeps service I/O outside the capture library. Implementors must
preserve exact request bindings, validate upload locations and redirects, bound
response sizes and timeouts, and map failures to closed errors. Upload must send
the exact supplied gzip body. A short body is an error, not a reason to repair or
recompress it. A failed request can still have succeeded remotely; reconcile
under the same saved identity, manifest, and plan.

The [`Transport` API](../../crates/antiburn-cloud-capture/src/contract.rs) states
the ordering, idempotency, and receipt rules for each method. Those rules do not
select an HTTP API or enable a service endpoint.

### Failure and operator recovery

`UnsupportedSource` means that the native database schema or ownership rules are
unsupported or unsafe. Capture does not silently omit that source, switch
formats, or expose raw SQL errors. This is separate from a saved wire format
that the current build cannot read.

Saved captures from a different wire format return `UnsupportedFormat` before
record recovery or upload. The spool does not rewrite their records, recompress
their chunks, or release their bytes. This includes the string-version format.
Before changing builds, drain or explicitly cancel pending captures under the
original build and acknowledge accepted receipts after saving host state. If
that build is unavailable, preserve a private copy for operator recovery. Do not
edit version fields or assign a new ID to make an old retry appear compatible.

Inspect saved entries after a capture failure. Errors before queue publication
may leave no entry. A published `preparing` entry normally requires explicit
cancellation, then a new ID to capture a later source generation. Do not replace a
queued capture when the source or metadata changes.

The narrow exception is a proved capacity refusal that created no reservation.
The host can save that proof and a retry deadline, then retry reservation under
the same capture ID with `Spool::retry_reservation`. Before retrying, it must
recheck the current account, destination, service, source access, project policy,
and unchanged source observation. A changed recorded CWD outside approved
folders, changed source observation, or missing native identity interrupts the
job without changing its ID or saved spool files. Absence from one discovery page
does not prove that an identity is missing.

The host must durably clear the capacity-only retry proof before reservation I/O.
Unknown delivery, historical failures without this proof, and a generic HTTP 429
do not permit automatic same-ID source reads. The spool accepts this retry only
for a present `Preparing` entry with no reservation, manifest, plan, status, or
completion intent, and no files other than `record.json`. Missing, quarantined,
reserved, and partial entries fail without changing saved bytes.

One corrupt record or invalid spool entry can prevent the whole spool from
opening. Corruption is not permission to delete bytes, change a capture ID, or
claim acceptance. Use this recovery procedure:

1. Stop all executors for the spool. If limits changed, restore the prior limits
   before trying to open it again. Saved entries must fit the configured limits;
   drain pending work and acknowledge accepted receipts before lowering them.
2. Preserve a private copy of the spool, its ownership and modes, and the host's
   request and terminal state. Do not attach native bytes or raw records to logs,
   analytics, issues, or support reports.
3. Have an operator inspect the affected entry locally. Correct an access or
   permission problem only against the trusted spool setup. Do not weaken the
   symlink, ownership, hard-link, mode, or digest checks.
4. If repair needs more work, an operator may move the whole affected capture
   directory to a separate private quarantine while execution is stopped. Keep
   every saved byte and its identity. Quarantine does not cancel the request or
   authorize recapture. Retain its host selection and consent state, and restore
   the entry under the same ID before resuming it.

The library does not repair corrupt records or reconstruct missing saved chunks
from a newer source. An explicit user cancellation remains the decision to
release an unaccepted capture. An accepted receipt still needs the host's durable
terminal state and acknowledgment.

## Synthetic fixtures and checks

[`fixtures/cloud/capture-v1`](../../fixtures/cloud/capture-v1) is the public
`claude-native-capture-fixtures-v1` package. Its manifest pins each native input,
typed section example, and independent reference stream part by length and
SHA-256. These synthetic shapes do not certify every historical Claude release.

[`fixtures/cloud/opencode`](../../fixtures/cloud/opencode) contains synthetic
earlier/current SQL inputs and real saved `ses_main` wire references. Tests
rebuild the current source in separate temporary roots and compare its manifest
and exact gzip parts with the checked-in output. The package manifest pins each
input and reference file by byte length and SHA-256.

The [Codex rollout fixtures](../../crates/antiburn-cloud-capture/tests/fixtures/codex)
exercise exact capture and distinguish owned headers from copied headers.
The [Pi fixture](../../crates/antiburn-cloud-capture/tests/fixtures/pi) exercises
exact capture of branches and tool results without assigning separate workers.

```sh
cargo fmt --manifest-path crates/antiburn-cloud-capture/Cargo.toml --check
cargo check --manifest-path crates/antiburn-cloud-capture/Cargo.toml --lib --locked
cargo clippy --manifest-path crates/antiburn-cloud-capture/Cargo.toml --all-targets --features cloud --locked -- -D warnings
cargo nextest run --manifest-path crates/antiburn-cloud-capture/Cargo.toml --features cloud --locked
```

CI selects this workspace for capture source and synthetic package changes. It
checks the default library separately and enables `cloud` for Clippy and Nextest.
The non-UTF-8 filesystem cases run on Linux, which permits those filenames.
Windows builds do not establish desktop capture support. None of these checks
enables Cloud in desktop builds or releases.

The explicit reference writer reads only these two synthetic packages:

```sh
cargo run --manifest-path crates/antiburn-cloud-capture/Cargo.toml --features cloud --locked --example write_fixture
```
