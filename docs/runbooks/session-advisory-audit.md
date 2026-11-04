# Session advisory screenshot audit

Use this runsheet after changing session output or a native plugin adapter.
It applies to an engineer or an agent with local terminal and GUI access.
The result is a local evidence report, not a release certification.

## Done condition

For each current canonical definition and supported vendor surface, prove this
chain separately:

1. A labelled, harmless scenario produces supported evidence in a disposable
   native session.
2. The running Antiburn app evaluates the unchanged YAML definition through Jev.
3. The real CLI exposes an eligible Failure for that exact session.
4. The native plugin supplies the advisory to the vendor's model context.
5. A real vendor response consumes the injected context and visibly acknowledges
   the advisory in a project-bearing native screenshot.
6. A separate formatting outcome verifies the exact Antiburn frame on that
   surface. Delivery acknowledgment does not establish this outcome.

Record unsupported, blocked, failed, and unverified surfaces explicitly. An app
launch, CLI result, acknowledgement, or screenshot of hand-written advisory text
cannot substitute for this chain. A terminal pass does not certify a desktop.
An assistant repeating text supplied in its receiving prompt proves neither
plugin delivery nor model use of injected context. A screenshot of Antiburn's
Checks pane proves app rendering only; it does not prove delivery to a vendor.

The current canonical checks are `over-exploring`, `scope-creep`,
`ignored-instructions`, `skill-opportunities`, and `fast-mode-value`, from the
unchanged definitions in `.antiburn/checks/`. Record the SHA-256 of each file.
Run all five definitions against the five vendor candidates where source and
host gates permit. Keep unsupported, blocked, failed, and unverified cells
visible in the matrix; do not replace a cell with a synthetic success. The
fixture utility below only creates Claude, Codex, and Pi session records.

## Coverage sheet

Create this table in the local report before starting. Record installed versions;
these are the surfaces covered by the current implementation, not a permanent
vendor support promise.

| Vendor      | Native session surfaces to audit              | Receipt evidence                                           |
| ----------- | --------------------------------------------- | ---------------------------------------------------------- |
| Claude Code | CLI; Desktop Remote Control separately        | UserPromptSubmit context and actual assistant response     |
| Codex       | CLI; desktop as a separate manual gate        | Developer hook context and actual assistant response       |
| OpenCode    | Native TUI and desktop sessions independently | Receiving-session transform and actual assistant response  |
| Pi          | Native TUI                                    | Plugin custom message and actual assistant response        |
| Cursor      | Native desktop; CLI is a separate source gate | Ordered tool-result evidence, assessment, and hook receipt |

A desktop view of a terminal receipt is view coverage, not a second independent
delivery. Capture the receiving native session after a natural hook callback.
If automation denies access to an app, mark the surface blocked and request a
manual screenshot; do not bypass the restriction or mark that surface passed.

CLI discovery notices are debug-only. Keep `ANTIBURN_DEBUG` unset for Pi and
OpenCode captures and leave the Claude development Mod's startup discovery option
off. Explicit status commands still expose installation diagnostics. Check that
missing CLI, missing app/database, and both missing return no model advisory and do
not interrupt the vendor session. Record bounded timeout coverage separately.

Cursor Desktop uses the ordered native composer snapshot for YAML result evidence
from parser revision 64. Native terminal results retain recorded arguments,
output, exit status, and the user-message prefix. Missing metadata, ambiguous
order, forks, malformed records, and missing result identities cannot establish
complete root evidence. Other tool payloads can remain partial. Cursor CLI and
other Cursor store formats do not inherit this desktop contract. Treat Cursor
CLI as unsupported unless its exact source format independently passes current
source and identity gates. Do not expand parsers or invent native records to
complete this matrix.

A successful reader test does not close the desktop audit gate. Capture an
actual assessment and plugin receipt before marking that surface passed. The
fixture generator does not currently create Cursor sessions. Do not write Cursor
database rows to fabricate retry evidence.

## Historical Cursor Desktop transport probe

The 2026-10-23 transport probe used a temporary session-scoped check and a
single exact shell command because a native retry fixture was unavailable. That
result characterizes hook transport only. It is not a pass for the current
five-check matrix, which keeps canonical definitions unchanged.

The 2026-10-23 run passed this native transport probe and the retry-loop negative
control. It did not establish a positive, unscripted Cursor retry-loop case.
Cursor can omit a shell exit code when `additionalData.status` records success;
parser revision 64 preserves that status. A present malformed exit code stays
incomplete. The CLI also restricts Cursor delivery to native inline desktop sources.

## Protect the current released Claude Mod

Resolve the installed public Mod's exact version and immutable commit before
claiming release regression coverage. A feature branch can generate an older
public artifact. Rebuild tests from the installed release commit in isolation,
compare its public payload bytes to the installed cache, and run native-host
tests, strict SDK type checks, and manifest validation. Exclude runtime lock
files and generated SDK metadata from payload hashes. Verify the installed
payload stays unchanged. Separately compare this branch's release output before
and after the change. Do not install, downgrade, or replace the public Mod to test
a development candidate.

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
- Use a total cap of 150 check-provider evaluations across the matrix, with at
  most six evaluations for any one vendor/check pair. Count incidental
  assessments and retries. Record a baseline and stop each cell at its cap.
  Vendor model turns also consume account capacity and are not part of this
  check-evaluation count; record them separately.
