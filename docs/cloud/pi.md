# Pi session capture

The [capture library](capture.md) can package one explicitly authorized Pi
session file on Unix. The library does not discover Pi sessions or enable Pi
upload in the desktop app. The caller owns source access, full-content consent,
and scheduling. Run library calls on a bounded blocking worker.

## Source identity and folder permission

`reopen_pi(path, expected_id, limits)` selects one absolute Pi session filename.
Pi uses `<timestamp>_<id>.jsonl` names. The part after the last underscore
must exactly match the expected ID. An ID at the end of a longer filename
component does not match. The caller must authorize the path as a Pi source.
The library does not search provider roots or distinguish other producers that
use the same layout.

The first physical line must be a complete JSON object with `type: "session"`
and the expected `id`. The line must fit within 64 KiB, including its newline.
The filename and later records cannot prove ownership. The file must be regular
with one hard link. No path component may be a symlink.

`Candidate::in_source_scope` binds `pi/<id>` to the caller's stable source
scope. `Candidate::project_evidence()` uses only the first header's `cwd`.
The encoded workspace directory, `parentSession` path, later CWDs, and paths in
tool arguments grant no project permission. A missing or invalid CWD leaves the
session ineligible for folder-selected capture. Apply `ProjectSelection::admit`
to test the recorded CWD against approved folder trees.

## Export and evidence limits

Capture pins the file generation before reservation. After reservation, capture
rechecks the pinned file's first header and any approved folder policy before
spooling bytes. A file change during capture interrupts it. If the spool has an
interrupted entry, cancel it before using a new capture ID for a later generation.

The `transcript` stream, with role `native_transcript` and schema `pi-jsonl`,
retains the full file within `Limits::source_bytes` (512 MiB by default).
It preserves physical order, all recorded branches, tool calls and results,
unknown fields, original line endings, and invalid or incomplete bytes after the
header. It does not select a branch, repair the body, or follow parent paths,
attachments, external files, or URLs.

The [metadata stream](capture-metadata.md) uses `agent: "pi"` and
`source_format: "PiJsonl"`. The first header supplies `native_session_id`.
Branch, message, and tool IDs do not prove independent worker IDs.
`native_agent_id` and `source_agent_version` are null. The header's numeric
`version` describes the session format, not a historical runtime.
`version_observations` stays empty and both version completeness flags are
false. `relationship` and `history_completeness` stay unknown.

The [synthetic capture fixture](../../crates/antiburn-cloud-capture/tests/fixtures/pi)
uses the header and branch shape from the
[pinned Pi V3 fixture contract](../../crates/antiburn-local/tests/fixtures/pi_characterization/README.md#core-retained-v3-contract).
Capture uses only header identity and CWD. It does not validate the body or
claim semantic support for all Pi versions or extensions.
