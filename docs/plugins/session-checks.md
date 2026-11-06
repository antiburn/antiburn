# Predefined checks in native sessions

Antiburn evaluates the installed YAML checks. Native plugins read its saved
results through the existing CLI and add advisory messages at a natural session
boundary. The development Claude mod reads the same results for its Checks pane.
The mod no longer detects problems from session events itself.

```mermaid
flowchart TD
    YAML["Predefined YAML checks"] --> App["Antiburn evaluates"]
    App --> Results["Saved local results"]
    Plugins["Vendor plugins"] -->|"Request session updates"| CLI["Antiburn CLI"]
    CLI -->|"Read only"| Results
    Results -->|"Findings and withdrawals"| CLI
    CLI -->|"Return updates"| Plugins
    Plugins -->|"Next natural boundary"| Context["Agent context"]
    Plugins -->|"Claude mod"| Pane["Existing Checks pane"]
```

There is no HTTP endpoint or local IPC service. Reading results does not start
the desktop app, discover YAML, evaluate a provider, or modify the product
database. The app must run and be configured to produce new assessments.

## Result eligibility and freshness

The app persists its winning definition metadata alongside activation state.
The first completed discovery snapshot has an explicit marker; before that,
clients wait. An initialized empty snapshot means no applicable configured
checks. The shared `antiburn-checks-store` library applies definition availability,
revision, generation, scope, source readiness, and evidence-validity gates.
The desktop report shares those gates and the authored-status message parser.

The CLI resolves the supplied session ID in the database and rejects missing or
ambiguous identities. Delivery covers local native main sessions only. It
excludes remote sessions, WSL, forks, and known subagents. Native adapters also
reject unsupported invocation ownership; a root database identity alone cannot
identify an event that a host attributes to its parent session.

`freshness_evaluated` remains false. These are eligible **saved** results, not
proof that Antiburn is running, that definitions on disk are unchanged, or that
an assessment covers the latest activity. Context messages state this limit.
A successful assessment covers its assessed work; it does not prove repair,
whole-session cleanliness, verification, or savings.

Unavailable, unsupported, disabled, unconfigured, waiting, missing-database,
and incompatible-schema states do not establish a clean result. A failed read
must not clear a previously displayed finding as if recovery occurred.

## Delivery protocol

`antiburn checks SESSION` retains its existing JSON envelope and filtering.
Predefined YAML findings add the `predefined_yaml` source. Ordinary reads still
return only new or changed problematic results, and never raw evidence.

Native adapters use `--delivery-v1 --consumer HOST:context`. The versioned
response contains a snapshot and meaningful `finding`, `cleared`, or
`invalidated` updates. Repeated passing assessments remain silent. Assessment
times and checking activity can update the pane without repeating advice.

Consumers acknowledge a receipt at their native handoff boundary. Command hooks
acknowledge after the stdout write completes. Pi schedules an acknowledgement after constructing the callback result.
OpenCode mutates the host message array before scheduling its acknowledgement. These APIs do not
confirm host consumption or model acceptance.
Pending batches can replay before acknowledgement, including after a crash.
An obsolete receipt cannot overwrite newer pending state. If acknowledgement
is lost, a later withdrawal still accounts for advice that may have been
handed off. Native host acceptance is not transactional with local receipt
storage; the support record must state the handoff point each API provides.

Context, pane, and explicit status consumers are independent. Status consumers
ending in `:status` do not stage or acknowledge updates. Separate locked CLI
state stores fingerprints and receipts, not evidence or authored messages.
The CLI never migrates or writes the product database.
At the 1,024-consumer bound, new consumers replace the least recently used
settled consumer, ordered by its last delivery-state change. Unchanged polls and
duplicate acknowledgements do not write state or refresh this order. Pending
findings and withdrawals remain protected. If every
consumer has an outstanding handoff, delivery fails closed until one settles.
Eviction can cause an old session to receive its saved finding again; recent
retained consumers keep acknowledgement and deduplication state.

Subprocess execution uses argument vectors and bounded input, output, lock,
and execution limits. Malformed or incompatible responses fail closed.
Disconnected adapters do not activate local detectors. Plugin reads do not
create evaluation requests or model turns.

## Automatic native display in Claude and Pi

