# Cursor producer contracts

These fixtures are synthetic. They contain no captured session data.

## Pinned Cursor Desktop chunked headers

`native_chunked_headers.json` characterizes the chunked-header store in Cursor
3.28.7, commit `25b52e7d03ebf2d5608914158e119f7992432a30`. The installed
`workbench.desktop.main.js` producer has SHA256
`a002fc9d697368f2f298461edd24ee8612303717b66e3843f7bb5fa73975c8f8`.
Its header-log reader and writer use exact
`composerConvHeaders:<composerId>:<six-digit chunk index>` keys. Each value is a
JSON header array. Numeric chunk order and array order define conversation order;
the root declares chunk size, chunk count, total header count, and generation.
The writer batches root and chunk changes atomically; generation is a root
conditional-write field, not part of the chunk key or header.

The writer filters original positional chunks, so an interior chunk can contain
fewer than 256 headers. The fixture tests that shape and uses IDs whose lexical
order differs from recorded order. It uses invented conversation text and IDs.
This pin establishes a storage shape, not a historical release range or proof of
natural check detection.

The same producer's Fork Chat path copies recorded bubble rows with their IDs
and original creation timestamps, then sets the cloned composer's creation time
to the fork time. It does not record a durable parent ID for local Fork Chat.
The fixture tests the additional ownership gate for chunked evidence: each
referenced bubble must have a native ISO UTC timestamp strictly after the
composer's epoch-millisecond creation time and within the discovery time.
A copied prefix stays ineligible even after new bubbles arrive or the chat is
renamed. Missing, invalid, equal, earlier, and future timestamps leave ownership
unproven. The gate does not invent a parent identity or establish chronology.

## Public generic store pin

Repository: `antonvp/cursor-acp-enriched`.
Commit: `4801804543f0234bdfc266fbd53d81a6f20e9508`.

