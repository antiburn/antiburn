# Burn check coverage and remediation

Status (2026-09-09): Phase 1 and Phase 2 implementation and acceptance
validation are complete. Phase 3 remains deferred.

## Architecture

The implementation uses these boundaries:

| Name | Responsibility |
| --- | --- |
| `SessionReader` | Read native session files and databases into normalized observations with explicit coverage. |
| `ModelCatalog` | Resolve reviewed provider, API, model, option, accounting, replacement, and pricing policy. |
| `AgentConfigEditor` | Inspect the effective agent model setting and prepare one supported minimal edit. |

The implementation reads existing local files and databases only. It does not
install hooks, plugins, collectors, or runtime subscriptions. It does not launch
an agent to collect evidence. Missing evidence remains unavailable.

## Phase 1: Burn check coverage

Phase 1 covers the accepted source contracts for OpenCode, Pi, Codex, Claude
Code, and Antigravity. It does not claim all historical versions. Cursor and
other agents keep their existing limited support. The current source and check
contracts are:

- [`docs/session-coverage.md`](../session-coverage.md) for discovery, framing,
  parsing, companions, fingerprints, resume state, and provider routes.
- [`docs/check-coverage.md`](../check-coverage.md) for the nine checks, finding
  eligibility, clean-result limits, and confirmed unsupported cases.

The implementation keeps all 26 `SourceFormat` values in each required
inventory and matrix. A schema, header, or pinned producer shape with synthetic
fixtures defines an accepted boundary when no release range is available.
Unknown changed evidence fails partial or unavailable.

Implemented results include:

- Claude Code and Codex assess D/T/S/O/F/C on complete accepted sessions and
  reviewed routes.
- OpenCode delegation requires native task metadata, matching ancestry, and the
  actual child model. Its export and SQLite readers use validated request order.
- Pi T uses the saved agent-selected policy. Pi S is finding-only for reviewed
  persisted output from the official example extension.
- M/B/K findings stay limited to complete observed subsets and calls. They do
  not claim a full historical inventory or a session-wide clean result.
- Cache accounting uses persisted provider and API values. `CacheWrite` selects
  Claude policy. `UncachedInput` selects OpenAI policy, including mixed-family
  sessions.
- Cursor and Antigravity keep direct findings only where the matrix permits
  them. Their source gates deny clean results.

CoreV2 `session_message` remains outside `OpenCodeSqliteV2`. That reader accepts
the `session`, `message`, and `part` cluster only. OpenCode WSL executable export,
Cursor synthesis, and Antigravity private identity and route gaps remain stated
limits.

## Phase 2: Backend remediation

Phase 2 implements exact backend targets, Copy Prompt Fix, Claude Code and Codex
old-model Auto Fix, durable verification watches, recurrence, and supported
old-model savings. No remediation UI is available.

### Target listing

`list_burn_check_targets` scans at most 512 current sessions and retains at most
512 raw findings before exact grouping. The server returns at most 100 grouped
targets and sets `truncated` when either internal bound is exceeded. The opaque
target cache holds 100 entries.

Target IDs expire after 10 minutes. The command has no pagination and accepts no
`maxTargets` input. A row is included only when its supported bounded prompt was
built successfully.

Each target identity includes the detector, agent, exact `SourceFormat`, scope,
and detector-specific cause. Old-model grouping keeps provider, API, observed
model, and replacement separate. Private selectors also bind the environment,
workspace when applicable, source generations, publication fences,
fingerprints, and current parser, analyzer, evidence, metrics, catalog, and
source revisions. Each action revalidates its cached findings.

Public target rows contain an opaque ID, bounded display facts, occurrence
count, Auto Fix availability, watch state, coverage limits, expiry, and savings
state. They do not contain raw paths, session IDs, transcript text, config
content, credentials, or evidence bodies.

### Copy Prompt Fix

`copy_prompt_fix_burn_check_target` returns a deterministic prompt only for a
supported current finding. The prompt is at most 8 KiB UTF-8. It includes at
most eight sanitized identities, each at most 256 bytes. It removes control
characters and excludes secrets, transcripts, config content, private paths,
and unrelated history.

The prompt states the observed problem, exact scope, objective, quality limits,
permission limits, and evidence needed for verification. It treats quoted
values as data. If required facts cannot fit safely, the command returns a typed
unavailable result. A successful command starts or reuses one exact durable
watch. It does not accept an external completion claim.

### Old-model Auto Fix

Automatic edits support only old-model findings for Claude Code and Codex. The
finding must have publication-time attribution to an existing effective global
or project model setting. The attributed effective value must match the
observed old model. This condition prevents current configuration from becoming
historical source truth.

The supported existing files and precedence are:

| Agent | Project preference | Global fallback | Format |
| --- | --- | --- | --- |
| Claude Code | `.claude/settings.local.json`, then `.claude/settings.json` | `~/.claude/settings.json` | JSON top-level `model` |
| Codex | `.codex/config.toml` | `~/.codex/config.toml` | TOML top-level `model` |

Project files apply only when they contain the model key. Otherwise, the global
file remains the effective target. Claude JSON output preserves unrelated data
but is rewritten as formatted JSON. Codex uses `toml_edit` to preserve unrelated
TOML formatting where possible.

Auto Fix supports native environments on non-Windows platforms. Windows apply
is unavailable. Runtime model or home overrides, managed configuration, Codex
profiles, unsupported agents, ambiguous or missing targets, and untrusted
workspaces make Auto Fix unavailable. The editor does not create a file. It
rejects symlinks, non-regular files, wrong ownership on Unix, unsafe roots,
invalid model values, malformed or duplicate definitions, and files larger
than 256 KiB.

