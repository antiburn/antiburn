# Session advisory screenshot audit

Use this runsheet after changing session output or a native plugin adapter.
It applies to an engineer or an agent with local terminal and GUI access.
The result is a local evidence report, not a release certification.

## Done condition

For each supported vendor, prove this chain separately:

1. Labelled retry evidence enters a disposable native session.
2. The running Antiburn app evaluates the unchanged YAML definition through Jev.
3. The real CLI exposes an eligible Failure for that exact session.
4. The native plugin supplies the advisory to the vendor's model context.
5. A real vendor response renders the current output in a screenshot.

Record unsupported, blocked, failed, and unverified surfaces explicitly. An app
launch, CLI result, acknowledgement, or screenshot of hand-written advisory text
cannot substitute for this chain. A terminal pass does not certify a desktop.

## Coverage sheet

Create this table in the local report before starting. Record installed versions;
these are the surfaces covered by the current implementation, not a permanent
vendor support promise.

| Vendor      | Terminal                                | Desktop                                 | Receipt evidence                                        |
| ----------- | --------------------------------------- | --------------------------------------- | ------------------------------------------------------- |
| Claude Code | Native CLI                              | Desktop Remote Control view             | UserPromptSubmit hook context and actual assistant text |
| Codex       | Native CLI                              | Separate manual gate                    | Developer hook context and actual assistant text        |
| OpenCode    | Native TUI                              | Native desktop                          | Actual assistant text in the receiving native session   |
| Pi          | Native TUI                              | Not applicable                          | Antiburn custom message and actual assistant text       |
| Cursor      | Unsupported by the current YAML runtime | Unsupported by the current YAML runtime | Do not infer support from plugin installation           |

A desktop view of a terminal receipt is view coverage, not a second independent
delivery. If automation denies access to an app, request a manual screenshot;
do not bypass the restriction or mark that surface passed.

## 1. Prepare and bound the run

- Record the candidate commit, working-tree state, app/CLI binary hashes, plugin
  artifact hashes, vendor versions, parser revision, and YAML hash.
- Use matching candidate app and CLI builds against one isolated audit database.
  Follow [isolated desktop setup](../debugging.md). Do not copy a user database.
- Keep the app bundle and CLI in stable local paths. Build-cache eviction must
  not remove a running audit binary. Remove synthetic CLI overrides.
- Use only a disposable Git repository with non-private fixture content. Enable
  Model-based Checks and its exact live CWD scope through the audit app.
- Configure Jev through the normal credential flow. Never print, copy into the
  report, or commit credentials. Keep unrelated repositories outside scope.
- Agree a provider-call cap before starting. The initial campaign used 24 Jev
  evaluations across all rounds, not 24 per vendor. Record the baseline and stop
  before exceeding the agreed allowance. Vendor turns also consume allowance.
- Store captures, native IDs, transcripts, database queries, and machine output
  outside Git in a local audit directory. Commit only reusable instructions and
  deliberately synthetic repository fixtures.

Build the candidate from the checkout being audited:

```sh
node scripts/assemble-antiburn-core-plugin.mjs
node scripts/assemble-antiburn-core-plugin.mjs --verify
```

Use generated `antiburn-core-plugin` artifacts, not a stale installed copy. A
long-running OpenCode backend must reload after artifact changes. Restart only
the owned audit backend. Never stop an engineer's unrelated sessions.

Follow [development adapters](../plugins/development-adapters.md) for candidate
loading. Its general visual-audit launcher checks installation and lifecycle;
it does not trigger the real YAML acceptance test described here.

## 2. Create a fresh trigger per vendor

Copy the unchanged `.antiburn/checks/failed-retry-loop.yml` from the candidate
checkout into the disposable repository at the same relative path. Confirm its
hash matches. Do not lower the threshold or add an easier audit check.

For Claude, Codex, and Pi, run from the source checkout:

```sh
node scripts/trigger-yaml-check-audit.mjs \
  --vendor claude \
  --cwd /absolute/path/to/disposable-repository \
  --output /absolute/path/to/new-local-audit/claude-fixture \
  --install
```

Repeat with `--vendor codex` and `--vendor pi`, each with a new output directory.
Omit `--install` for an artifact-only dry run. Use the returned manifest's exact
session ID and native path. Do not reuse a consumed or invalidated session to
certify a new formatter.

The generator writes three labelled failed tool results. These are scripted
native evidence, not commands executed by an agent. It seeds neither assessments
nor provider responses. Preserve the manifest's `evaluation_verified: false`;
subsequent real evaluation proof belongs in a separate record.

### Native continuation details

- **Claude:** resume the generated session with the candidate hooks loaded. The
  fixture emulates 2.1.278; characterized continuation support includes 2.1.309.
  Do not relabel a newer recording to bypass a parser rejection. Enable Remote
  Control with a unique audit title when a desktop view is required.
- **Codex:** a rollout file alone may not be resumable by UUID. Register it through
  the installed native app-server `thread/resume` API with its absolute `path`
  and experimental API enabled. Use the returned ID. Do not modify vendor state
  database rows. Review and trust only the packaged hook through the normal host
  UI when required; never disable hook review to make the audit pass.
