# Real YAML session-check acceptance

The immediate target is the existing **Failed retry loop** definition reaching
one Claude Code main session through the real Antiburn app and CLI.
Do not add another audit definition or a new communication service. The initial
Claude acceptance and the later cross-vendor visual audit have separate results.

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
Disable temporary activations and stop owned backends after capture. The
bidirectional request plan and broader vendor audit remain separate work.

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
behavior or compatibility with newer Claude recordings. The current retained-root
parser rejects 2.1.308 and 2.1.309 evidence. Do not relabel current recordings as 2.1.278.

The 2026-10-23 local audit produced one real Jev assessment (`stuck`, probability
1.0), displayed the unchanged authored failure in Antiburn, and delivered that
advisory through a real Claude UserPromptSubmit hook. The resumed agent's prose
attributes the scripted attempts to itself; the fixture manifest remains the
source of truth for their provenance. Current-version ingestion and recovery
clearance remain separate acceptance cases.

## Cross-vendor fixture audit

Select `--vendor claude|codex|pi` with the trigger script. Claude is the default.
Each format is characterized independently; installing a transcript proves neither
vendor resumability nor current-version compatibility. Pi uses its configured
`PI_CODING_AGENT_DIR` or `PI_AGENT_DIR` and the native project-directory form.
Codex installation currently supports Antiburn ingestion only: Codex v0.163.0
requires a registered thread to resume by UUID, beyond the generated rollout.
Do not insert rows into the vendor's state database to manufacture resumability.

OpenCode uses its native session and replay APIs for this audit. For a warm
reader, replay pending and terminal tool states with stable identities. Explicitly
configure the candidate plugin in the disposable project's `opencode.json` and
verify the backend loaded it; placing a file in `.opencode/plugins` alone did not
load it in the audited desktop backend.

The later 2026-10-23 audit has these results:

- OpenCode desktop: real Jev assessment (`stuck`, probability 0.97), authored
  failure visible in Antiburn, and the actual desktop response repeats the advisory.
- Pi: real assessment and actual plugin custom-message delivery, confirmed by its
  RPC event stream and assistant response. Later native writes invalidate the
  assessment. A terminal screenshot and continued-session compatibility remain open.
- Claude 2.1.309: resuming the characterized fixture invalidates the assessment;
  the actual desktop response reports no advisory. Earlier print-mode evidence
  does not establish current interactive compatibility.
- Codex: real assessment visible in Antiburn; UUID resume rejects the unregistered
  fixture. Vendor receipt and terminal/desktop screenshots remain unverified.

Keep the local report's timestamps and validity snapshot with each capture.
These scripted retry fixtures exercise the real evaluator; they do not prove
spontaneous agent retry behavior, recovery clearance, or all-vendor acceptance.