Claude's synchronous UserPromptSubmit and PostToolUse hooks include a plain-text
update frame in top-level `systemMessage`, alongside the existing Markdown advisory
`additionalContext`. The native hook notice does not render Markdown, so its frame
omits Markdown delimiters and escaping. A leading newline separates the opening
rail from the host's hook prefix. Claude Code 2.1.317 has native visual
notification evidence; 2.1.318 emits the message as a native informational notice
in stream JSON. The host owns warning presentation. Async hooks do not provide
this user-facing behavior and are not used. Markdown-capable surfaces retain the
Markdown frame. Cleared and withdrawn updates retain a readable check name when
supplied; when only the finding ID remains, they use “Previous advisory” instead.

Pi returns one custom message with `display: true` in an interactive TUI with
`hasUI`. Its `content` keeps the full advisory guardrail and canonical Markdown
frame in model context. A separate optional renderer extension reads the same frame from
renderer-only `details` and displays just that frame, without Pi's default
custom-message label or the guardrail paragraph. Headless and RPC operation
retain the original hidden context. The renderer uses Pi's host-provided TUI
packages; the package declares them as peer dependencies. If that optional
extension cannot load or the host lacks the renderer API, the context extension
still delivers the full message and Pi uses its default rendering. Saved messages
without renderer metadata also fall back to their full content. A renderer probe
does not establish a captured native session.

Both paths use the existing `HOST:context` consumer and receipt because each
native result carries context and display together. No second UI fetch repeats
the finding. A receipt confirms only the documented handoff boundary, not pixels,
model consumption or human attention. The CLI deduplicates acknowledged revisions;
changes, clearing and invalidation follow the existing update protocol. Pending
unacknowledged handoffs can replay. Clear and withdrawal messages append updates;
they do not remove historical transcript entries. Read failures stay silent.

Native notification and complete-frame visibility require separate session
captures. The support table below retains its original context-only scope.
These display paths do not change evidence parsing, check eligibility, thresholds,
or automatic-installation gates.

### Codex notice output