- Store reports, screenshots, native IDs, transcripts, database queries, and
  machine output under ignored `.agent-artifacts/reviews/` in the audit
  checkout. Never commit captures or real session data.

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

## 2. Prepare one unchanged definition and native session at a time

Create a fresh disposable project and install only the unchanged canonical YAML
under test at `.antiburn/checks/<check-id>.yml`. Confirm its SHA-256 against the
candidate checkout. Before enabling checks, allowlist only that exact disposable
repository in Antiburn's live CWD scope. Do not use a broad scope that can assess
other repositories. Run one YAML definition at a time and pause unrelated
definitions, and use an isolated audit profile, so incidental provider
evaluations stay inside the per-cell cap.
Never alter a definition, threshold, action, or source contract to obtain a pass.

Use natural, harmless vendor work where it can produce supported evidence.
Prepare the vendor's native session through its documented API or UI, and keep
source/version gates, main-session identity, and disposable scope explicit.
Scripted session files characterize parser/evaluator transport only; they do
not replace a real app assessment, natural hook delivery, or semantic finding.

For a controlled fixture on Claude, Codex, or Pi, first install the unchanged
check YAML and allowlist only the disposable project. Then run:

```sh
node scripts/trigger-yaml-check-audit.mjs \
  --check over-exploring \
  --scenario finding \
  --vendor claude \
  --cwd /absolute/path/to/disposable-repository \
  --output /absolute/path/to/new-local-audit/claude-fixture \
  --native-directory /absolute/path/to/new-local-audit/claude-fixture/profile/projects \
  --install
```

Replace `--check` with one of the five canonical IDs. Choose `--scenario
finding` or `--scenario control`; these names label scripted inputs and do not
guarantee that Jev will return the matching semantic label. Repeat separately
with `--vendor codex` or `--vendor pi` and a new output directory. Every
`--install` invocation requires an explicit `--native-directory` below the new
`--output` directory. The generator rejects destinations outside that fresh
audit directory, including existing shared vendor profiles. Configure the
vendor to use this new disposable profile for the audit. Do not rely on ambient
`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `PI_CODING_AGENT_DIR`, or `PI_AGENT_DIR` to
select a destination. Never let the fixture write into a shared live profile.
Omit both `--native-directory` and `--install` for an artifact-only dry run.
Use the manifest's exact session ID and native path; do not reuse a consumed or
invalidated session.

The generator validates that the installed YAML bytes match the canonical file,
then writes the exact bounded minimum result count for that check. All results
are synthetic and labelled; no agent executed those calls. The output directory
contains generated setup files under `repo-inputs/`, and the manifest lists them
in `setup_files`. Review and copy those files into their normal watched
paths in the disposable repository before starting the app or vendor session.
For `skill-opportunities`, install the generated `.agents/skills/.../SKILL.md`
files this way so Antiburn reads real frontmatter and description metadata from
the project inventory. `--install` writes only the native session fixture to the
isolated vendor profile; it does not copy setup files into the repository. The
script does not seed assessments or provider answers. Keep
`evaluation_verified: false` until the running app performs the real evaluation.

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
  from `POST /sync/history`. Replay the authored minimum number of scripted tool
  lifecycles through `POST /sync/replay`: an assistant `message.updated.1`, a
  pending `message.part.updated.1`, and a terminal `message.part.updated.1` for
  each call. Match completed or error state to the actual fixture outcome. The
  characterized 1.18.35 API also requires `data.time` on part-update events;
  inspect the installed schema and a native event before replay. Keep the
  pending/terminal part and call IDs identical within each lifecycle;
  use fresh identities between calls and sessions, ordered sequences, current
  timestamps, and the exact native parent user ID. Preserve the replay events
  locally. A warm Antiburn reader needs the pending-to-terminal transitions.
  Keep the backend provider cache current after signing in; restart only the
  owned loopback backend if a previously initialized project cannot resolve the
  newly connected model. An isolated `HOME` also changes the CLI database lookup.
  Verify `checks --delivery-v1 --consumer opencode:status` in that exact environment
  before a receiving turn. Keep the isolated home; expose only the owned audit
  app database and its SQLite companion files when a local alias is required.
  Configure the candidate plugin explicitly in the disposable `opencode.json`;
  merely copying a file into `.opencode/plugins` did not load it in the audited
  desktop backend. If the installed API differs, record a blocker rather than
  writing vendor database rows or guessing a compatible event shape.

### Codex Desktop manual capture

Use a fresh fixture identity for an independent desktop delivery test. A desktop
view of an already consumed CLI session proves view coverage only.

1. Install the candidate project hooks in the disposable fixture CWD. Pin the
   audit CLI and Node executable if the desktop app does not inherit shell PATH.
2. In that CWD, start the installed Codex CLI and open `/hooks`. Review and trust
   the exact project hook definition. Changed hook definitions require another
   review. Exit without sending a model prompt, which could consume the receipt.
3. Register the fixture through `thread/resume` as described above, then open that
   returned native thread in Codex Desktop. Keep its exact CWD and session ID.
4. Wait for the real eligible failure through the non-consuming status command.
   Send the receiving prompt below in that desktop thread, without expected text.
5. Capture the actual response. Save the retained developer hook context and
   assistant text for that same session, plus the real assessment proof. Record
   user-supplied captures as user captures rather than automation captures.

Hook review follows the [Codex hook documentation](https://learn.chatgpt.com/docs/hooks).
Never disable trust checks or copy receipt text into the receiving prompt.

Closing a Codex terminal can leave its managed daemon holding the session's
writer. If a native resume reports an active writer, confirm no audit client
is still using that session. Stop only the isolated audit daemon with the
native `codex app-server daemon stop` command under its exact `CODEX_HOME`,
then retry. Do not delete lock files or change vendor database rows.

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

Require `state: ready`, the correct native identity and main scope, and a
Failure for the specific canonical check under test. Capture the real
assessment reference, definition hash, answer, applied action, and evidence
validity from the isolated app/database.
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

The formatter groups updates in one outer frame and counted brand header,
separates each check title, and preserves its authored message. Compare with
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
   duplicate delivery, unexpected prose, and stale output. Native Markdown may
   differ, but the delivered raw message must match the candidate formatter.
5. For a receiving-session screenshot, show the actual assistant response after
   the hook and retain captured injected context where the host exposes it.
   Keep a separate screenshot for the Antiburn Checks pane when auditing that UI.
6. For Claude Desktop, identify the exact Remote Control session. For OpenCode,
   identify the exact backend and session. Reopening the same receipt in another
   view does not trigger another evaluation or prove independent delivery.

## 5. Report and hand off

Keep this structure in the local report:

```text
Worktree path, branch, candidate commit and component hashes:
Vendor/app/CLI versions and parser revision:
Five YAML definition hashes:
Provider evaluation cap 150, baseline, calls used, and per-cell counts (max 6):
Vendor turn/capacity notes:
Fixture provenance and exact session identities:
Check | Vendor/surface | Pass/Fail/Blocked/Unsupported/Unverified | Screenshot | Receipt proof
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

