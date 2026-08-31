# Discovery adapter test fixtures

Inputs for the adapter tests under `../`. Each file is loaded with `include_str!`
from the test that consumes it, so it must stay inside this crate.

## Provenance

| File | Origin |
| --- | --- |
| `cursor-cli-store-fork.json` | **Synthetic**, authored from the contract |
| `cursor-cli-agent-fork.json` | **Synthetic**, authored from the contract |
| `claude-cli-job-fork.json` | **Synthetic**, authored from the contract |
| `claude-desktop-session-sidecars.json` | **Synthetic**, authored from the contract |
| `opencode-cli-production-db.json` | Re-homed from `crates/analysis`, explicitly authorized |

"Authored from the contract" means the values were derived from the parser code
and the consuming test's assertions — the shape a reader requires and the
boundary a test proves — not from any recorded session. Every identifier, path,
title, and message body is invented. The only user is `avery` and the only
workspace is `/home/avery/projects/demo-app`.

A production capture must not be added here, redacted or otherwise. If a test
needs a shape that isn't represented yet, author it the same way: read the
parser, write the minimum input that exercises the branch, and say so below.

## What each file encodes

### `cursor-cli-store-fork.json`

Consumed by `cursor::tests::cursor_store_db_reader_detects_the_19_of_27_fork_boundary`.

Two blob-id sets, 19 and 27 entries, where the parent's is a strict subset of
the child's — the exact condition `cursor_store_db_fork_observation` accepts as
fork lineage (`parent.blob_ids.len() < child.blob_ids.len()` and
`parent.blob_ids.is_subset(&child.blob_ids)`). The test seeds the actual
SQLite stores itself via `synthetic_cursor_blob_id(index)`, so the ids here
mirror that generator (`{index:064x}`) and the file's job is to state the
boundary the reader is being held to.

### `cursor-cli-agent-fork.json`

Consumed by `cursor::tests::agent_fixture_keeps_distinct_sessions_and_detects_marked_fork`.

Three Cursor Agent sessions sharing one workspace, covering all three arms of
`annotate_cursor_agent_forks`:

- `…0101` — the parent. Two visible items; its title has no `" (forked)"`
  suffix, so it is never itself treated as a child.
- `…0102` — the fork. Title is `"<parent title> (forked)"`, created later, and
  its visible items are the parent's two followed by one more — satisfying
  `strict_visible_prefix`. This session must get a fork observation naming
  `…0101`.
- `…0103` — the fail-closed case. Same `" (forked)"` title, so the parent
  lookup runs, but its transcript diverges from the first item on, so no strict
  prefix exists and no observation may be emitted.

Each session carries `meta` (written verbatim as `chats/<id>/meta.json`, where
`title` and `createdAtMs` reach the synthetic metadata line) and `transcript`
(the agent-transcript JSONL records, in the
`{role, message.content[].text}` shape `cursor_agent_visible_items` reads).

### `claude-cli-job-fork.json`

Consumed by `claude::tests::interactive_fork_job_enriches_waiting_child_with_parent_and_cwd`.

- `job_state` — a `ClaudeForkJobState` in its camelCase on-disk form, written to
  `~/.claude/jobs/<child>/state.json`. It satisfies every gate in
  `interactive_fork_job_logs`: `interactiveLineage` is true, `sessionId` equals
  `forkSessionId`, the parent id differs, both ids are `[A-Za-z0-9-]`, and `cwd`
  is absolute.
- `child_transcript` — the child's `~/.claude/projects/-repo/<child>.jsonl`
  records, which discovery appends to the synthesized header. They repeat the
  state's `cwd` so the scanner's last-cwd-wins rule agrees with the header.

### `claude-desktop-session-sidecars.json`

Consumed by `claude::tests::test_discover_recent_skips_desktop_session_sidecar_files`.

Three files that all sit in one `claude-code-sessions/<workspace>/<window>/`
directory, covering each arm of `desktop_manifest_session_log`:

- `manifest` — a desktop session manifest with `cliSessionId`. This is the only
  admitted file, and it promotes the CLI transcript named by that id.
- `manifest_without_cli_session` — a manifest with `sessionId` but no
  `cliSessionId`. It names no transcript, so it holds no token records and must
  be skipped.
- `scheduled_tasks_sidecar` — the `scheduled-tasks.json` task configuration that
  the desktop app writes beside the manifests. It is not a session at all and
  must be skipped.

### `opencode-cli-production-db.json`

Consumed by `tests/opencode_tests.rs`. This is the one fixture re-homed from
`crates/analysis/tests/fixtures/session_forks/opencode/`, which the extraction
allowlist authorizes by name for this consumer. The filename is deliberately
unchanged so it stays traceable to the authorized source during review.
