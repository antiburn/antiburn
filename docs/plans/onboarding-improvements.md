# Onboarding improvements: agents detected, a useful Ready page, nudges explained

Branch: `claude/onboarding-improvements-8f6140` · Status: planning

## Status

| Step | State |
| --- | --- |
| Explore current code, verify data sources | Done |
| Agree this plan with Keith | Done 2026-08-31 — all questions answered except one u-3 sub-question (do OFF-by-default agents stay silently watched?) |
| Feature 1: Agents detected step | Built 2026-08-31 — `DisabledAgents` setting + `recent_sessions_excluding` filter (Rust), `agentsDetected` step second in the flow, Settings → Sources mirror, popover re-query, tests green. Zero-session agents seed OFF and the whole set persists (Option 1); the later-install question is still open with Keith |
| Feature 2: Ready page stats | Built 2026-08-31 — `get_hygiene_summary` command (window + disabled-agent filter mirrors the session list; only current ready evidence counts analyzed), Ready rebuilt per v5 proto (title-1 heading, richer sentence with window + agent names, 88px slot with stats card / progress bar, plain toggle rows), `nudges_respect_dnd` setting end-to-end with the OS Focus gate now opt-in, Settings → Notifications mirror row, analytics/review-source card and read-only footnote removed from Ready. Rust 658 + frontend 938 tests green |
| Feature 3: Nudges explained | Not started — surface confirmed: Settings → Notifications (discuss, 2026-08-31) |
| Quick win: add `~/Developer` to `MACOS_CODE_DIRS` | Done 2026-08-31 — line + test in `crates/antiburn-local/src/repositories/platform/macos.rs`, 6/6 pass. Rides in the Feature 1 PR |
| Feature 4: sourcesAndRepos step redesign | Design settled via proto 2026-08-31 (see section below); own PR |
| Tests + manual test + screenshot | Not started |
| PR | Not started — one PR per feature (now four); 2 stacks on 1, and 4 also touches `OnboardingFlow.tsx` so it joins that stack |

## Where things stand today

The onboarding window is a fixed 680×480 flow of three steps —
`welcome → sourcesAndRepos → ready` — driven by a plain step tuple in
[OnboardingFlow.tsx:66](../../apps/desktop/src/views/onboarding/OnboardingFlow.tsx).
Discovery kicks off when the reader advances past Welcome, so by the Ready
step a scan has usually finished.

Three facts shape all three features:

1. **Per-agent detection data exists end to end and is never rendered.**
   `ScanStatus.agents` carries `{ agent, lastCompletedAt, sessionsSeen }` per
   agent ([dto.rs:256](../../apps/desktop/src-tauri/src/dto.rs)), and the
   frontend has a full display registry (names, icons) in
   `lib/presentation/agents.ts`. No component reads it. One caveat: the
   `agents` array is only filled when status is read through the
   `get_scan_status` **command** — scan **events** omit it by design — so any
   step that shows it must fetch, not just read the pushed snapshot.
2. **Detection means "sessions found", not "app installed".** There is no
   is-this-binary-installed probe. Copy must say "found 304 Claude Code
   sessions", never "Claude Code is installed".
3. **Burn checks are real now — and measured fast on first run.** Each
   session gets 3 burn checks, computed by a background evidence worker that
   drains a queue newest-first (see
   [session-check-states.md](session-check-states.md)). The plan originally
   assumed a fresh install leaves most rows at "Computing checks…" when the
   reader reaches Ready. **Measured 2026-08-31** on Keith's machine (cold
   dev-build run, fresh debug store): all 323 sessions in the 14-day window
   analyzed in 46 seconds — 175 in the first 15s, 269 by 30s; the last-7-days
   cohort ~80% done within 25s. So on this hardware, real check results exist
   by Ready. Caveat: one fast Mac, one dataset — a slow disk or giant
   transcripts would stretch it, so progress-shaped copy ("Checked X of Y")
   degrades more gracefully than a bare count.

## Feature 1: "Agents detected" step

A new step that shows which coding agents antiburn found sessions for, with
per-agent session counts.

