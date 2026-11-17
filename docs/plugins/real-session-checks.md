# Real YAML session-check acceptance

Use the existing **Failed retry loop** definition to check native advisory
delivery through the real Antiburn app and CLI. Qualify each host version and
surface separately. One successful host does not establish cross-vendor support.

For the reusable engineer and agent procedure, follow the
[session advisory screenshot audit runsheet](../runbooks/session-advisory-audit.md).
It includes clean framing, native continuation, evidence, and coverage gates.

## Setup

Use matching candidate app, CLI, and `antiburn-core-plugin` builds. The app and
CLI must read the same database. Remove synthetic CLI overrides. A local
launcher may forward arguments to the real CLI with an explicit audit config;
it must not manufacture results or acknowledgements.

Use a disposable Git repository and install the unchanged repository definition
from `.antiburn/checks/failed-retry-loop.yml` into its project checks directory.
Configure the real Jev decision connection and enable Model-based Checks through
the app. Limit evaluation scope to the disposable repository. Do not copy a user
database, insert assessment rows, or store credentials in fixtures.

## Acceptance

1. Start a fresh Claude Code main session with the packaged native hooks.
2. Confirm Antiburn discovers its actual session ID and exact CWD, loads the
   definition, and enables the correct live scope.
3. Run a harmless failing task and inspect the recorded evaluation. Do not ask
   the agent to repeat failures: requested retries are an authored clear case.
4. Confirm the real evaluator persists an eligible assessment. A positive case
   requires three matching unresolved failures, no correction or requested retry,
   and the unchanged authored confidence threshold.
5. When a failure assessment exists, continue the session across its next native
   callback and verify the authored advisory reaches agent context.
6. Capture the definition revision, assessment reference, CLI result, actual
   delivery, and acknowledgement in local evidence. An ACK alone does not prove
   model consumption. A passing assessment alone does not prove advisory delivery.

If no qualifying positive case occurs, record it as unverified. Do not weaken the
check or replace the evaluator to obtain a pass. CLI reads remain side-effect
free; the app's normal evaluation cycle produces the result.

Cap the initial run at 24 Jev evaluations and reassess before further calls.
Provider-backed checks and vendor turns use the configured provider allowance.
Keep private sessions outside evaluation scope. Keep screenshots, native IDs,
transcripts, and machine output in local audit artifacts rather than Git.
Disable temporary activations and stop owned backends after capture.

## Repeatable visual trigger

For a controlled visual audit, generate clearly labelled scripted native evidence:

```sh
node scripts/trigger-yaml-check-audit.mjs \
  --cwd /absolute/path/to/disposable-repository \
  --output /absolute/path/to/new-audit-directory \
  --install
```

The repository must already contain the unchanged definition and be enabled in
Antiburn. Omit `--install` for an artifact-only dry run. Each run creates a new
session with three matching failed tool results. The script does not insert an
assessment or manufacture a provider response. The manifest records the definition
hash and labels evaluation as unverified; capture the actual assessment separately.

This fixture emulates the characterized Claude CLI **2.1.278** format. It proves
the evaluation and delivery path with controlled evidence, not spontaneous retry
behavior or compatibility with newer Claude recordings. The continuation
contract below also covers characterized 2.1.309 inputs. Do not relabel
recordings from another version as 2.1.278. The fixture manifest, not the
resumed agent's prose, identifies scripted evidence.

## Cross-vendor fixture audit

Select `--vendor claude|codex|pi` with the trigger script. Claude is the default.
Each format is characterized independently; installing a transcript proves neither
vendor resumability nor current-version compatibility. Pi uses its configured
`PI_CODING_AGENT_DIR` or `PI_AGENT_DIR` and the native project-directory form.
Codex v0.163.0 requires producer metadata and a registered thread to resume by
UUID. The fixture includes an explicit scripted originator and model provider.
Register the installed rollout through the native app-server `thread/resume` API
with its absolute `path` and experimental API capability enabled. Preserve the
returned thread ID for terminal resume. Do not insert rows into the vendor's
state database to manufacture resumability.

OpenCode uses its native session and replay APIs for this audit. For a warm
reader, replay pending and terminal tool states with stable identities. Explicitly
configure the candidate plugin in the disposable project's `opencode.json` and
verify the backend loaded it; placing a file in `.opencode/plugins` alone did not
load it in the audited desktop backend.

Keep timestamps and the validity snapshot with each local capture.
These scripted retry fixtures exercise the real evaluator; they do not prove
spontaneous agent retry behavior, recovery clearance, or all-vendor acceptance.

## Continuation compatibility

The retained Claude root contract accepts the characterized 2.1.309 CLI
continuation alongside 2.1.278. Other versions remain rejected. Pi can advance
lineage through inert `custom` and `custom_message` entries; signal-bearing custom
rows remain unavailable. Both the native reader and the live continuation gate
apply this rule. The fixture supplies zero usage counters so Pi's native
terminal can render scripted assistant records. Check continuation with fresh
live captures after parser changes. An earlier invalidated assessment does not
become valid after an upgrade. For OpenCode, reopen the actual receiving session
to inspect receipt history rather than creating another session for a screenshot.

Keep the candidate app and CLI on the same parser revision. After a revision
change, dormant native delivery cursors refresh after real session activity and
the normal discovery/service cycle. Wait for the CLI status to become ready
before testing advisory receipt. A prior invalidated assessment is not restored
by a screenshot or a new terminal view.

Codex project-local hooks are disabled until the disposable project is trusted.
Review the single packaged Antiburn hook through the host's normal review screen;
do not bypass hook trust. A generated rollout alone is not receipt proof.