The command reads the selected file, prepares one replacement, rechecks file
identity and bytes, writes an atomic same-directory replacement, preserves file
permissions, syncs the directory, reads the file again, and verifies the model
value. A conflict does not retry against new content. An uncertain post-write
result enters durable recovery. A successful write enters `watching`, not
`fixed`.

### Publication attribution

Ready evidence publication can store three nullable desktop-derived values for
Claude Code and Codex: a hashed physical target, the effective scope, and the
effective model. Publication stores them only when the current effective value
matches a model observed in complete model evidence. Non-ready publication
stores no attribution.

The hash uses the local store secret and includes the agent, physical path, and
the model-setting key. The values exist only to select and verify later
remediation scope. They are not session-source evidence or historical truth.

### Watches and verification

The durable lifecycle is:

```text
reserved -> writing -> recoveryNeeded -> watching -> fixed -> recurred
```

Copy Prompt Fix starts or reuses `watching`. Auto Fix uses the first three
states for write and crash safety, then enters `watching`. Startup reconciliation
removes unused reservations and checks uncertain writes against the same
physical target and effective scope.

Generic prompt watches use a conservative post-boundary rule. They keep the
exact source format and canonical target identity. A finding remains unresolved.
Only a fresh complete assessment that proves the exact target absent can mark
the watch fixed. A later exact finding marks it recurred. Positive-only source
coverage cannot verify absence.

Old-model watches use a stricter rule. They accept only main-thread sessions
that started after the boundary and have matching publication-time physical
target, scope, and effective old or replacement model attribution. The observed
turn must also match the provider and API. Actual replacement use after the
latest old-model use marks the watch fixed. Actual old-model use after fixed
marks it recurred. Missing eligible evidence leaves the watch watching.

The watch records the exact observed fixed and recurrence times from evidence.
It does not use config readback, inactivity, report age, deletion, or a changed
policy as proof. Verification DTOs include `evidenceRevision` for watching,
fixed, unresolved, and recurred results when evidence was evaluated.

### Savings

Only old-model replacement has numeric savings. The implementation recomputes
the cumulative API-equivalent price difference from eligible replacement token
classes assigned to the exact physical target:

```text
sum(observed token class * pinned old rate)
  - sum(observed token class * pinned replacement rate)
```

The watch pins both rate sets and `pricingRevision` at creation. Savings close
at the exact recurrence time. Known zero and negative values remain known.
Missing rates, evidence, or a pricing revision remain typed unknown results.
Arithmetic overflow also remains unknown. The API exposes
`apiEquivalentCostAvoidedUsd`; it does not expose `tokenEquivalent`, a percent
fallback, or a forced positive minimum.

### Persistence and work scheduling

Desktop migration V43 is the final unreleased remediation schema. It stores
exactly two versioned JSON envelopes: `definition_json` and `result_json`. Each
envelope has a 32 KiB database and application limit. Indexed columns retain
target identity, lifecycle state, `dirty_revision`, `evaluated_revision`, the
effective boundary, and exact creation, update, fixed, and recurrence times.

Evidence publication increments `dirty_revision` for matching active watches in
its guarded transaction. The existing evidence worker evaluates a snapshot
outside the write lock. It commits only when the observed dirty revision still
matches. Otherwise, the worker evaluates the newer revision later. There is no
new queue, lease, scheduler, or window-open dependency.

Session retention does not cascade-delete watches. The schema adds no preview,
event, claim, contribution, queue, or lease table.

### Backend contract

The backend exposes exactly three commands:

```text
list_burn_check_targets
auto_fix_burn_check_target
copy_prompt_fix_burn_check_target
```

Only the `popover` window has permissions for these commands. The Rust DTOs and
TypeScript mirrors use typed outcomes, actions, lifecycle states, verification
reasons, and savings reasons. The verification payload carries
`evidenceRevision`. Known savings carries `pricingRevision`.

No command accepts a path, command, JSON pointer, replacement bytes, completion
claim, page cursor, or target-count option. There is no preview, history,
recommendation, generic apply, or backend pagination operation.

## Acceptance

Acceptance validation passed on 2026-09-09:

- Engine formatting and strict Clippy passed. The engine ran 1,366 unit tests,
  all integration suites, and its doctest. Two ignored unit tests did not run.
- Desktop Rust formatting and strict Clippy passed. All 1,107 tests passed.
- The check coverage contract ran four tests and passed.
- Desktop lint, type checks, all 1,294 frontend tests, and the production build
  passed. The existing large-chunk warning remains non-failing.
- The full quality scan, secrets scan, and whitespace checks passed.

## Completion record

- Phase 1 source and check contracts are complete for the documented limits.
- Phase 2 engine APIs, desktop persistence, commands, TypeScript mirrors,
  prompts, editors, watches, verification, recurrence, and savings are complete.
- No analytics event was added because Phase 2 has no user-visible remediation
  action. Phase 3 must review action and outcome analytics before UI work ships.
- No remediation UI is implemented.
- Final acceptance validation passed on 2026-09-09.
- Phase 3 remains deferred.

## Phase 3: Deferred UI and wider fixes

Phase 3 can add the Burn Checks target-row UI after final acceptance. It can
also evaluate automatic fixes for reasoning, unused MCP servers, and unused
skills, plus wider agent support.

Each future editor must keep the Phase 2 privacy, source coverage, exact target,
freshness, trusted path, atomic write, and semantic readback rules. It must not
infer a safe edit from one unused observation or add arbitrary filesystem paths
or commands.