**Position changed 2026-08-31 (v3 proto round): the step comes second**,
right after Welcome — `welcome → agentsDetected → sourcesAndRepos → ready`.
Discovery kicks on leaving Welcome, so the reader may watch counts fill in
live on this step; the component already plans for that ("rows appearing as
agents complete"). The intro line is just "antiburn found sessions from N
agents on this Mac." — no toggle-explainer sentence (cut in the same round).

**What it shows** (decided in discuss, 2026-08-31): a checklist — one row per
agent with icon + display name (from `lib/presentation/agents.ts`) and a
toggle. Detected agents show "N sessions" and default ON; zero-session agents
are included but default OFF. This implies a new per-agent enabled setting
that does not exist today (the only `enabled` column is per-repository, and
the analysis cohort is compile-time): a persisted setting, scan/discovery
gating in Rust, and a mirror in Settings so the choice stays editable.
Toggle semantics, first half answered (chat, 2026-08-31): OFF hides the
agent's already-indexed sessions, not just future scans — so the setting can
be a display/report filter over data that stays indexed, which keeps the
Rust surface small. Still open: should OFF-by-default agents stay silently
watched so a later install still surfaces?

**Mechanics** (the flow was built to scale off the tuple, so this is small):

- Add `"agentsDetected"` to `STEPS` and `ANALYTICS_STEP` in
  `OnboardingFlow.tsx`, plus a render branch and an `AgentsDetected`
  component.
- On entry, call `getScanStatus()` to get the filled `agents` array (caveat 1
  above). While the scan is still running, show the rows appearing as agents
  complete, or a short "Still looking…" state.
- Add the step's data to `OnboardingSession` snapshot if needed.
- Analytics: the `onStepViewed` plumbing handles the new step for free once
  it's in `ANALYTICS_STEP`.

**Constraint**: the window is fixed 680×480 and `onboarding.rs` pins the body
height budget with compile-time asserts. Eleven agents can't all fit as fat
rows; a compact list or two-column grid, capped with the detected ones first.

## Feature 2: Ready page tells the reader something useful

Today Ready says "N sessions are indexed and waiting in the menu bar."
Keith's sketch: "304 sessions detected … 40 sessions failed checks".

**The honesty problem**: burn checks won't have run yet (fact 3). Two honest
shapes, in order of preference:

- **Option A — report what's true now, point at what's coming.** Keep the
  session count, add the per-agent breakdown ("304 sessions across Claude
  Code, Codex and Cursor"), and add one forward-looking line: "Burn checks
  are running in the background — results appear on each session in the menu
  bar." No fake numbers, no waiting.
- **Option B — live counter.** Ready polls check progress and the line
  updates in place: "Checked 61 of 304 sessions so far — 9 have burn
  findings." Real numbers, more moving parts: needs a new aggregate-hygiene
  Rust command (current `get_session_hygiene` is per-listed-row, and the
  Insights report is a 30-day reduction that is `pending` at first run), plus
  polling in a window the reader may close at any moment.

Plan assumed **Option A** for this PR and filed Option B as a follow-up once
an aggregate hygiene count exists — that command is also what a future
"weekly summary" nudge would need, so it earns its keep twice. If Feature 1's
step already shows the per-agent breakdown, Ready's version compresses to
one sentence so the two steps don't repeat a table.

**Reopened and decided 2026-08-31** after the throughput measurement (fact
3) showed the whole 14-day window analyzed in ~46s. Keith's call (chat):
show **real analysis, stylized attractively, in its own card** —

> 234 Sessions analyzed
> 80% pass the session checks
> Most common failure: Overpowered subagents

("Overpowered subagents" was illustrative; the card uses the real check
names — reasoning overkill, excess cache rehydration, bloated initial
context.) While results are not yet available, the card shows a **progress
bar over the number of sessions** ("Analyzing sessions", checked X of Y).
Only if real progress turns out too hard, fall back to a faked ~10-second
bar. Real progress should not be hard: the same aggregate-hygiene command
that powers the card can report checked/total, so the fake bar is a
last-resort fallback, not the plan.

Consequences: the aggregate-hygiene Rust command moves **into scope** for
this PR (it also serves the future weekly-summary nudge), and Ready needs a
light poll while the bar is showing. Option A's plain sentence survives as
the zero-sessions state.

**v4 proto refinements (2026-08-31)**: stat figures use the mono font
(`fonts.mono`), matching the app's number styling; the analyzing state is a
bare lightweight bar (no card); the launch-at-login toggle is a plain
single line, not a card, joined by a new **"Nudges respect Do Not Disturb"**
line, default OFF (Focus-status authorization — `notifications.rs` already
has the plumbing; the "requires permissions" label was cut in the v5 round —
the permission prompt appears on first toggle instead).

**v5 layout rule**: the results stat card and the analyzing progress bar
render inside a fixed-height slot (88px in the proto), so the Ready heading
and toggle rows sit at identical positions in both states — no reflow when
analysis finishes. Verified pixel-identical in the v5 screenshot. Welcome and
Ready hero headings move up the documented scale to `type-title-1` with
`type-body` copy. Removed from onboarding entirely in the v3/v4 review
rounds: Welcome's network/analytics paragraph, Ready's analytics +
review-source card, and Ready's nudges one-liner (Feature 3's onboarding
tie-in is dead; the pane explainer stands alone). Flag: analytics now has
no pre-consent disclosure anywhere in onboarding — raised with Keith
2026-08-31, unresolved.

## Feature 3: Explain nudges as a feature

Keith's note says "on the alerts page" — **there is no alerts page**. The
closest surface is Settings → Notifications
([NotificationsPane.tsx](../../apps/desktop/src/views/settings/NotificationsPane.tsx)),
which already lists every notification kind, placement, auto-dismiss, and a
debug-only sample row. See the open question below; assuming the
Notifications pane is the place:

- Add a short explainer at the top of the pane: what a nudge is (antiburn's
  own small notification window, not a macOS notification), when one
  appears (only for the listed kinds — nothing else interrupts), and that
  each kind can be turned off individually.
- Promote the "Sample notifications" row from debug-only to always-on for
  one representative kind ("Show me one") so a reader can see a nudge
  before deciding what to allow. The debug row keeps the full kind list.
- Optional tie-in to onboarding: one sentence on the Ready step ("antiburn
  nudges you when a session burns hot — choose what interrupts you in
  Settings → Notifications"). Cheap, and it makes the feature discoverable
  at the moment the reader is deciding what this app does.

## Feature 4: sourcesAndRepos step redesign (progressive disclosure)

Added 2026-08-31 after a proto round (three variants, then two single-column
revisions; final: `scratchpad proto repo-search-disclosure/single-v3.html`,
picked "A as a single vertical"). The current two-column step becomes one
vertical column:

- **Heading is a question (Keith, chat 2026-08-31)**: "Where should antiburn
  look for your Claude Code, Codex and Cursor repos?" — detected agent names
  joined naturally, "your repos" when none are detected. The step follows the
  agents step, so the names are already known. Voice stays "antiburn", not
  "we", matching the rest of the flow.

- The 8 readable default roots collapse to one disclosure row: "Searching 8
  standard code folders ▸" (expands to the path list). Individual paths are
  never shown at rest.
- **v5 round (2026-08-31)**: the search-location rows (the disclosure
  summary, the expanded path list, and the locked row) use the mono font
  with leading check glyphs, styled like the app's session-check rows. The
  locked row swaps its check for a lock glyph until access is granted.
- The locked `~/Documents/GitHub` row stays visible on its own line ("needs
  permission"), with the "Add a folder…" button inline on the same row. The
  "Added by you" section header is gone; user-added folders appear as rows
  under the summary.
- "Repos found" and its permission card sit below and take the rest of the
  window; found repositories fill that space.
- The "Repositories appear once a coding session has run in one." explainer
  is **cut entirely** (v5 round) — the empty state is just the
  "Nothing found yet" illustration, no explainer sentence.
- Off-macOS the locked row and permission card never render (no consent
  concept on Windows/Linux), so the step is just summary row + results.

## Locked design decisions

- Copy says "found N sessions", never "installed" (fact 2).
- No fabricated or premature check numbers on Ready (fact 3). Mock hygiene
  data is gone from the codebase and stays gone.
- The new step derives everything (dots, step count, Back) from the `STEPS`
  tuple — no parallel bookkeeping.
- Design tokens per `apps/desktop/design.md`; agent icons only via the
  `renderAgentIcon` slot.

## Out of scope

- ~~Aggregate hygiene count command + live Ready counter (Option B)~~ —
  moved into Feature 2's scope 2026-08-31.
- Live per-agent scan progress (Rust drops agent identity in the scan
  progress callback today); the step fetches after completion instead.
- Any new nudge kinds (e.g. a "weekly burn summary" nudge) — separate
  feature.
- A standalone "Alerts" page, unless the open question says otherwise.

## Open questions for Keith

1. **"Alerts page"** — answered (discuss, 2026-08-31): (a) Settings →
   Notifications. The one-line Ready-step pointer stays unless Keith cuts it.
2. Ready page — answered (discuss, 2026-08-31): Option A for this PR;
   Option B and its aggregate-hygiene command stay in Out of scope.
3. Agents-detected step — answered (discuss, 2026-08-31): include
   zero-session agents, OFF by default, as toggles. Two follow-up questions
   pending on thread u-3: what OFF means for already-indexed sessions, and
   whether OFF agents stay silently watched.
4. PR split — answered (discuss, 2026-08-31): three PRs, one per feature;
   Feature 2 stacks on Feature 1 since both touch `OnboardingFlow.tsx`.
