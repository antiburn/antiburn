# Native capture v1

`antiburn-native-capture-v1` captures exact native source bytes for later Cloud
processing. Cloud owns semantic parsing. The desktop does not convert its local
normalized session model into an upload payload. Local parsing and Burn Check
coverage remain governed by [session coverage](../session-coverage.md) and
[check coverage](../check-coverage.md).

This is an unreleased public contract. The standalone
[`antiburn-cloud-capture`](../../crates/antiburn-cloud-capture) crate contains no
network client. Its default feature set is empty. All capture code requires the
explicit non-default Rust feature `cloud`. The library does not enable desktop
sync, select a service URL, grant consent, or enable production.

## Source scope

The scoped discovery API accepts one explicitly authorized Claude
`projects/<project>` directory. It never looks up a home directory or reads a real
provider folder in tests. The host must enforce source access policy and consent
before discovery and capture.

Discovery reads directory entries and file metadata, not native content. It
keeps admitted directory handles for enumeration, companion checks, and capture.
Renaming an ancestor cannot redirect source reads. It admits:

- Direct `<session-id>.jsonl` transcripts, excluding `audit.jsonl`.
- Separate `<session-id>/subagents/agent-<id>.jsonl` transcripts.
- Only the exact same-stem `agent-<id>.meta.json` companion of an admitted child.

Each child or fork is an independent capture with its own source key,
reservation, hashes, retry state, and acceptance. A parent capture never includes
child transcripts or another fork's replay source. Project files, audit logs,
job state, arbitrary JSON files, referenced attachments, and files named in
transcript text are outside this scope. A companion retains unknown fields as
exact native bytes. A child remains discoverable when its parent's transcript
is absent.

Discovery has a default bound of 4,096 directory entries and 512 session
candidates. It fails the whole discovery call when it reaches a bound; it does
not claim that a truncated list is complete. Symlinked directories and sources
are unavailable. Native sources must be regular files with one hard link.

The capture takes file handles and pins size and filesystem identity before
reservation. It checks size, identity, modification time, and change time before
and after streaming. A changed source fails capture. It does not silently upload a
moving prefix or reread newer content under the same capture ID. An unchanged
native file can still have lost earlier history before capture. History
completeness is always `unknown`.

## Native metadata

The generated `capture-metadata.json` has `schema_version: 1`, the named capture
version, the fixed capture identity, agent `claude-code`, and source format
`ClaudeJsonl`. It records the native filename session ID and optional child agent
ID separately. These are capture identifiers, not semantic identity certification.

`source_agent_version` is a historical version recorded in native JSONL, or JSON
`null`. It never comes from an installed executable. Streaming inspection keeps
at most 16 distinct top-level `version` strings and at most 64 KiB per JSONL
record. One distinct version with a complete scan produces the version string.
Mixed versions, malformed records, oversized records, or an incomplete final
record leave it null. `recorded_agent_versions` retains the bounded observed
set, and `version_scan_complete` records the inspection limit. The native
transcript is not changed or clipped by these metadata limits.

Relationships are explicit:

- `unknown` means that capture has no accepted relationship proof.
- `child` records the exact parent source key from the native subagents layout.
- `fork` records a direct native `forkedFrom.sessionId` or an exact child
  companion's `isFork: true` and unprefixed `parentAgentId`. A self-reference
  does not prove a fork edge.

Unknown or conflicting fork metadata does not infer lineage from matching text,
timestamps, or repository association. A fork edge does not imply that its
parent was discovered, uploaded, or remains available.

`extensions` is a namespaced JSON object for future optional metadata. Producers
must keep it bounded to 32 KiB and must not use it to change the v1 framing,
source ownership, or identity rules. Readers must tolerate unknown extension
keys. Changing required fields or byte framing requires a new capture version.

## Byte layout and hashes

The expanded stream is an ordered concatenation, without separators:

1. Exact `transcript.jsonl` bytes, including invalid JSON, original line endings,
   and an incomplete final record.
2. Exact `native.meta.json` bytes, when the proven companion exists.
3. UTF-8 `capture-metadata.json`, serialized as compact JSON.

The manifest records each file's role, expanded offset, byte length, and lowercase
SHA-256. Empty native files are permitted. The generated metadata is nonempty.
No filename or content is used as an extraction path.

Chunks cover this stream in order. Every non-final chunk expands to exactly
8,388,608 bytes (8 MiB). The final chunk expands to 1 through 8,388,608 bytes.
File boundaries do not reset chunk boundaries. Each chunk is one independent
gzip member, using level 6, modification time zero, OS byte 255, and no filename
or comment. The compression name is `gzip-mtime0-os255-level6`.

The manifest records contiguous zero-based chunk indices, expanded offsets,
expanded and compressed byte lengths, and both SHA-256 digests. It also records
the complete expanded stream's SHA-256 and length. Manifest identity is the
SHA-256 of the compact JSON serialization of `CaptureManifest`, in its declared
field order. The checked-in lockfile and synthetic reference pin the compressor
output used here. Resume always sends saved compressed bytes; an upgrade must
not recompress an existing capture.