- [`src/storeReader.ts`](https://github.com/antonvp/cursor-acp-enriched/blob/4801804543f0234bdfc266fbd53d81a6f20e9508/src/storeReader.ts)
  reads `blobs(id, data)`, with UTF-8 JSON in `data`.
- [`test/fixtures/create-fixture.mjs`](https://github.com/antonvp/cursor-acp-enriched/blob/4801804543f0234bdfc266fbd53d81a6f20e9508/test/fixtures/create-fixture.mjs)
  constructs assistant `tool-call` blocks with `toolCallId`, `toolName`, and
  `args`, and tool `tool-result` blocks with `toolCallId` and `result`.
- Results can be strings or arrays of text blocks. Rich results live at
  `providerOptions.cursor.highLevelToolCallResult` on a tool-role payload.
- [`src/pathDiscovery.ts`](https://github.com/antonvp/cursor-acp-enriched/blob/4801804543f0234bdfc266fbd53d81a6f20e9508/src/pathDiscovery.ts)
  checks `.cursor/acp-sessions/<sessionId>/store.db` first, then the older
  `.cursor/chats/<hash>/<sessionId>/store.db` path.

This pin is a third-party reader contract, not a Cursor release-range claim.
`store_reader_blocks.jsonl` uses its generic shape. Added user records and IDs
test local preservation; they do not establish a public scope workflow schema.
The existing transcript fixtures characterize the accepted local adapter shapes.
They do not prove that all Cursor versions persist those shapes.

## Accepted normalization

The parser retains generic tool arguments, result structure, exact call IDs,
and recorded text. Array results remain JSON arrays, so block boundaries and
order survive. A rich result is separate tool evidence with the same call ID
only when the record has exactly one result block. It has no user authority.
Ambiguous multi-result rich payloads remain unbound tool evidence.

Store synthesis accepts top-level objects with a recognized `role` and string
or array `content`, and the local fixture-backed `{messages: [...]}` container.
The container has only that key, and every item must be a transport message.
Objects with `type`, root arrays, unknown wrappers, and all tool-block aliases
remain opaque. Discovery does not recurse through arguments, results, arbitrary
object values, or nested containers. `discovery_boundaries.json` tests each tool
alias and unknown wrappers. Unrecognized blobs retain their exact JSON value in
a `__unrecognized_store_blob__` record and produce a parser gap, without a user
turn. Session metadata is not treated as a message blob.

String and array content, provider options, role, and message identity survive.
A top-level message without its own ID uses its blob ID. Repeated user text
remains separate occurrences. Full user text survives, including prefixes,
suffixes, literal `<user_query>` examples, and multiple tagged sections.
`user_query_text.jsonl` tests these cases. No wrapper text is assumed disposable.

Array order and database row order survive; row order is not proof of chronology
or branch ancestry. Store synthesis always emits `cursor_scope_ordering` with
`unproven`, because the public pin does not establish a session branch sequence.
IDE synthesis emits the same gap when it lacks a nonempty, unique, complete
`fullConversationHeadersOnly` ID list. Conversation-map traversal and sorted
bubble IDs are fallback presentation orders, not scope chronology. An accepted
header list preserves its recorded sequence; it does not prove question or plan
approval. Sources that combine header and other conversation representations
remain unproven.

The parser maps `cursor_scope_ordering: unproven` to an
`AttributionIncomplete` unusable record. This reaches whole-source coverage,
even when every user message has a timestamp. `ordering_corrections.json` tests
reversed IDs, both insertion orders, tied IDE timestamps, descending store
timestamps, and a user correction. Future whole-scope assembly must reject this
partial history rather than sort it or infer approval order.

`native_scope_bubbles.json` characterizes local IDE bubble extraction: type `1`
is user and type `2` is assistant. Direct text keeps whitespace; a valid `richText`
JSON string keeps its exact serialized structure instead of joining fragments.
Tool/error fallback fields do not supply missing user text. These synthetic
adapter fixtures have no public dedicated workflow pin. They do not establish
question or Build records in `toolFormerData`, `toolResult`, or composer metadata.
Assistant fallback text is not a native plan version or user approval.
Missing referenced bubbles and unavailable user content produce explicit parser
gaps. They cannot become complete history by silently skipping those records.

The existing chats discovery root stays in use. The ACP path is not broadly
discovered by this change. The pinned fixture generator has no session metadata;
a tool-only ACP database cannot satisfy the current session source contract.
Tests read the generic schema at an exact path with explicit synthetic metadata.

## Optional evidence gaps and negative controls

`optional_evidence_negative.jsonl` deliberately invents question/plan payloads
inside the pinned generic transport. These are negative controls, not accepted
dedicated schemas. `AskQuestion`, `createPlan`, `approved`, `success`, and
`Build approved` do not establish an authoritative answer or plan approval.
Pending calls, orphan results, and ordinary tool strings remain generic evidence.
Ordinary user messages remain the source of user authority.

No reviewed public pin establishes persisted dedicated question IDs, option
selections, custom answers, answer origin, cancellation, plan versions, plan
edits, or Build approval in the accepted sources. No plan URI is resolved to a
companion. Ephemeral, home-saved, and workspace-saved plans cannot be associated
by directory location alone. Missing references remain unavailable; no plan
directory is scanned and current file bytes do not prove an approved version.

Shared integration must update source/check coverage limits and bump the parser
revision for content preservation. Do not advertise typed question/plan support
or complete native scope history from this generic pin. Dedicated evidence needs
an additional producer pin and matching transcript/companion fixtures.

## Pinned Cursor Desktop Read/Grep completion

`native_read_grep_binary.jsonl` constructs synthetic protobuf messages for the bounded Cursor 3.25.6 Read/Grep schema. Every ID, path, timestamp, parameter, result, and encoded byte is generated from synthetic values. This is not a redacted transcript or captured machine output. The producer descriptor pin and native observations remain in ignored local audit evidence.

The pinned shape supports only schema-matched successful or error Read/Grep completion with exact tool ID, tool name, arguments, and completed lifecycle state. Discovery decodes the opaque bytes and emits a small verified binding. It never projects the binary. Result content remains unavailable, and read extent or search output completeness is not claimed. Unknown fields, malformed encodings, missing or conflicting oneofs, mismatched arguments, and unrecognized modes remain unknown or unavailable as appropriate. The fixtures do not establish a producer release range.

`native_read_grep_projected.jsonl` is the corresponding synthetic normalized projection used by the analysis reader. It contains no binary bytes and preserves only the verified binding.
