# Burn Check Remediation Plan

Status (2026-09-10): implementation is complete. Local validation passed.
Real-machine and data-dependent validation remain release checks.

[`docs/remediation.md`](../remediation.md) is the current implementation guide.
[`docs/check-coverage.md`](../check-coverage.md) defines check, prompt, Auto Fix,
verification, and savings support. [`docs/session-coverage.md`](../session-coverage.md)
defines accepted source contracts.

## Delivered Scope

- Phase 1 characterized the reviewed Claude Code, Codex, OpenCode, Pi, and
  Antigravity passive sources. It kept unsupported evidence fail-closed.
- Phase 2 added exact findings, bounded prompts, initial model editors, durable
  watches, recurrence, and old-model savings.
- Phase 3 added the main Burn checks workspace, passive enrollment, action
  joins, four distinct ID types, two-step Auto Fix, all nine estimate methods,
  aggregate wins, sample navigation, recovery, and reviewed analytics.
- Auto Fix supports Claude Code model and reasoning settings, Codex model and
  reasoning settings, OpenCode model settings, and Pi model and reasoning
  settings.
- macOS and Linux support apply. Native Windows supports safe read attribution
  but not apply. WSL remains a separate environment.
- Antigravity supports prompts for D and O findings. It has no safe automatic
  editor or positive fix verifier.

## Architecture Record

The engine owns typed findings, prompt construction, pure verification, and
typed estimates. The desktop shell owns target grouping, trusted roots,
publication attribution, local persistence, file changes, recovery, IPC, UI,
and analytics.

Vendor evidence policy uses `VendorRemediationPolicy` under
`src-tauri/src/remediation/vendors/`. Vendor config parsing and edits use
`VendorConfig` under `src-tauri/src/agent_config/vendors/`. Shared lifecycle,
storage, and atomic-write code contain no vendor-specific branches.

The implementation reads existing local files and databases. It installs no
hooks, plugins, collectors, or runtime subscriptions. It does not launch an
agent to collect check evidence.

## Historical Phase 2 Record

This section records the 2026-09-09 Phase 2 boundary. It does not describe the
current product boundary.

Phase 2 exposed backend remediation without a remediation UI. Auto Fix covered
only publication-attributed Claude Code and Codex old-model settings. It used
one command that prepared and applied the edit. Phase 3 replaced that contract
with separate prepare and apply commands and expanded editor support.

Phase 2 validation passed engine formatting, strict Clippy, 1,366 unit tests,
integration tests, and doctests. Desktop Rust formatting, strict Clippy, and
1,107 tests passed. Desktop lint, type checks, 1,294 frontend tests, and the
production build passed. Coverage, quality, secret, and whitespace checks
passed.

## Phase 3 Completion Record

### Phase 3A: Mock UI

The maintainer approved the mock on 2026-09-10. It established the main-window
navigation, shared check presentation, expandable target rows, review dialog,
prompt action, samples, aggregate wins, and loading and error states. It did not
call production remediation commands.

### Phase 3B: Contracts And Persistence

Desktop schema V44 added safe display snapshots and durable contributions. The
public contract separated durable finding IDs, durable attempt IDs, expiring
action IDs, and expiring prepared-operation IDs. It also added bounded aggregate
wins and opaque sample navigation.

### Phase 3C: Passive Verification And Estimates

Desktop schema V45 added publication-time passive enrollment and action joins.
It preserved the passive boundary and origin when an action joined an attempt.
All nine estimate methods gained typed inputs, units, revisions, and unavailable
results. Contribution writes became atomic with verification transitions.

### Phase 3D: Automatic Editors

The editor matrix is complete for the advertised scope:

| Agent       | Model                                                                           | Reasoning                                                          |
| ----------- | ------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| Claude Code | Complete                                                                        | Complete                                                           |
| Codex       | Complete                                                                        | Complete                                                           |
| OpenCode    | Complete                                                                        | Unavailable because accepted sources have no historical effort map |
| Pi          | Complete                                                                        | Complete                                                           |
| Antigravity | Unavailable because accepted sources do not bind one effective physical setting | Unavailable                                                        |

The editors resolve current project and global precedence. They distinguish the
trusted repository root from the session cwd. They reject unsafe roots,
symlinks, unsupported ownership, runtime and managed overrides, malformed
config, and changed targets. They preserve unrelated data and verify semantic
readback. An uncertain replacement enters durable recovery.

### Phase 3E: Production UI And Analytics

`BurnChecksSession` uses `useSyncExternalStore`. It loads the report only while
the section is active and visible. It loads targets only for open failed checks.
It coalesces refreshes, rejects stale results, and retains previous data after a
refresh error. The production view has no fixture fallback.

The main window and popover share names, icons, ordering, assessed counts,
token-burn formatting, and status colors. The main view hides not-assessed rows.
It uses simple target detail text, opaque sample navigation, a two-step review,
and a shaped loading skeleton.

Analytics distinguishes visible report states, Auto Fix review, confirmation,
typed apply results, prompt preparation, clipboard success, and visible later
outcomes. It sends no work data, values, IDs, paths, exact tokens, or exact
costs.

### Phase 3F: Local Acceptance

Local validation passed on 2026-09-10:

- Engine formatting, strict Clippy, 1,370 unit tests, integration tests, and
  doctests passed.
- Desktop Rust formatting, strict Clippy, and 1,141 tests passed.
- The analytics-enabled suite passed 1,217 tests. Two ignored tests did not run.
- Desktop lint, type checks, 1,312 frontend tests, and the production build
  passed.
- Coverage, design drift, secrets, whitespace, and changed-code quality checks
  passed.
- Browser fallback review covered the unavailable state at 1000 x 560 and a
  constrained frame in light, dark, and reduced-motion modes.

## Release Validation

Release validation must use native macOS, Linux, and Windows machines. It must
cover the advertised read and apply matrix with real supported agent config. It
must also cover data-dependent checks, prompt copy, clipboard errors, action
expiry, changed config, recovery, recurrence, sample navigation, both themes,
keyboard use, screen readers, reduced motion, and constrained windows.

Windows must remain read-only for remediation until native apply semantics,
ACL preservation, reparse points, sharing conflicts, file identity, atomic
replacement, and uncertain-write recovery pass review and tests.