By default, the Codex UserPromptSubmit adapter includes a plain-text update frame in
`systemMessage`, alongside its existing Markdown `additionalContext`. It uses
the same context receipt and writes both fields in one hook result. The
[Codex hook contract](https://developers.openai.com/codex/hooks) defines
`systemMessage` as a warning surfaced in the UI or event stream. The host owns
the warning layout; the adapter does not ask the assistant to reproduce it.

Codex CLI 0.166.0 `exec --json` accepted and acknowledged the adapter's context
in an isolated synthetic transport probe, but did not expose its notice in the
captured output. A separate native TUI capture showed the complete plain-text
frame after an ordinary readiness prompt, separately from the assistant's reply.
That probe used a synthetic CLI response and localhost provider; it establishes
automatic native presentation alone. A follow-up controlled audit delivered a
real saved predefined Fast mode value assessment through the same TUI hook after
an ordinary readiness prompt. A second ordinary turn showed no new notice; the
CLI retained one acknowledged revision and returned no new update or receipt.
The assessment used controlled fixture evidence, and the receiving provider
returned only a local canned reply. This verifies saved-assessment delivery,
native presentation and duplicate suppression, not natural-work detection or
assistant reproduction. This native-hook path has not produced a visible notice
in the tested desktop conversation. Do not infer headless or desktop display
support from the hook output fields alone.

For desktop projects that need an inline notice, the adapter accepts the explicit
option `--presentation assistant`. Append this option to the packaged
`session-check-hook.mjs` command in the project's reviewed hook configuration;
keep only one Antiburn UserPromptSubmit hook for that project. Do not add the
option to a shared global hook unless assistant rendering is also wanted in
terminal sessions using that configuration. Omitting the option (or using
`--presentation native`) preserves the native terminal path. The configuration
preview and bundled hooks continue to select native presentation by default;
there is no automatic desktop detection.

Assistant presentation supplies a fixed instruction to show the canonical
Markdown frame after the ordinary answer on that turn. The frame is JSON-encoded
as advisory data, with instructions to ignore commands within it and honor
conflicting explicit user instructions. This mode omits `systemMessage` to avoid
a second native notice. It uses the existing context receipt and deduplication
ledger: switching presentation modes does not replay an acknowledged revision.
Acknowledgment proves hook handoff, not that the assistant rendered the frame.
The assistant can omit or alter it; this is not a deterministic native widget.

A cross-surface audit of this opt-in adapter displayed a real saved terminal
assessment as the exact branded Markdown frame in Desktop. A subsequent ordinary
turn did not repeat the notice. The assessment used controlled terminal fixture
evidence; freshness was not re-evaluated. The hook handed off the warning before
a direct Desktop request failed with an input-item-ID error. An ordinary prompt
sent through Codex's dedicated chat tool then recovered the response from retained
hook context, without a request to reproduce Antiburn output or a receipt reset.
This verifies that recovery path, not a fix for direct Desktop submission or
fresh Desktop-origin assessment support. A failed model request can occur after
the hook acknowledges delivery; acknowledgment does not guarantee a visible
notice or an automatic retry.

### OpenCode optional native TUI candidate

OpenCode's server adapter retains its existing context transformation. A separate
optional TUI module, `runtime/session-check-ui.tsx`, targets exactly OpenCode
1.19.3 and `@opencode-ai/plugin/tui`. It registers a persistent `app_bottom`
frame without replacing the prompt. It prefers the public OpenTUI Markdown
intrinsic with canonical brand, check titles, authored multiline messages and
rails. Styles use public host theme tokens; each native style resource is
destroyed on unmount. A rendering error falls back to the complete canonical
plain-text frame. Links are passive and remain closed. A bounded scrollbox
wraps the text and retains its full content. Long frames require scrolling.
Actual OpenCode 1.19.3 PTY captures establish synthetic CLI transport, complete
short frames, long-message wrapping and scrolling, A/B/A session isolation,
home-route removal and one UI acknowledgement per session revision.
A native terminal-window screenshot also verifies a complete short synthetic
frame, Markdown styling and exact session identity, correlated with one UI
acknowledgement.
A separate actual packaged TUI probe confirms canonical Markdown with
native bold/link styling, multiline wrapping and scrolling, plus complete
plain-text fallback after an injected Markdown failure. It also observes native
style resource cleanup.

A live OpenCode 1.19.3 audit used a disposable project and one ordinary
diagnostic turn. The unchanged Skill opportunities check scored 0.96 at its
0.95 threshold. Its complete warning appeared automatically below the composer
in the attached terminal. The real CLI recorded the matching `opencode:ui`
acknowledgement. This verifies live display for one session through the
explicit loopback serve/attach path.

OpenCode Desktop 1.19.4 has no GUI plugin host for this renderer;
its native desktop window is unsupported by this terminal TUI candidate.

Install the server entry through its existing configuration. Configure the TUI
entry separately in `tui.json`, using an absolute path to the assembled file:

```json
{
  "plugin": ["/absolute/path/to/opencode/runtime/session-check-ui.tsx"]
}
```

The package exposes this target as `./tui`; its root export remains the server
plugin. The managed payload includes the optional files without enabling them.
The TUI candidate accepts only an exact active main-session identity returned
by the connected server and public SDK request/response metadata showing a
loopback HTTP(S) connection. Use an explicit local `opencode serve --hostname
127.0.0.1 --port PORT` and `opencode attach http://127.0.0.1:PORT`. The default
embedded server's internal URL, remote servers, redirects, child sessions,
unknown identities and other host versions fail closed. Session metadata
requests reject redirects before following them. Merely sharing a
workspace does not establish session identity.

The UI uses the separate `opencode:ui` consumer. Context acknowledgement does
not consume a pending UI update. An acknowledgement means that the complete
matching frame reached the mounted host component after route and revision
checks; it does not prove terminal pixels, human attention, or display in every
attached client. Registration alone does not acknowledge anything. A headless
server never loads this consumer. Missing UI, failed CLI reads and renderer
load failure leave the existing context path available.

Each active route gets a fresh read. Full snapshots restore current findings
when another client already acknowledged the global UI receipt. Pending
updates can replay after restart or failed acknowledgement. Same revisions do
not schedule another acknowledgement after success within the active route.
Clear and withdrawal updates replace the old warning and persist while that
route's snapshot remains unchanged; acknowledged transitions do not replay on
restart. A new snapshot replaces these transitions. Leaving the session,
losing a safe binding, an unavailable read or unloading the plugin removes the
frame. Route generations reject late asynchronous reads and stale component
mounts. Reads coalesce while one is in flight; active routes refresh on message
updates, session idle and bounded polling. No transcript messages or assistant
follow-up turns are created.

### Cursor optional conversation Inbox candidate

Cursor retains its existing postToolUse model-context delivery. A separate
optional editor extension is assembled under `cursor/inbox/`. It targets the
public editor APIs checked in Cursor 3.25.6 and 3.28.7 (VS Code 1.128.0).
Assembly does not install, enroll or enable it, and the desktop-managed context
payload does not load it.
The extension uses the existing unmanaged CLI resolver; it does not inherit the
desktop payload's private CLI connection.

A controlled packaged reference-hook test reaches the real editor webview and
acknowledges one matching UI receipt after a duplicate reference. It uses a
synthetic CLI response, not a native model tool event or real assessment. An
unobscured Cursor 3.28.7 native-window capture verifies both rails, the branded
frame, authored text, conversation identity and enrolled workspace.

A live Cursor 3.28.7 audit combined this Inbox and the chunked-header parser
with the separate scoped Cursor inventory change. One ordinary diagnostic turn
produced a Skill opportunities warning at 0.99 against the unchanged 0.95
threshold. The warning appeared in the already-open Inbox and stayed visible
through a normal refresh with the same `cursor:ui` receipt. This verifies live
display in the combined build. A separate synthetic two-window audit verified
that distinct enrolled roots each received only their own conversation.

Load the extension directory in a separate editor development host, then run
**Antiburn Inbox: Enroll This Workspace Window**. Enrollment is explicit and
limited to local POSIX workspaces with at most eight roots. Remote workspaces,
empty windows and Windows are unsupported. Use **Antiburn Inbox: Preview Optional
Hook Configuration** to print a separate postToolUse reference-hook fragment.
Merge that fragment through Cursor's normal configuration and trust controls,
preserving the existing context hook and any other hooks. Previewing does not
write configuration. Do not replace the context hook with the reference hook.

The reference hook publishes bounded conversation references to an existing
private enrollment. It does not store authored advice, transcripts or context
receipts. The Inbox reads canonical CLI snapshots with the separate `cursor:ui`
consumer. Each card identifies its exact conversation; workspace attribution is
hook-reported. The Inbox does not follow the active native chat or identify the
originating editor window. Matching enrolled windows can receive the same
conversation. Workspace changes end enrollment.

Changed findings can reveal the Inbox while preserving keyboard focus. Closing
it mutes automatic reveal until **Antiburn Inbox: Show Enrolled Conversations**
is used. **Antiburn Inbox: Stop Enrollment** stops this window's enrollment.
The renderer preserves authored text with a bounded emphasis, inline-code and
passive-link subset; unsupported Markdown remains literal. Raw HTML is escaped
and external resources, active links and command URIs are disabled.

UI acknowledgements require a matching session, receipt and renderer generation,
confirmed after application to a visible webview. Hook completion and successful
`postMessage` do not acknowledge display. The UI receipt is shared across
windows, not proof of per-window attention. Full snapshots restore current
findings after another window consumes a receipt; this does not guarantee that
every window sees each clear or withdrawal transition. Missing, hidden, muted or
disposed UI leaves context delivery independent. Local bridge metadata expires;
no provider calls or assistant continuation turns are created by this renderer.

## Native delivery versus evidence coverage

A native adapter is a delivery mechanism. It does not add an evidence reader
or expand the app's check coverage. See [check coverage](../check-coverage.md)
and [session coverage](../session-coverage.md) for those separate contracts.

| Host             | Natural boundary                               | App YAML evidence coverage                                                                     |
| ---------------- | ---------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| Claude Code      | UserPromptSubmit and PostToolUse command hooks | Existing supported native file contract                                                        |
| Codex            | UserPromptSubmit command hook                  | Existing supported native file contract                                                        |
| Cursor Desktop   | postToolUse command hook                       | Bounded ordered inline `cursor-desktop:*` native main sessions                                 |
| Cursor Agent CLI | postToolUse command hook                       | Native YAML evidence acceptance remains unverified; delivery-only fixtures do not establish it |
| Pi               | before_agent_start for user messages           | Existing supported native file contract                                                        |
| OpenCode         | Main-session message transformation            | Existing supported local provider-database contract                                            |

Cursor Desktop supports bounded shell arguments, output, recorded result status,
and available user-prefix text under parser revision 64. Unsafe identity, order,
or ownership remains unavailable; other tools retain partial evidence. The
[audit runsheet](../runbooks/session-advisory-audit.md#cursor-desktop-delivery-probe)
records a real YAML transport probe and a successful-call retry-loop negative
control. A positive Cursor retry-loop capture remains unverified. Desktop
support does not extend to Cursor CLI or other store formats.

No adapter wakes an idle agent, starts another model turn, blocks Stop to force
continuation, or relies on skill invocation for automatic delivery. Unsupported
host schemas or ambiguous ownership remain unavailable.

OpenCode retains handed-off advice for the current user turn across tool steps.
The bounded in-memory cache clears on a new user turn, compaction, or session deletion.
Reattachment does not read another receipt or acknowledge the same handoff again.

## Development and distribution

Build and install the CLI from `apps/cli`; an older installed CLI will not
understand the versioned flags. Use its JSON configuration to select a disposable
database for tests. Do not test against production session data.

The plugin assembler emits local candidates under the ignored
`.agent-artifacts/antiburn-core-plugin/` tree. The published Claude artifact and its
immutable marketplace pin remain separate. Native candidate validation is not
permission to publish, promote a marketplace entry, tag, or release.

### Codex and Cursor hook registration

Generate a native hook configuration fragment from the assembled package:

```sh
node .agent-artifacts/antiburn-core-plugin/codex/hooks/session-check-config.mjs --host codex
node .agent-artifacts/antiburn-core-plugin/cursor/hooks/session-check-config.mjs --host cursor
```

On macOS and Linux, these commands print JSON with the absolute packaged hook
path. They do not change configuration or trust hooks. Review and merge the
fragment into the host's native configuration, preserving existing hooks:
Codex uses `~/.codex/hooks.json` (or `$CODEX_HOME/hooks.json`); Cursor supports
project `.cursor/hooks.json`. Use each host's normal hook enablement and trust
review. Keep the generated package at its registered path; regenerate the
fragment after moving it. The preview rejects unsupported platforms.

The tested Codex and Cursor CLI builds did not activate bundled hooks merely
because the plugin was discovered or enabled. Their native configuration route
must be registered separately. Codex desktop plugin installation and hook trust
are separate from this CLI acceptance record.

The runtime support record must distinguish unit tests, SDK type checks,
package loading, and actual native context delivery. A host accepting a manifest
or loading an extension does not prove that its next prompt receives context.
Record the exact host version, platform, surface, fixture, and observed context
before claiming native acceptance.

## Local native acceptance record

The following development checks use synthetic CLI responses and disposable
profiles on macOS. Loopback endpoints capture native handoff; these checks do
not establish real app evaluation or public installation.

| Host build                      | Surface and observation                                                                                                                                                                                            | Remaining boundary                                                                                                              |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------- |
| Claude Code 2.1.307             | CLI print mode; packaged UserPromptSubmit and PostToolUse hooks placed the assessment in captured model requests and acknowledged it                                                                               | Desktop and public distribution require separate checks                                                                         |
| Codex CLI 0.163.0               | Native UserPromptSubmit registered in isolated hooks.json; packaged hook placed the assessment in a loopback Responses request and acknowledged it                                                                 | Bundled-plugin hook trust and desktop installation are separate from native CLI configuration                                   |
| Pi 1.3.0                        | CLI with packaged extension and offline configuration; before_agent_start placed the assessment in a loopback model request and acknowledged it                                                                    | Type validation targets SDK 0.80.3; TUI and other runtime versions require separate checks                                      |
| OpenCode 1.19.3                 | Packaged plugin with an isolated offline dependency profile; messages transform placed the assessment in the primary build model request, then acknowledged it; the preceding title request received no assessment | Type validation targets SDK 1.19.2; native handoff requires mutating the host message array in place                            |
| Cursor Agent 2026.05.24-dda726e | Native project postToolUse configuration referenced the unmodified packaged hook; after a real shell tool, the native Connect hook response carried the assessment and the CLI acknowledged it                     | The loopback protocol fixture observes native context handoff, not a cloud model request; YAML evidence coverage is unsupported |

There is no claim of Windows, Linux, remote-host or GUI parity from these
checks. Command-hook acknowledgement confirms a completed stdout write, not
model consumption. Pi uses a callback-result handoff boundary. OpenCode changes
the transient primary-session prompt before the host converts it to model
messages. Title and compaction requests do not consume its delivery receipts.

## Validation and privacy

Use temporary SQLite databases and synthetic authored messages. Verify source
and definition invalidation, clearing/retriggering, restarts, concurrent sessions,
duplicate callbacks, lost acknowledgements, malformed responses, and timeouts.
Compare the product database before and after CLI reads to prove no writes.
Verify real-session mod events produce no detector findings or fix actions.

Delivery adds no first-party analytics event containing session IDs, check IDs,
authored messages, evidence, or transcript data. Local connection diagnostics
provide the operational signal for this development milestone; broader adoption
measurement remains outside this change. Existing usage meters stay independent
of check connectivity.