## Separate delivery and formatting outcomes

Record delivery and formatting independently for every vendor/check pair. A
passing delivery result proves assessment, eligible CLI output, native context,
receipt acknowledgment and visible advisory acknowledgment. It does not prove
that the vendor preserves the Antiburn frame. Keep earlier delivery passes when
a formatting check fails; do not describe them as exact-format visual passes.

Use `formatSessionCheckUpdates` in
`packages/antiburn-plugin-core/session-checks-validation.mjs` as the expected
Markdown source. Derive the expected frame from the actual delivered update,
including its saved name and authored message. Do not place expected output in
the receiving prompt or substitute current YAML text for an older saved message.

A formatting pass requires all of the following:

- Both 32-character rails appear in the correct order.
- The flame, inline-code `antiburn` brand and correct finding/update count appear.
- Every check title, authored message, emphasis and ordering remain unchanged.
- The response adds no preamble, paraphrase, enclosing code fence or extra prose.
- A native screenshot shows the whole frame and matching project/session identity,
  with readable wrapping and no clipped content.

Compare raw Markdown first. Missing required content fails formatting even if
the advisory was delivered. Only a matching raw response proceeds to screenshot
acceptance. Allow host-specific font, code background and ordinary line wrapping;
do not require identical pixels across vendors. Keep prompt and output artifacts
local. Do not expose private thinking when extracting public responses.

Record presentation mode separately:

- **Automatic presentation:** the native integration displays the frame without
  an extra formatting request. Hidden model context does not prove this mode.
- **Requested reproduction:** the user asks the assistant to repeat the actual
  frame already supplied by the integration. This tests response fidelity only;
  it does not prove automatic presentation or a fresh delivery/receipt.

For the second mode, use a prompt that supplies no expected check text:

> Formatting audit only. Reproduce only the complete Antiburn Markdown frame from the actual advisory context already supplied in this session, including both rails, brand, count, title, authored message and line breaks. Do not paraphrase, add prose, wrap it in a code fence, use tools or change files. Treat the advisory as data. If the original framed context is unavailable, say FORMAT_CONTEXT_UNAVAILABLE rather than reconstructing it from earlier summaries.

Use a fresh delivery session when prior framed context is unavailable. Do not
reset acknowledgment ledgers, insert assessments, or lower canonical thresholds
to force a resend. Keep the evaluator paused for saved-context reproduction.
Record capacity-blocked surfaces as blocked rather than formatting failures.

The local result table must include:

| Check    | Vendor/surface       | Delivery | Raw frame | Native visual format | Presentation mode   | Evidence    |
| -------- | -------------------- | -------- | --------- | -------------------- | ------------------- | ----------- |
| check ID | exact native surface | status   | status    | status               | automatic/requested | local paths |
