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
acknowledge after the stdout write completes. Pi and OpenCode schedule an
acknowledgement after constructing the callback result. These APIs do not
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

Subprocess execution uses argument vectors and bounded input, output, lock,
and execution limits. Malformed or incompatible responses fail closed.
Disconnected adapters do not activate local detectors. Plugin reads do not
create evaluation requests or model turns.

## Native delivery versus evidence coverage

A native adapter is a delivery mechanism. It does not add an evidence reader
or expand the app's check coverage. See [check coverage](../check-coverage.md)
and [session coverage](../session-coverage.md) for those separate contracts.

| Host        | Natural boundary                               | App YAML evidence coverage                                                |
| ----------- | ---------------------------------------------- | ------------------------------------------------------------------------- |
| Claude Code | UserPromptSubmit and PostToolUse command hooks | Existing supported native file contract                                   |
| Codex       | UserPromptSubmit command hook                  | Existing supported native file contract                                   |
| Cursor      | postToolUse command hook                       | Unsupported; synthetic delivery tests do not establish evaluation support |
| Pi          | before_agent_start for user messages           | Existing supported native file contract                                   |
| OpenCode    | Session-bound system context transformation    | Existing supported local provider-database contract                       |

No adapter wakes an idle agent, starts another model turn, blocks Stop to force
continuation, or relies on skill invocation for automatic delivery. Unsupported
host schemas or ambiguous ownership remain unavailable.

## Development and distribution

Build and install the CLI from `apps/cli`; an older installed CLI will not
understand the versioned flags. Use its JSON configuration to select a disposable
database for tests. Do not test against production session data.

The plugin assembler emits local candidates under the ignored
`.agent-artifacts/antiburn-shell/` tree. The published Claude artifact and its
immutable marketplace pin remain separate. Native candidate validation is not
permission to publish, promote a marketplace entry, tag, or release.

The runtime support record must distinguish unit tests, SDK type checks,
package loading, and actual native context delivery. A host accepting a manifest
or loading an extension does not prove that its next prompt receives context.
Record the exact host version, platform, surface, fixture, and observed context
before claiming native acceptance.

## Local native acceptance record

The following development checks use synthetic CLI responses and disposable
profiles on macOS. A loopback mock model endpoint captures the host request;
these checks do not establish real app evaluation or public installation.

| Host build                                      | Surface and observation                                                                                                                      | Remaining boundary                                                                                                          |
| ----------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| Claude Code 2.1.307                             | CLI print mode; packaged UserPromptSubmit hook placed the synthetic assessment in the captured request, then acknowledged it                 | PostToolUse has unit and packaging proof only; desktop and public distribution remain separate                              |
| Codex CLI 0.163.0                               | CLI native UserPromptSubmit configured in isolated hooks.json; packaged hook placed the assessment in a loopback request and acknowledged it | Bundled-plugin hook trust and desktop installation are not proven by direct hook configuration                              |
| Pi 1.3.0                                        | CLI with packaged extension and offline configuration; before_agent_start placed the assessment in a loopback request and acknowledged it    | Type validation targets SDK 0.80.3; TUI and other runtime versions require separate checks                                  |
| OpenCode 1.19.3                                 | Isolated server health passed; prompt API runs did not produce observed plugin CLI calls or advisory context                                 | Native system-transform delivery remains unverified; a final guarded prompt API run stalled before a captured model request |
| Cursor 3.25.6 / Cursor Agent 2026.05.24-dda726e | Synthetic adapter and packaging tests; isolated CLI attempted api2.cursor.sh before any hook, and the local guard denied it                  | Actual native postToolUse route remains unverified; YAML evidence coverage is unsupported                                   |

There is no claim of Windows, Linux, remote-host or GUI parity from these
checks. Command-hook acknowledgement confirms a completed stdout write, not
model consumption. Pi uses a callback-result handoff boundary. OpenCode native
delivery remains unverified.

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
