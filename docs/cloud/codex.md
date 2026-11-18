# Codex rollout capture

The [capture library](capture.md) can package one explicitly authorized Codex
rollout on Unix. This source contract does not enable Codex upload in the
desktop app. The host owns rollout discovery, source access, account-bound
content consent, and scheduling. Run library calls on a bounded blocking worker.

## Source identity and folder permission

`reopen_codex(path, expected_id, limits)` accepts one absolute `rollout-*.jsonl`
path and the expected native session ID. It does not scan for other sessions,
resolve home directories, or read a session index or database. The host supplies
the expected ID for the selected source. The first physical line must be a
complete JSON object with `type: "session_meta"` and a matching `payload.id`.
The header is bounded to 64 KiB including its newline. The filename, a
`payload.session_id` alias, and later records cannot prove identity. An empty,
missing, oversized, malformed, ambiguous, or mismatched header does not admit a
source. The file must be regular with one hard link, and no path component may
be a symlink.

`Candidate::in_source_scope` adds the host's stable account and source-root scope
to `codex/<id>`. A fork or spawned thread is an independent source with its own
capture and retry state. The library does not read the parent to export a child
or fork.

`Candidate::project_evidence()` uses only the first owned header's `payload.cwd`.
It does not report changes during the session or hash the full file.
`ProjectSelection::admit` requires the recorded directory to match an approved
folder tree. Missing or invalid CWDs grant nothing. Later copied headers,
`turn_context`, tool arguments, and paths inside messages cannot grant permission.
The host must check current content consent separately.

## Export and evidence limits

The library pins the file generation before reservation. After reservation, it
checks the same file and reads the header again to prove the native ID and
recheck any approved folder policy before spooling the body. Mutation interrupts
capture instead of publishing a partial package. Wait for the rollout to settle.
If the spool has an interrupted entry, explicitly cancel it before capturing a
later generation under a new ID.

An admitted capture keeps the whole rollout as `transcript` with role
`native_transcript` and schema `codex-jsonl`. The shared `Limits::source_bytes`
bound defaults to 512 MiB. The library does not clip or rewrite the bytes.
It keeps original line endings, copied records, unrecognized fields, invalid
UTF-8, and interrupted final records after the header. This is a framing
contract, not a claim about support for every Codex version. It exports no
companion or external attachment and does not follow paths or URLs.

The separate [metadata stream](capture-metadata.md) uses `agent: "codex"` and
`source_format: "CodexJsonl"`. The first owned header can report:

| Native field under `session_meta.payload` | Metadata |
| --- | --- |
| `id` | `native_session_id`; `native_agent_id` is null |
| `cli_version` | One `owned` version observation if it has a supported version string |
| `forked_from_id` | `fork` with proof `codex-session-meta-fork-v1` |
| `source.subagent.thread_spawn.parent_thread_id` | `child` with proof `codex-session-meta-thread-spawn-v1` |

Missing, invalid, self-referential, or competing edges do not prove a
relationship. A known edge grants no access and does not prove that the parent
has a capture. Later copied headers keep their native bytes but supply no CWD,
version observation, or relationship evidence. The library does not infer lineage
from other fields or from matching content.

A recorded `cli_version` identifies the first header's observed producer version.
It does not prove the producer for all retained content. `source_agent_version`
is null, both version completeness flags are false, and `history_completeness`
is `unknown`.

The [synthetic rollout fixtures](../../crates/antiburn-cloud-capture/tests/fixtures/codex)
cover a fork with a copied header and an independent spawned thread. Capture
tests create files in temporary directories and compare expanded spool bytes
with the original input.
