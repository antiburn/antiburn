# Cursor capture source

The capture library supports the Cursor sources below on Unix with the `cloud`
feature. These rules do not describe all Cursor releases or grant upload
consent. Local Cursor analysis uses a separate
[source contract](../session-coverage.md).

The host must authorize the exact provider source and bind the candidate to a
stable account and source scope. It must approve a recorded project folder
before capture. A cached local session, provider directory name, encoded path,
message, tool path, or inferred fork cannot grant permission.

## IDE composer

`CursorIdeComposer` binds one composer ID to one authorized
`User/workspaceStorage/<workspace-id>` directory and the paired
`User/globalStorage/state.vscdb`. The host must authorize both stores.

- Bounded discovery returns independent IDs in lexical order. It reads the
  exact `composer.composerData` workspace row and checks bounded global composer
  metadata and the native `composerHeaders` index for a recorded workspace ID.
  It does not read bubble payload.
  An ambiguous composer index marks the page incomplete. Other proved
  composers and the continuation remain available.
- `workspace.json` must record one absolute local `folder` URI without escaped
  path characters. For chunked headers, the global `composerHeaders` table
  must have exactly one row with the root's `composerId` and the directory's
  `workspaceId`. An inline composer can also match through its own
  `workspaceIdentifier` or through the workspace's `allComposers` list. A
  selected or last-focused list alone cannot prove ownership. It also needs a
  matching `workspaceId` in the composer. A conflicting recorded workspace or
  composer ID blocks capture.
- Before consent, SQL returns only bounded composer identity, workspace
  fields, and layout flags. After reservation, the library rechecks folder and composer
  membership before it selects global payload. It exports the exact
  `cursorDiskKV` row with key `composerData:<composer-id>` and the exact
  `bubbleId:<composer-id>:<bubble-id>` rows named in its native headers. The
  headers come from a non-empty `fullConversationHeadersOnly`, or from the
  `_v: 18` `headerLog` layout with an empty inline list. Missing or duplicate
  keys, duplicate header IDs, and other nonempty conversation representations
  are unsupported. There is no fallback to inferred bubble order.
- A single read snapshot binds all selected global rows. The workspace proof
  comes from a different database and file. The library checks it before
  export and again before publication. It does not claim an atomic snapshot
  across those stores.

### Chunked header snapshots

In Cursor 3.31.15, build `3eb8c1339083`, the local IDE and Desktop writers
publish the composer root and changed header chunks in one SQLite transaction.
Incremental writes check the preceding root `headerLog.generation` and can
reuse unchanged chunks. Chunk keys do not contain a generation. Bubble writes
commit separately. A snapshot can contain a bubble update that committed
before the corresponding header update. The capture represents the stored
snapshot, not an atomic revision of every message or a complete history. The
observed build supports this layout claim, not a release range.

The library reads `headerLog` with a `generation` of 1 through 128 ASCII
letters, digits, underscores, or hyphens. It requires a `chunkSize` of 256,
`chunkCount` from 1 through 256, and `count` from 1 through 16,384.
It reads only the declared `composerConvHeaders:<composer-id>:<six-digit-index>`
keys, starting at `000000`. Each chunk is a JSON array of at most 256 headers.
Chunks can be sparse, including nonfinal chunks. The sum of array lengths must
equal `count`. Every header must have a unique `bubbleId` and numeric `type`
of 1 or 2. Its exact composer-scoped bubble row must record the same ID and
type. Root, chunk, and scoped bubble JSON values used for proof are limited to
1 MiB each. Expanded stream and time limits still apply.

The native composer-index row is checked in the same snapshot as the exported
rows, and its workspace ID is checked again before publication. The paired
`workspace.json` must still prove the approved folder. None of the workspace
or index rows are uploaded.

The row stream uses ID `cursor-ide-rows`, role `native_rows`, and schema
`antiburn-sqlite-native`. It declares `cursorDiskKV` with `key` as its logical
key. The composer row comes first, then declared chunk rows in index order for
chunked sources, then exact bubble rows in header order. The stream contains no
workspace index, workspace file, unreferenced row, or other composer's row.

## CLI transcript

`CursorCliAgentJsonl` binds one exact `projects/<project>/agent-transcripts/`
`<session>/<session>.jsonl` file under the authorized `.cursor` root. The same
root must have exactly one `chats/<workspace>/<session>/meta.json` for that
ID. Its `schemaVersion` must be `1`, `hasConversation` must be true, and its
top-level `cwd` must match an approved folder. In the observed
`2026.11.22-e9042b2` sample, the chat workspace name is the lowercase MD5 of
that exact CWD. The transcript project name is the absolute path without its
initial slash, with the other slashes replaced by hyphens. Both bindings must
match. The library rejects duplicate IDs across transcript project roots.
Storage directory names and transcript contents do not grant permission. GUI
transcript mirrors without same-ID CLI metadata do not qualify.

Discovery pages visit at most 16 session entries and read no transcript bytes.
An entry whose supported sidecar reports `hasConversation: false` is not a
session. Skip it without marking the page incomplete.
Ambiguous or oversized sidecars block only their own candidate and mark the
page incomplete. Folder discovery and Save follow continuations within a shared
budget. Save still needs valid evidence for each selected folder group.
Capture pins the transcript before
reservation and checks the sidecar after reservation and again before
publication. Every record must be a bounded LF-terminated JSON object. The
first record must be a user record with the observed `message.content` array.
Capture verifies framing without rewriting bytes. The transcript stream uses ID
`transcript`, role `native_transcript`, and schema `cursor-cli-jsonl`. It
preserves exact bytes. Capture does not upload the sidecar, `store.db`, encrypted
blobs, database keys, or external files. Legacy `chatSessions` JSON and
unbound transcripts remain unsupported.

## Bounds and fixtures

Composer rows use the [typed SQLite format](sqlite-native.md). Unknown ordinary
columns in those rows retain their exact SQLite types and bytes. Composer proof
values are at most 1 MiB each. Bounds include the configured discovery entry
limit and 16,384 bubble references. CLI sidecar proof is at most 64 KiB.
Each export has a 30-second deadline and must fit the expanded source byte
limit. The library never publishes a truncated capture.

Capture metadata uses agent `cursor` and the exact source format name. Runtime
version and relationship remain unknown. No installed version, title, or
prefix match proves a historical producer or lineage.

The [Cursor fixtures](../../fixtures/cloud/cursor) are synthetic. Tests create
temporary stores and files and verify selected bytes, row order, project
rechecks, and exclusion of unrelated inputs. The fixture shapes are not a
release range claim.