- **Pi:** use the manifest's `native_file` with `--session` and explicitly load
  the generated Pi extension. Disable unrelated extensions and tools for the
  receiving turn when supported. The fixture uses Pi V3 JSONL; installed-version
  continuation still needs actual receipt proof.
- **OpenCode:** the trigger script does not generate this vendor. Use an owned,
  loopback-only backend and its installed API schema. Create a native session
  through `POST /session`, then a labelled initial user message with
  `POST /session/{sessionID}/message` and `noReply: true`. Obtain its sequence
  from `POST /sync/history`. Replay three scripted tool lifecycles through
  `POST /sync/replay`: an assistant `message.updated.1`, a pending
  `message.part.updated.1`, and an error `message.part.updated.1` for each call.
  Keep the pending/error part and call IDs identical within each lifecycle;
  use fresh identities between calls and sessions, ordered sequences, current
  timestamps, and the exact native parent user ID. Preserve the replay events
  locally. A warm Antiburn reader needs the pending-to-terminal transitions.
  Configure the candidate plugin explicitly in the disposable `opencode.json`;
  merely copying a file into `.opencode/plugins` did not load it in the audited
  desktop backend. If the installed API differs, record a blocker rather than
  writing vendor database rows or guessing a compatible event shape.

Do not instruct a real agent to repeat failures to manufacture a positive case:
requested retries are a clear case under the authored definition. Scripted
fixtures exercise evaluation and delivery; unscripted retry detection is a
separate acceptance case.

## 3. Wait for the real assessment

Let Antiburn's normal discovery and evaluation cycle run. Read status without
consuming the plugin's delivery:

```sh
/path/to/real/antiburn checks SESSION_ID --delivery-v1 --consumer HOST:status
```

Use the adapter host name: `claude-code`, `codex`, `pi`, or `opencode`. If using an
isolated CLI config, apply it to this command and the plugin's executable wrapper.
The wrapper may forward arguments to the real CLI; it must not synthesize JSON.

Require `state: ready`, the correct native identity and main scope, and a Failure
for `failed-retry-loop`. Capture the real assessment reference, definition hash,
answer, applied action, and evidence validity from the isolated app/database.
Query the database read-only; never insert or restore assessment rows.

After native continuation, confirm the assessment remains valid and the native
reader has caught up to the latest source generation. Check parser revision,
pending/block state, and delivery errors where available. A parser revision
change can require fresh native activity and the normal service cycle. Do not
spam prompts while status is waiting. An invalidated assessment is not restored
by reopening its terminal; use a new fixture after correcting the actual cause.

## 4. Receive and capture

Submit this prompt once after readiness, without including the expected output:

> Show only the new Antiburn advisory exactly as provided, preserving its Markdown layout. Do not add a code fence, preamble, or explanation. If none arrives, say so. Do not use tools or modify files.

The current single-finding layout has one outer frame, a counted brand header,
a separate check title, and the unchanged authored message. Compare with
`formatSessionCheckUpdates` in
`packages/antiburn-plugin-core/session-checks-validation.mjs`; use the candidate
formatter as the source of truth when the layout changes again.

Capture each applicable native surface after the actual response completes:

1. Use a fresh, uniquely titled audit session. Hide unrelated sidebars and keep
   the advisory legible. Collapse verbose tools or scroll to the latest response
   where possible. Trigger error rows may remain visible: label them as intentional
   scripted fixture evidence, not plugin failures or repository test regressions.
2. Preserve vendor identity and enough window context to establish the surface.
   Capture the native view without replacing text, editing the image, or pasting
   a fabricated assistant response. Save full evidence separately from framing.
3. Save actual injected context where the host retains it, plus the actual
   assistant response. An ACK proves receipt bookkeeping, not model consumption.
4. Record wrapping, clipping, rail rendering, header count, bold/code formatting,
   duplicate delivery, unexpected prose, and any stale output. Native Markdown
   appearance can differ; raw markup and message content must match the candidate.
5. For Claude Desktop, identify the exact Remote Control session. For OpenCode,
   identify the exact backend and session. Reopening the same receipt in another
   view does not trigger another evaluation or prove independent delivery.

## 5. Report and hand off

Keep this structure in the local report:

```text
Candidate commit and component hashes:
Vendor/app/CLI versions and parser revision:
YAML definition hash:
Provider budget, baseline, calls used:
Fixture provenance and exact session identities:
Surface | Pass/Fail/Blocked/Unsupported/Unverified | Screenshot | Receipt proof
Real assessment references, answers, actions, and evidence validity:
Generation catch-up and delivery errors:
Visual findings:
Unclosed gates:
Cleanup performed and owned sessions intentionally left open:
```

Include a local screenshot gallery or absolute image links and verify it opens.
Separate real delivery claims from scripted trigger provenance. Never promote a
blocked desktop capture to a pass based on a CLI screenshot. Do not claim recovery
clearance, spontaneous retry behavior, remote-host coverage, or unsupported
vendors from this test.

Retain only needed local evidence. Disable temporary evaluation scope and stop
owned backends when the demonstration no longer needs them. Do not delete the
engineer's sessions, global plugins, credentials, or databases. If sessions stay
open for review, state that explicitly and retain the remaining evaluation cap.

For historical acceptance and compatibility details, see
[real YAML session checks](../plugins/real-session-checks.md).
