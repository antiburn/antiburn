# Session row: check-state copy and edge cases

Branch: `claude/ui-test-state-handling-10db01` · Status: implemented, awaiting Keith's manual test

## Status

| Step | State |
| --- | --- |
| Investigate why counts vary and why checks are not assessed | Done |
| Agree the copy rules below | Done (discuss 2026-08-28: ellipsis on transient states; fraction stays) |
| Implement + tests | Done — 902/902 desktop tests pass |
| Keith manual test + screenshot | Waiting on Keith — needed before the PR opens |
| Commit + push | Done — 3 commits on `claude/ui-test-state-handling-10db01` |
| Draft PR | Blocked on the screenshot |

## Found during manual test (2026-08-28)

- The tooltip's "incomplete evidence" wording was unhelpful; replaced with
  concrete phrases ("couldn't read the whole session log", "this agent's logs
  don't record what this check needs").
- Root cause found for many "not assessed" rows: current Claude Code writes
  `custom-title`, `bridge-session`, and `artifact-comment-monitor` housekeeping
  records that the parser did not recognize, which marked whole transcripts
  partially read. Added all three to the recognized-eventless list in
  [jsonl.rs](../../crates/antiburn-local/src/analysis/vendors/jsonl.rs),
  extended the housekeeping characterization fixture to cover them, and bumped
  `PARSER_REVISION` to 5 so stored verdicts recompute. Engine 962/962 and shell
  566/566 tests pass.
- Scale of that bug, measured on this machine: 289 of the 302 Claude Code
  transcripts inside the 14-day discovery window (95%) hold at least one of the
  three record types, so 95% of recent sessions could never report a clean
  check.
- Separate bug found, not fixed here: discovery treats `.json` sidecar files
  under `claude-code-sessions/` as sessions. `scheduled-tasks.json` and a
  `local_*.json` both appear in `session_evidence`.

## What the screenshot actually shows (answers first)

**Every session has exactly 3 burn checks.** Reasoning overkill, excess cache
rehydration, bloated initial context. Nothing has "only 1 or 2 tests" — the
row's denominator only counts *assessed* checks, and the rest get pushed into
the "· N not assessed" tail. So:

- `0/1 checks pass · 2 not assessed` = 3 checks: 1 assessed (it failed), 2 not assessed
- `3/3 checks pass` = all 3 assessed, all clean
- `0/0 checks pass · 3 not assessed` = nothing was assessable at all

The varying denominator is the confusing part, not missing checks.

**Why checks come back "not assessed".** The engine
([badges.rs](../../crates/antiburn-local/src/insights/badges.rs)) is
deliberately conservative: it will report a *finding* from partial evidence,
but it refuses to say *clean* unless the session's evidence is complete. A
check lands in "not assessed" for one of three reasons:

1. **Capability missing** — the session's source doesn't record what the check
   needs (e.g. a Codex transcript without reasoning-effort tiers or cache
   token classes). All 3 checks drop at once.
2. **Incomplete evidence** — the check saw nothing wrong, but coverage was
   partial: a truncated tail, a cancelled write, a malformed or unrecognized
   record anywhere in the transcript. "No finding + partial evidence" is
   honestly "not assessed", never "clean". This is the common case for the
   antiburn sessions in the screenshot — one bad record downgrades the whole
   session.
3. **Evidence contract incomplete** — the detector's input contract wasn't
   satisfied.

The list row currently never says *why* — the reason exists in the payload
(`notAssessedReason`) and Insights already has reader wording for it, but the
session-row tooltip drops it.

## When the checks actually run

The badges are computed from stored evidence at render time; the evidence
itself is produced by a background pipeline
([scan.rs](../../apps/desktop/src-tauri/src/scan.rs),
[insights_worker.rs](../../apps/desktop/src-tauri/src/insights_worker.rs)):

1. **Discovery** — a scan pass runs at app launch, then on a 60-second tick
   *only while the popover is visible* (plus explicit kicks). Each pass finds
   transcripts modified in the last **14 days**, including sessions written
   before antiburn was installed — antiburn reads the agents' own transcript
   files, so pre-install history appears as long as the file's mtime is inside
   the window. Anything older than 14 days is never discovered.
2. **Queueing** — a discovered session whose content changed since the last
   look is marked *pending*. An active session changes on every pass, so it is
   re-queued continually.
3. **Evidence worker** — a separate worker drains the pending queue newest
   first, one transcript at a time, parsing the whole file and storing
   evidence (retry with 30s→15m backoff, 5 attempts, then *failed*).
4. **Render** — opening the popover asks `get_session_hygiene` for the listed
   rows; badges are folded from stored evidence on the spot.

So the states Keith asked about:

- **Fresh install / app load over old sessions**: everything inside 14 days
  queues at once; rows read "Computing checks" until the worker catches up,
  newest rows first.
- **Session still being written**: the analyzer accepts a stable *prefix* of
  the growing file. Prefix evidence can prove a finding but never "clean", so
  a live session shows findings immediately and holds everything else at "not
  assessed" (`evidenceState: activelyGrowing`, shown as "Still writing")
  until a later pass sees the settled file.
- **After an app update**: stored evidence from an older parser/analyzer
  revision reads "Refreshing" and is re-queued at launch.

## Proposed copy rules

Current main renders `${passed}/${assessed} burn checks · N not assessed`
in [SessionStatusBar.tsx](../../apps/desktop/src/components/session/SessionStatusBar.tsx).
Proposed states:

| Data | Before | Now |
| --- | --- | --- |
| 3 assessed, all clean | `3/3 burn checks` | unchanged |
| 3 assessed, 1 finding | `2/3 burn checks` | unchanged |
| 1 assessed (failed), 2 not | `0/1 burn checks · 2 not assessed` | `0/3 burn checks · 2 not assessed` |
| 0 assessed | `0/0 burn checks · 3 not assessed` | `Not assessed` (tertiary ink, no fraction) |
| Evidence pending/processing | `Computing checks` | `Computing checks…` (ellipsis on every transient state — also `Refreshing checks…`, `Still writing checks…`; settled states stay bare) |

Rules:

1. **The denominator is every check, never only the assessed ones.** Keith
   caught the original rule live: `0/1 burn check · 2 not assessed` makes the
   reader add 1 and 2 to learn the session has three checks, and the two
   numbers look like they disagree. A fixed denominator removes the
   contradiction — `0/3 · 2 not assessed` says three checks exist, none
   passed, and two of them could not be judged. This also retires the
   singular-noun problem by construction: the denominator is the badge count,
   which is always three, so `0/1` can no longer appear.
2. **Never render `0/0`**: with nothing assessed the fraction is noise; the
   whole verdict becomes `Not assessed` in `label-tertiary`.
3. **The ink now ramps against the total too.** One failure beside two
   unassessed checks reads orange (33%), not full red. Claiming maximum
   severity when two thirds of the checks never ran overstated the finding.
4. **Tooltip explains the grey rows**: each not-assessed check names its
   reason in plain terms ("couldn't read the whole session log", "this
   agent's logs don't record what this check needs"). The shared vocabulary
   lives in `lib/presentation/sessionHygiene.ts`.
5. `aria-label` copy follows the same rules: "0 of 3 burn checks pass; 2 not
   assessed".

## Open question for Keith

- "burn checks" vs the screenshot's older "checks pass" — main already renamed
  it; happy either way, plan assumes "burn checks" stays.

## Implementation sketch

- `SessionStatusBar.tsx`: derive the three display states above; singular
  helper; `Not assessed` state.
- `sessionHygiene.ts`: export not-assessed reason wording (lifted from
  `InsightsPane.tsx`'s `NOT_ASSESSED_WORDING`, reworded for a single session);
  attach to tooltip rows.
- Tests: extend `SessionStatusBar.test.tsx` for the singular case, the
  zero-assessed case, and the tooltip reasons; keep `SessionList.test.tsx`
  passing.
- No Rust changes; the payload already carries `notAssessedReason`.