Streaming reads and hashes use a 64 KiB buffer. The default native transcript
limit is 512 MiB; a native companion and generated metadata each have a 64 KiB
limit. Total expanded capture size is bounded to 1 GiB, including metadata.
The queue defaults to 512 entries and 2 GiB of spool bytes. It reserves a
conservative compressed size plus space for two bounded queue records before
native reads. A queue record cannot exceed 512 KiB. Limits may be lowered.

## Private durable queue

The host supplies a private local spool directory. Unix directories use mode
`0700`; files use `0600` and belong to the effective user. The library rejects
shared modes, symlinks, and hard-linked spool files. An exclusive file lock admits
one executor across processes. Native opens walk directory handles with
`O_NOFOLLOW`; they do not follow a replaced source symlink. Spool iteration,
creation, reads, writes, renames, and deletion stay relative to admitted
directory handles. Renaming an ancestor cannot redirect a locked executor.

This scoped implementation supports Unix private modes. Other platforms fail
with `UnsupportedPlatform` until a private ACL implementation is available.
The host must not claim Windows capture support.

Queue records and chunks are flushed before publication. Queue record updates
use write, file sync, atomic rename, and directory sync. Entries persist
independently, so a process restart can enumerate many pending captures.
The capture identity fixes:

- Random host-generated `capture_id`, used as the idempotency key.
- Verified account and destination identifiers.
- The selected device-local source key, independent of inferred repositories.

Source paths and credentials do not enter the upload manifest. The host must
scope source keys to a device and authorized source root with
`Candidate::in_source_scope` to avoid collisions. The library does not generate
IDs, store credentials, or schedule work. The host must never reuse a capture ID,
including after acceptance or cancellation. It must retain terminal operation
state in its own durable request record. This spool removes terminal entries;
it is not an unbounded capture-ID registry.

Capture saves a `preparing` record, then calls `Transport::reserve` immediately
before reading native content. Reservation failure, source mutation, or a crash
during capture leaves an interrupted entry. It cannot resume native reads. The
user can explicitly cancel it and start a new capture with a new ID. Repeating
`capture` with an existing identity returns its saved entry without reopening
native content.

Fully saved captures become `queued`. Submission becomes `awaiting_integrity`
before completion is sent. An executor stop, process exit, logout, auth failure,
offline result, or upload failure keeps every saved byte. These events are not
capture cancellation.

## Narrow transport and resume

`Transport` exposes `reserve`, `negotiate`, `status`, `upload_part`, and `complete`.
The native host owns authentication, consent, verified endpoint policy, timeouts,
bounded response decoding, and any HTTP implementation. A destination ID is not
a URL. The transport must not redirect a capture to another account or
destination, including during token refresh.

Reservation and completion are idempotent for the fixed capture ID. A reservation
returns a bounded opaque reservation ID. Negotiation receives the complete
manifest. It may report reused
parts, but only through server-confirmed part receipts bound to that exact
identity and manifest digest. Each receipt supplies a chunk index and its exact
compressed SHA-256. Unknown indices, duplicates, mismatched hashes, and identity
or manifest mismatches fail closed.

Resume validates local saved chunk sizes and hashes before transport calls. It
negotiates current server status, skips only confirmed parts, and queries status
after each successful upload. A lost upload response leaves the capture intact;
the next resume reconciles server status before it resends a part. It never
reopens native sources.

`complete` only submits the saved manifest. HTTP 202 or another successful
submission does not prove integrity acceptance. Only a server status with
`integrity_accepted: true`, the exact capture and manifest identities, and every
confirmed part permits automatic byte deletion. Reused chunks remain local
until this same acceptance. Explicit `Spool::cancel` is the only other
byte-release path. It cancels this local capture; remote deletion is a separate
host operation.

Transport errors use closed values (`Offline`, `Unauthorized`, `Conflict`,
`Rejected`). Filesystem errors expose only their error kind. Native paths,
transcript content, credentials, signed URLs, and raw service errors must not
enter logs, analytics, or renderer status.

## Synthetic fixtures and checks

[`fixtures/cloud/capture-v1`](../../fixtures/cloud/capture-v1) is the public
`claude-native-capture-fixtures-v1` package. Its manifest pins byte lengths and
SHA-256 digests for every native input and reference capture. It contains no real
transcript, copied provider output, credential, private link, or machine path.
Its source shape is synthetic and scoped; it does not certify every historical
Claude release.

From the repository root:

```sh
cargo fmt --manifest-path crates/antiburn-cloud-capture/Cargo.toml --check
cargo clippy --manifest-path crates/antiburn-cloud-capture/Cargo.toml --all-targets --features cloud --locked -- -D warnings
cargo nextest run --manifest-path crates/antiburn-cloud-capture/Cargo.toml --features cloud --locked
```

The reference writer uses only this package's synthetic sources:

```sh
cargo run --manifest-path crates/antiburn-cloud-capture/Cargo.toml --features cloud --locked --example write_fixture
```
