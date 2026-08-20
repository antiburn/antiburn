---
artifact: master_plan
issue: GH-89
title: "Adopt aislop as a diff-scoped CI gate and document contributor setup"
created_by: master_planner
created_at: "2026-08-20"
---

# Adopt aislop as a diff-scoped CI gate and document contributor setup

- **Issue:** [#89](https://github.com/antiburn/antiburn/issues/89) — *Adopt aislop as a diff-scoped CI gate and document contributor setup*
- **Date:** 2026-08-20
- **Verified against:** `feat/gh89` @ `0137f15` (equals `main` content for every file this plan touches) — prefer symbol and file names over line numbers. The promoted-finding footprint below is measured at `609d293`, which differs from `0137f15` only in this plan document.

## Overview / Problem

The repository has no agent-slop quality gate. CI enforces format, clippy, tests,
source boundary, dependency licenses, and DCO (`.github/workflows/ci.yml`), and
nothing judges the shape of the code an agent writes: oversized files, oversized
functions, stub bodies, narrative comments, hardcoded IDs and URLs, non-test
`unwrap()`.

`aislop` 0.14 scores this tree **57 / 100** today (`aislop scan`, 322 files, 0
errors, 33 warnings). A whole-repository score gate is therefore red on day one
and unusable. A diff-scoped gate is green on day one and still catches new
offenders, so it can land while the GH-70 Local Insights stack is still one
unmerged base commit. The actionable ordering boundary is stated once in Risks:
land before the GH-70 implementation seams begin. After a GH-70 child seam
branches, adding a check means rebasing the stack.

Two facts shape the design and were measured, not assumed:

1. **Nothing is repository-enforced today.** `main` has no branch protection and
   no branch rules, so `ci-required` is an aggregate job, not a required check
   (evidence below). This plan owns that gap: CH-005 creates the rule, so the
   gate is genuinely required once CH-005 lands, and advisory before it.
2. **Every slop class GH-89 names is `warn` by default, and warnings never fail
   `aislop ci`.** Teeth come from promoting named rules to `error` in the config,
   which is supported and measured below. A low `ci.failBelow` alone yields a
   gate that cannot fail. Promotion and threshold choice pull in opposite
   directions, and Decision 15 states how the plan resolves that.
3. **Promotion makes 20 existing findings in 10 files error-severity, and the
   plan accepts that cost.** The gate never scans an unchanged file, so those
   findings are silent until a pull request touches one of these 10 files:

   | File | Promoted findings |
   | --- | --- |
   | `apps/desktop/src/lib/ipc.ts` | 10 × `ai-slop/empty-function` |
   | `apps/desktop/src/components/activity/TruncatedText.tsx` | 1 × `ai-slop/empty-function` |
   | `apps/desktop/src/components/activity/useActivityGroupPinning.ts` | 1 × `ai-slop/empty-function` |
   | `apps/desktop/src/lib/useDialogDismissal.ts` | 1 × `ai-slop/empty-function` |
   | `apps/desktop/src/lib/useElementWidth.ts` | 1 × `ai-slop/empty-function` |
   | `apps/desktop/src/lib/useGlobalKeydown.ts` | 1 × `ai-slop/empty-function` |
   | `apps/desktop/src/views/popover/PopoverSession.ts` | 1 × `ai-slop/narrative-comment` |
   | `apps/desktop/src-tauri/src/provider_usage/live/sources/cooldown.rs` | 1 × `ai-slop/meta-comment` |
   | `apps/desktop/src-tauri/src/provider_usage/live/sources/codex_fetch.rs` | 1 × `ai-slop/hardcoded-url`, 1 × `ai-slop/hardcoded-id` |
   | `apps/desktop/src-tauri/crates/sound/src/synth.rs` | 1 × `ai-slop/rust-non-test-unwrap` |

   A pull request that edits any line of one of these files is **red** until the
   finding is resolved. A touched file is judged whole. `aislop ci` has no
   baseline mechanism, so this plan adds none. Decisions 14 and 16 state the
   accepted cost and the contributor escape hatch.

Clearing the standing findings is issue #90, not this work.

## Goals

- A CI job that judges each pull request on **its own changed files** and fails on
  the named slop classes.
- A repository rule on `main` that makes `ci-required` an enforced check, so the
  job blocks a merge. The seam commits the exact ruleset definition; a repository
  admin applies it (CH-005).
- Committed, reviewed `aislop` configuration: measured thresholds, promoted
  severities, third-party telemetry off, an exact version pin.
- Contributor documentation: how to run the gate locally, what it judges, an
  accurate sign-off note beside the existing DCO block, and the named files that
  fail the gate when a pull request touches them.
- Green at enablement: every open pull request head at enablement time passes the
  new gate against `main` before the rule is switched on, or its red finding is
  resolved by a Decision 16 route (the contributor fixes it, or adds the
  justified `#90` directive), or the pull request merges or closes first. This is
  a diff-scoped claim about the heads that are open then, not a whole-tree
  zero-error claim.

## Current State (evidence)

### Repository CI

- `.github/workflows/ci.yml` holds every gate in one workflow. `classify`
  (`scripts/classify-ci-changes.mjs:classifyPaths`) emits `docs_only`, `frontend`,
  `engine`, `desktop_backend`, `full`, `release_app`, `release_engine`; most jobs
  key off it. Two jobs do not: `boundary` runs unconditionally, `dco` runs
  `if: github.event_name == 'pull_request'`, reads
  `github.event.pull_request.base.sha`, and checks out with `fetch-depth: 0`.
- `ci-required` aggregates. Its `require_success` / `allow_skip` /
  `require_if_selected` shell helpers name every job; a job missing from `needs`,
  from the `env` block, or from a helper call is silently unenforced.
- `scripts/verify-workflow-portability.test.mjs:namedStep` asserts on exactly one
  step today ("Compile the release target without bundling or signing"). No test
  covers `ci-required`. The suite runs inside `classify` by `node --test` with
  `classify-ci-changes.test.mjs`, `verify-app-engine-release.test.mjs`,
  `wait-for-main-ci.test.mjs`.
- `scripts/check-boundary.mjs` scans the whole tree (`.yml`, `.md`, `.toml`,
  `.json`, source) for `FORBIDDEN_ANY_CASE` tokens; only its `EXEMPT` set escapes.
- Root `package.json` (`antiburn-workspace`) is a private pnpm workspace root with
  no dependencies, `packageManager: pnpm@11.6.0`, `engines.node >= 22`; CI uses
  `NODE_VERSION: "24"`. `scripts/sbom-frontend.mjs` builds the SBOM from
  `pnpm -C apps/desktop licenses list --prod`, so a root devDependency never
  reaches a shipped SBOM.
- `CONTRIBUTING.md` holds the `git commit -s` block (**Ground rules**), the
  disclosure boundary (**Boundaries that pull requests must respect**), and the
  local command list (**Development**). It names no slop gate.
- No `.aislop/` directory and no aislop reference anywhere
  (`grep -ril aislop` → no match).

### Live GitHub enforcement (checked 2026-08-20)

- `gh api repos/antiburn/antiburn/rules/branches/main` → `[]`.
- `gh api repos/antiburn/antiburn/rulesets` → two active rulesets,
  `release-tag-creation` and `release-tag-immutability`, both `"target":"tag"`.
- `gh api repos/antiburn/antiburn/branches/main/protection` → HTTP 404.
- `gh api repos/antiburn/antiburn` permissions for this session:
  `admin:false, maintain:false, push:true`.
- The open-pull-request set is **a moving target, so this plan names a command,
  not a list**. The enablement sweep evaluates every open pull request head at
  enablement time, listed by
  `gh pr list --state open --json number,author,isDraft,headRefName`.
  Dated snapshot, **illustrative only, not a claim any seam depends on**
  (2026-08-20): 13 open pull requests — #77 – #85 are `app/dependabot` bumps,
  #86, #87 and #88 are authored by `martyportier` and carry real code, and #91
  is the draft GH-70 base pull request. PR #40 is **CLOSED**; #86
  ("crate-backed floating usage HUD") supersedes it.
- **A known red head exists at the snapshot.** #86 adds 6 lines to
  `apps/desktop/src/lib/ipc.ts` (`gh pr diff 86`), and that file carries 10
  `ai-slop/empty-function` findings. The rule is threshold-independent
  (Decision 15), so no threshold choice clears it. Decision 16 gives the only
  routes: the contributor fixes the finding, or adds the justified `#90`
  directive, or the pull request merges or closes before enablement.

### Measured aislop behavior (my runs, this checkout)

- `aislop scan .` → score 57, 322 files, 0 errors, 33 warnings; engines
  format 0, lint 0, code-quality 28, ai-slop 20, security 0. The 20 `ai-slop`
  findings are 15 `info` and 5 `warning` at default severity, and they are the
  same 20 findings that the promoted config reports as errors. Format and lint are
  clean, so the bundled biome/oxlint do not fight the repository's own
  formatters.
- `aislop scan -d` largest offenders: `opencode.rs` 3516, `cursor.rs` 3197,
  `scanner.rs` 2855, `discovery/mod.rs` 2023, `platform/git.rs` 1948,
  `src-tauri/src/scan.rs` 1803, `apps/desktop/src/lib/ipc.ts` 1310.
- **Thresholds are per-language multiplied.** With `maxFileLoc: 300` the reported
  caps were `.ts` 300 and `.tsx` 450; with the default 400 the repo scan reported
  `.rs` 1000, `.tsx` 600, `.ts` 400. A threshold is not readable as a line count.
- **`complexity/function-too-long` mis-measures the Rust discovery files:**
  `cursor.rs:1827 sqlite_table_string_rows · 1371 lines` inside a 3197-line file.
  A threshold that passes today's tree must clear 1371 lines for `.rs`, which
  makes the rule advisory at enablement (Decisions 8 and 15).
- **`aislop rules` severity catalog:** `complexity/file-too-large`,
  `complexity/function-too-long`, `ai-slop/empty-function`,
  `ai-slop/narrative-comment`, `ai-slop/meta-comment`, `ai-slop/hardcoded-id`,
  `ai-slop/hardcoded-url`, `ai-slop/rust-non-test-unwrap`, `ai-slop/todo-stub`
  are all `warn`. Only `ai-slop/hallucinated-import` and the `security/*` rules
  are `error` by default.
- **Warnings do not fail, and the score barely moves.** Probe: one oversized file
  in a one-file diff → score 98, exit 0. `scoring.smoothing: 20` and
  `maxPerRule: 40` make a `failBelow` floor a blunt lever on small diffs.
- **Severity promotion works and gives the gate teeth.** Probe: a config
  `rules:` map with `complexity/file-too-large: error` → score 95, exit 1.
  A second probe with `ai-slop/empty-function`, `ai-slop/hardcoded-url` and
  `ai-slop/rust-non-test-unwrap` promoted → 3 errors, exit 1. Removing the map
  returned the same tree to exit 0.
- **Diff scoping is proved by identity, not by count.** Paired probe in a clone
  of `main` @ `49a20be`, with every Decision 7 rule promoted. Run A commits one
  clean new `probe_scratch.ts`: `Scope 1 changed vs HEAD~ file(s)`, 0 issues,
  exit 0, and `codex_fetch.rs` is absent although it carries a known
  `ai-slop/hardcoded-url` error at line 82. Run B appends one comment line to
  `codex_fetch.rs`: `Scope 1 changed vs HEAD~ file(s)`, and the run names
  `.../codex_fetch.rs:82` (hardcoded URL) and `:88` (hardcoded ID) as `[ERROR]`,
  exit 1. The same file is silent when unchanged and named when touched, so the
  evaluated set is observable by path. `summary.files` in the JSON is the project
  file count, not the scanned scope. The base commit must exist locally, so the
  job needs `fetch-depth: 0` like `dco`.
- **A touched file is judged whole, not by changed lines.** Run B changed a
  trailing comment and the pre-existing findings at lines 82 and 88 failed the
  run. Editing any line of a file with a promoted finding turns that pull request
  red.
- **Promoted-set footprint today: 20 error findings in 10 files.** Whole-tree
  `aislop scan --json` at `609d293` with all Decision 7 rules promoted:
  `ai-slop/empty-function` 15 — 10 in `apps/desktop/src/lib/ipc.ts`, and one each
  in `apps/desktop/src/components/activity/TruncatedText.tsx`,
  `apps/desktop/src/components/activity/useActivityGroupPinning.ts`,
  `apps/desktop/src/lib/useDialogDismissal.ts`,
  `apps/desktop/src/lib/useElementWidth.ts` and
  `apps/desktop/src/lib/useGlobalKeydown.ts`; `ai-slop/narrative-comment` 1
  (`apps/desktop/src/views/popover/PopoverSession.ts:417`);
  `ai-slop/meta-comment` 1
  (`apps/desktop/src-tauri/src/provider_usage/live/sources/cooldown.rs:306`);
  `ai-slop/hardcoded-url` 1 at line 81 and `ai-slop/hardcoded-id` 1 at line 87,
  both in
  `apps/desktop/src-tauri/src/provider_usage/live/sources/codex_fetch.rs`;
  `ai-slop/rust-non-test-unwrap` 1
  (`apps/desktop/src-tauri/crates/sound/src/synth.rs:152`).
  `ai-slop/todo-stub` has zero instances. `ipc.ts` is a hot file, so it carries
  the most friction.
- **Measured loose complexity thresholds.** `maxFunctionLoc: 600` still reports
  `Function too long (max: 900)` three times in `cursor.rs`, so the Rust function
  multiplier is 1.5. With `maxFileLoc: 1500` and `maxFunctionLoc: 1000` the whole
  tree reports zero `complexity/*` errors. These numbers are measured, and the
  seam confirms them against the base commit it lands on.
- **No baseline mechanism for CI.** `aislop commands` lists only `.aislopignore`
  and `.gitignore` as scope files, and `aislop hook baseline` captures a score
  for local hooks, not a CI suppression set. The binary does implement inline
  `aislop-ignore-next-line`, `aislop-ignore-line` and `aislop-ignore-file`
  comment directives (`src/utils/aislop-ignore.ts` in `dist/index.js`), and a run
  reports `Suppressed N finding(s) via aislop-ignore directives`. A directive is
  the contributor escape hatch of Decision 16; no seam here writes one.
- `aislop init` is interactive; `aislop init --strict </dev/null` writes
  `.aislop/config.yml`, `.aislop/rules.yml`, `.github/workflows/aislop.yml`. The
  generated config carries `quality.maxFunctionLoc: 80`, `maxFileLoc: 400`,
  `maxNesting: 5`, `maxParams: 6`, `lint.typecheck: true`, `security.audit: true`,
  `ci.failBelow: 85`, and **`telemetry.enabled: true`**. The generated workflow
  uses `scanaislop/aislop@v1` with `version: latest` — two unpinned references.
- `aislop doctor`: 5 engines ready (format/biome, lint/oxlint, code-quality/knip,
  ai-slop, security/pnpm audit); architecture skipped.
- Local binary is 0.14.0; npm `latest` is 0.14.1.

## Desired End State

- `.aislop/config.yml` is committed and self-documenting: measured thresholds set
  loose enough for today's tree, an explicit promoted-to-`error` rule set, engine
  choices, `telemetry.enabled: false`, ASD-STE100 comments recording ratchet
  intent and #90.
- `aislop` is available at one exact pinned version to CI and contributors
  through the same command.
- `ci.yml` runs `aislop ci --changes --base <PR base sha>` on pull requests and
  `ci-required` enforces it; a script test fails if that wiring regresses.
- A repository rule on `main` requires the `ci-required` check, and its
  definition is committed in the repository.
- `CONTRIBUTING.md` documents the gate, the local command, the diff-scoped
  semantics, the ratchet policy, the 10 files that fail the gate when a pull
  request touches them, the escape hatch of Decision 16, and the `git commit -s`
  sign-off note.

## Locked Decisions

1. **One workflow.** The job lives in `.github/workflows/ci.yml` and is listed in
   `ci-required`; the generated `.github/workflows/aislop.yml` is unused. Every
   other gate is enforced through `ci-required`, and a second required workflow
   splits that contract.
2. **Diff-scoped, not repository-scoped.** `aislop ci --changes --base <base sha>`,
   with `actions/checkout` at `fetch-depth: 0` so the base commit resolves.
   Measured: the run reports `Scope N changed vs <base> file(s)` and ignores
   unchanged legacy offenders. The whole-repository score is 57, so only a
   diff-scoped gate is green on day one.
3. **Pull-request only.** The job mirrors `dco`:
   `if: github.event_name == 'pull_request'`, `require_success` on
   `pull_request`, `allow_skip` on `push`. `--base` needs
   `github.event.pull_request.base.sha`, which does not exist on push.
   Consequence, stated plainly: on a push to `main` the aislop job is skipped and
   "main is green" means `ci-required` succeeds with aislop skipped. This is a
   pre-merge gate by construction. Possible future follow-up, not scope here: a
   push-scoped variant that compares against the previous commit.
4. **Not classify-gated.** Like `boundary` and `dco`, the job runs whatever
   `classify` emits. A diff run takes seconds and slop can enter any path.
5. **Exact version pin, no `latest` and no caret.** The seam pins the exact
   version it verifies against, in one place. An unpinned linter that gains a
   rule turns an unrelated PR red.
6. **Third-party telemetry stays off.** `telemetry.enabled: false` is written
   into the committed config, not inherited. `CONTRIBUTING.md` permits exactly
   two first-party calls — the release-feed update check and the D-027 / D-28
   anonymised usage-analytics channel — and bans every other telemetry or
   analytics channel. A contributor tool that reports to its vendor is not one of
   the two permitted calls, so it stays disabled.
7. **Teeth come from promoted severities, not from a score floor.** The config
   `rules:` map promotes every slop class GH-89 names to `error`, because
   measured warnings never fail `aislop ci` and one warning in a small diff costs
   about two score points. The set is `complexity/file-too-large`,
   `complexity/function-too-long`, `ai-slop/empty-function`,
   `ai-slop/narrative-comment`, `ai-slop/meta-comment`, `ai-slop/hardcoded-id`,
   `ai-slop/hardcoded-url`, `ai-slop/rust-non-test-unwrap`,
   `ai-slop/todo-stub`. Each promotion is proved by a negative check (FR-9).
8. **`complexity/function-too-long` is promoted, and it is advisory by
   threshold.** The rule mis-measures `cursor.rs` as one 1371-line function, so
   the effective `.rs` cap must clear that number for today's tree to pass. No
   realistic new function reaches it. The plan states this plainly instead of
   pretending the rule bites: at enablement the promotion carries the severity,
   not the sensitivity. The threshold tightens under #90, when the
   mis-measurement is investigated. The config records both facts in an
   ASD-STE100 comment.
9. **`ci.failBelow` is set low and is not the gate.** It exists so #90 can raise
   it as findings clear.
10. **The `security` engine is off in the committed config.** Rust advisories run
    in the `licenses` job: `cargo deny --locked check advisories` for both
    `crates/antiburn-local` and `apps/desktop/src-tauri`
    (`.github/workflows/ci.yml:licenses`). That job checks no JavaScript
    advisory, and `apps/desktop/package.json` ships production dependencies
    (`react`, `react-dom`, `@radix-ui/*`, `@tauri-apps/*`, `recharts`,
    `lucide-react`, `simple-icons`). The accepted gap is stated, not denied:
    JavaScript advisories are covered only by Dependabot, which watches the `npm`
    and `cargo` ecosystems (`.github/dependabot.yml`). A new npm advisory must
    not turn an unrelated pull request red, which is the stability property that
    lets this land before the GH-70 implementation seams. Closing the gap with an advisory check
    is separate work.
11. **`lint.typecheck: false`.** `pnpm --filter @antiburn/desktop type-check`
    already runs in `desktop-frontend`; a second type-check would need a full
    workspace install inside the diff job for no new signal.
12. **The `architecture` engine stays off and no `.aislop/rules.yml` is
    committed.** No architecture rule is defined yet.
13. **This plan owns branch enforcement.** The `main` rule requiring
    `ci-required` is a scope outcome (CH-005), not an assumption and not a
    dependency on another issue. The seam commits the exact ruleset definition
    as a reviewable artifact, plus the apply and verify commands. A repository
    admin runs the apply step, because the agent session holds `admin:false`.
    CH-005 is verified only by the live API result, not by the committed file.
    Before CH-005 lands the check is advisory; after it lands the gate is
    required, and GH-89's "required gate" framing is exact.
14. **No standing-finding cleanup by these seams.** No seam here edits product
    runtime code, and no seam pre-seeds a suppression directive. #90 clears the
    standing findings. This decision binds the seams of this plan; it does not
    bind a contributor, who keeps the Decision 16 escape hatch.
15. **Promote every rule, and set thresholds loose enough for today's tree.**
    The gate must catch new slop at enablement, not fail on legacy files, so
    severity and threshold carry different jobs. Severity is maximal: every rule
    in Decision 7 is `error`. Each threshold is the smallest value that passes
    the whole tree today with a stated margin, measured by scan, never guessed,
    and remembering that thresholds are language-multiplied. This splits the
    promoted set in two, and the config comment names both halves:
    - **Threshold-independent, the real teeth:** `ai-slop/empty-function`,
      `ai-slop/narrative-comment`, `ai-slop/meta-comment`,
      `ai-slop/hardcoded-id`, `ai-slop/hardcoded-url`,
      `ai-slop/rust-non-test-unwrap`, `ai-slop/todo-stub`. Any new instance
      fails the build on day one. No threshold can loosen these rules, so
      promoting them accepts the red-on-touch cost of Decision 16.
    - **Threshold-bounded, advisory at enablement:** `complexity/file-too-large`
      (must clear `opencode.rs` at 3516 lines after the Rust multiplier) and
      `complexity/function-too-long` (Decision 8). Both catch only extreme new
      code until #90 tightens them.
    The ratchet path is #90 and only #90. No seam here tightens a threshold.
16. **All 9 rules ship at `error`, red-on-touch is normal ratchet cost, and a
    contributor has a documented escape hatch.** The alternatives lose more:
    promotion of only the zero-instance rules leaves the gate nearly toothless,
    and cleaning the 20 findings now pulls #90 work into this issue. So the 10
    files of the Overview table stay red on touch. A contributor unblocks such a
    pull request in one of two ways: fix the finding, or add an `aislop-ignore`
    directive that carries a justification comment and a `#90` reference. The
    directive token stays verbatim, because it is machine-read; its justification
    prose is ASD-STE100 (`AGENTS.md`, "Comments"). CH-004 documents both ways.

## Invariants & Constraints

- Every commit carries a DCO sign-off — `git commit -s` (`AGENTS.md`,
  "Commits"). The `dco` job fails the PR on any single unsigned commit.
- All code comments, including YAML and workflow comments, are ASD-STE100:
  active voice, present tense, one idea per sentence, instructions ≤ 20 words
  (`AGENTS.md`, "Comments"). A comment states what the code cannot show.
- No React `useEffect` and no Rust dead/deprecated lint suppression without
  explicit dev agreement (`AGENTS.md`). No seam here may silence a finding in
  product code that way.
- The repository's boundary is **disclosure, not technique** (`CONTRIBUTING.md`,
  "Boundaries that pull requests must respect"). antiburn reaches no service of
  ours and hands the reader's data to no one who does not already hold it. Two
  first-party calls are permitted and no more: the release-feed update check, and
  the anonymised usage-analytics channel recorded as D-027 and deviations D-28,
  which must keep its four properties. No other telemetry or analytics channel
  may enter the tree; `scripts/check-boundary.mjs`,
  `crates/antiburn-local/tests/boundary.rs`, and
  `apps/desktop/tests/no-exfiltration.test.ts` enforce this mechanically.
- Every new text file is scanned by `scripts/check-boundary.mjs` for the
  `FORBIDDEN_ANY_CASE` tokens. Do not add a file to its `EXEMPT` set to pass.
- `docs/oss/*` manifests are governance records; changes need a
  maintainer-approved governance decision (`CONTRIBUTING.md`, final boundary
  bullet).
- `ci-required` must stay fail-closed: every job appears in `needs`, in the `env`
  block, and in a `require_*` call.
- The CI control-plane tests (`node --test scripts/*.test.mjs` in `classify`)
  must stay green after any `ci.yml` edit.
- CI pins third-party actions by commit SHA (every `uses:` line in `ci.yml`). Any
  new external dependency is pinned with equal strength.
- Documentation-only changes stay documentation-only: `isDocumentation` in
  `scripts/classify-ci-changes.mjs` covers `docs/**` and `CONTRIBUTING.md`.

## Definition of Done (applies to every seam)

- `aislop ci --changes --base origin/main` passes for the seam's own diff.
- `node --test` over the four control-plane test files passes when the seam
  touches `ci.yml` or `scripts/`.
- `node scripts/check-boundary.mjs` reports no violation in a tracked path.
- Every commit is signed off; every added comment is ASD-STE100.
- No product runtime code changes.

## Patterns & Utilities to Reuse

- `dco` job in `ci.yml` — pull-request-only job, `fetch-depth: 0`, reads
  `github.event.pull_request.base.sha`, with matching `allow_skip` handling.
- `boundary` job — cheap unconditional gate shape.
- `ci-required` helpers `require_success` / `allow_skip` — reuse, do not invent a
  new enforcement shape.
- `scripts/verify-workflow-portability.test.mjs:namedStep` — the existing way to
  assert on `ci.yml` text from `node --test`.
- `CONTRIBUTING.md` **Development** section for local commands; **Ground rules**
  for the sign-off note.
- pnpm tooling already in CI: `corepack enable`, `pnpm install --frozen-lockfile`.

## Functional Requirements

- **FR-1:** A committed `.aislop/config.yml` sets every value the gate uses; no
  default is inherited silently for `engines`, `quality`, `rules`, `ci`, or
  `telemetry`.
- **FR-2:** `telemetry.enabled` is `false`.
- **FR-3:** The aislop version used by CI is an exact pin, and a contributor runs
  the same version from a documented command.
- **FR-4:** CI runs `aislop ci --changes --base ${{ github.event.pull_request.base.sha }}`
  on pull requests, inside `.github/workflows/ci.yml`, with the base commit
  fetched.
- **FR-5:** `ci-required` fails when that job fails on a pull request and
  tolerates its skip on push.
- **FR-6:** With the committed config, every open pull request head passes
  `aislop ci --changes --base <main sha>`, and the whole tree reports zero
  `complexity/*` error findings. The tree is **not** free of promoted `ai-slop`
  errors: the 20 findings in 10 files listed in the Overview stay, and a pull
  request that touches one of those files fails until the finding is fixed or a
  Decision 16 directive suppresses it. A threshold change never clears such a
  finding, because these rules are threshold-independent (Decision 15). The
  plan promises a green diff-scoped gate, not a green whole tree.
- **FR-7:** `CONTRIBUTING.md` documents the gate, the local command, the
  diff-scoped semantics, the ratchet policy, the 10 red-on-touch files, the two
  escape-hatch routes of Decision 16, and the sign-off mechanism. `git commit -s`
  is the documented sign-off command, and the document names no other.
- **FR-8:** The config promotes the Decision 7 rule set to `error`.
- **FR-9:** A seeded violation of each promoted rule makes
  `aislop ci --changes` exit 1. Exit 0 alone is never accepted as evidence.
- **FR-10:** A run proves its scope by identity, not by count: a named file that
  carries a known promoted finding (for example
  `apps/desktop/src-tauri/src/provider_usage/live/sources/codex_fetch.rs:81`) is
  absent from the findings while it is unchanged, and is reported by path when a
  scratch commit touches it. The reported changed-file count matches the diff as
  a second, weaker check.
- **FR-13:** Every threshold is the smallest measured value that keeps the tree
  free of `complexity/*` error findings, with the margin recorded. The config
  names each threshold-bounded rule as advisory at enablement (Decision 15).
- **FR-11:** A `node --test` case fails if the aislop job loses its pull-request
  guard, its `--changes --base` command, its `needs` entry, its result `env`
  binding, its PR `require_success`, or its push `allow_skip`.
- **FR-12:** A repository rule on `main` requires the `ci-required` check, and
  `gh api repos/antiburn/antiburn/rules/branches/main` returns it.
- **FR-14:** The ruleset definition is committed as a reviewable artifact, with
  the apply command, the verify command, and the rollback command.

## Scope Areas (backlog — NOT seams)

- [ ] **CH-001 — Committed, measured aislop configuration.** Acceptance:
  `.aislop/config.yml` exists with explicit `engines`, `quality`, `rules`
  severity promotions, `ci.failBelow`, and `telemetry.enabled: false`;
  `aislop scan` at the base commit reports zero `complexity/*` errors under it
  and the same 20 `ai-slop` errors in the same 10 files that the Overview lists,
  with no new one; a seeded
  violation of each promoted rule exits 1; each threshold records its measured
  value and its margin over today's worst file or function; ratchet intent, the
  #90 reference, and the advisory-by-threshold rules are recorded in ASD-STE100
  comments. Likely touches: `.aislop/`,
  `.gitignore` for any aislop cache directory. Provisional tier: 2.
  (Refs: FR-1, FR-2, FR-6, FR-8, FR-9, FR-13)
- [ ] **CH-002 — Pinned, reproducible aislop entry point.** Acceptance: CI and a
  contributor invoke the same exact version by one documented command; the pin is
  as strong as the repository's SHA-pinned actions; nothing about it reaches a
  shipped artifact or SBOM. Likely touches: root `package.json`,
  `pnpm-lock.yaml`, or the install step of the new CI job. Provisional tier: 2.
  (Refs: FR-3)
- [ ] **CH-003 — Diff-scoped `aislop` job wired into `ci-required`.** Acceptance:
  `ci.yml` runs the gate on pull requests against the PR base SHA with the base
  commit fetched; `ci-required` lists it in `needs`, `env`, and a `require_*`
  call with push skip tolerated; the FR-10 paired probe passes, so a named file
  with a known finding is absent while unchanged and reported by path when
  touched; a seeded violation on the branch
  turns the check red, and removing it turns it green; the sweep runs
  `aislop ci --changes --base <main sha>` against **every open pull request head
  listed by `gh pr list --state open` at sweep time**, and each still-open red
  head is resolved before CH-005 by one of three routes — the contributor fixes
  the finding, the contributor adds an `aislop-ignore` directive with a
  justification and a `#90` reference (Decision 16), or the pull request merges
  or closes. Threshold choice is not a route for a threshold-independent rule.
  Likely touches:
  `.github/workflows/ci.yml`. Provisional tier: 3 — it changes the merge path for
  every contributor. (Refs: FR-4, FR-5, FR-6, FR-9, FR-10)
- [ ] **CH-004 — Contributor documentation.** Acceptance: `CONTRIBUTING.md`
  explains the gate, the local command, that it judges changed files rather than
  the repository score, which rules fail a build, that thresholds are loose today
  and tighten under #90, which two rules are advisory by threshold, that the gate
  runs on pull requests only, and which 10 files fail the gate when a pull request
  touches them. The document states the Decision 16 escape hatch: fix the
  finding, or add an `aislop-ignore` directive with a justification comment and a
  `#90` reference, with the directive token verbatim and the justification in
  ASD-STE100. The sign-off note gives `git commit -s` and no other mechanism.
  Likely touches: `CONTRIBUTING.md`. Provisional tier: 1. (Refs: FR-7)
- [ ] **CH-006 — Automated coverage of the workflow contract.** Acceptance: a
  `node --test` case in `scripts/` fails when any element of FR-11 is removed
  from `ci.yml`; the seam demonstrates each failure by temporary mutation before
  restoring the file. Likely touches:
  `scripts/verify-workflow-portability.test.mjs` or a sibling test file.
  Depends on CH-003 and blocks CH-005: the contract is proved regression-proof
  before the check becomes required. Provisional tier: 2. (Refs: FR-11)
- [ ] **CH-005 — `ci-required` becomes an enforced check on `main`.** This is the
  last outcome. It lands after CH-003 wiring, after CH-006 mutation coverage, and
  after a fresh sweep of every open pull request head at that moment shows no
  unresolved red head. Acceptance: no still-open head is red under the committed
  config, where each red head was cleared by a fix, by a Decision 16 directive,
  or by the pull request merging or closing; `gh api repos/antiburn/antiburn/rules/branches/main` returns an
  active rule of type `required_status_checks` naming `ci-required`; a pull
  request with a failing `ci-required` cannot merge; the ruleset definition is
  committed as a reviewable artifact (a JSON payload file, or a documented
  `gh api` call in repository docs) together with the apply, verify, and
  rollback commands; the rollback disables or deletes that ruleset. **The agent
  session holds `admin:false` and `maintain:false`, so the human applies the
  ruleset as repository admin. The seam is not verified until live
  `gh api repos/antiburn/antiburn/rules/branches/main` output shows a rule
  requiring the `ci-required` check.** Likely touches: a committed ruleset file
  under `docs/` or `.github/`, plus `CONTRIBUTING.md` if the seam adds a note.
  Provisional tier: 3. (Refs: FR-12, FR-14)

> Ordering is a dependency hint, and the IDs are append-only, so CH-006 is
> presented before CH-005. The config precedes the job. The pin precedes CI use.
> The job precedes its contract test. Enforcement is last: CH-005 follows
> CH-003, CH-006, and a fresh sweep of every open pull request head.

## Out of Scope (Non-Goals)

- Clearing the 33 standing findings or tightening thresholds toward the defaults
  — #90.
- Local Insights work — #70.
- SARIF upload to GitHub code scanning (`aislop ci --sarif`).
- `aislop hook install`, `aislop agent`, and the hosted platform.
- The `architecture` engine and a committed `.aislop/rules.yml`.
- The `security` engine (Decision 10).
- Investigating the `complexity/function-too-long` mis-measurement (Decision 8).
- A push-scoped aislop run on `main` (Decision 3).
- Any change to product runtime code, to `docs/oss/*`, or to the boundary
  scripts' pattern tables. No seam here writes an `aislop-ignore` directive
  (Decisions 14 and 16).
- Branch rules beyond the single required-check rule of CH-005.

## Risks & Open Questions

No open questions. Decision 16 settles the promotion and cost question.

- **CH-005 needs a human admin step.** The agent session holds `admin:false` and
  `maintain:false`, so the seam prepares the ruleset and the human applies it.
  The risk is timing, not ownership: until the human applies it, CH-005 stays
  unverified and the gate stays advisory.
- **Threshold numbers are language-multiplied**, so any number must be validated
  by a scan, not compared to a line count.
- **Loose thresholds weaken two rules on purpose** (Decision 15). The risk is a
  false sense of coverage. The mitigation is naming the two advisory rules in
  the config, in `CONTRIBUTING.md`, and in #90, so nobody reads a green build as
  proof that file and function size are policed.
- **Red-on-touch friction lands mostly on one hot file.**
  `apps/desktop/src/lib/ipc.ts` carries 10 of the 20 promoted findings, so it is
  the file most likely to make an unrelated pull request red. The enablement
  sweep over open pull request heads (CH-003) measures the real cost before
  CH-005 switches enforcement on.
- **Version drift:** local binary is 0.14.0, npm `latest` is 0.14.1. The seam
  pins the version it verifies with and says which.
- **Every open pull request gains a new check**, and the set changes daily. The
  sweep therefore reads `gh pr list --state open` at enablement time; the dated
  snapshot in Current State is illustrative. A snapshot head is known red: #86
  touches `apps/desktop/src/lib/ipc.ts`. That rule is threshold-independent, so
  the remedy is a Decision 16 route or the pull request merging or closing.
- **Sequencing:** the GH-70 base pull request (#91, draft, one commit) is already
  open, so that milestone is passed. The remaining actionable boundary is to land
  CH-003 and CH-005 before the GH-70 implementation seams begin, while no child
  branch targets `feat/gh70`. After a child seam branches, adding a required
  check forces a stack-wide rebase.

## Verification Strategy & Success Metrics

Positive and negative evidence are both required; an exit-0 run proves nothing on
its own, because a widened scan can also pass.

- **Config:** whole-tree `aislop scan` under the committed config → zero
  `complexity/*` errors, and exactly the 20 known `ai-slop` errors in the 10 known
  files; record the score and the finding list. A new error-severity finding
  outside that known set fails the check. This proves the loose half of
  Decision 15.
- **Negative, per promoted rule:** seed one scratch file per rule in Decision 7
  (oversized file, oversized function, empty function, narrative comment, meta
  comment, hardcoded id, hardcoded URL, non-test `unwrap()`, TODO stub), run
  `aislop ci --changes --base origin/main`, confirm exit 1 and the expected rule
  ID in the JSON, then discard the file. A rule that cannot be made to fail is
  not enforced. This proves the strict half of Decision 15. The seeds for
  `complexity/file-too-large` and `complexity/function-too-long` must exceed the
  chosen thresholds, and the report states that both rules are
  advisory-by-threshold at enablement.
- **Scope, paired probe by identity (FR-10):** run A commits one clean scratch
  file and asserts exit 0 with
  `apps/desktop/src-tauri/src/provider_usage/live/sources/codex_fetch.rs` absent
  from the findings; run B touches that file in a scratch commit and asserts the
  run names `codex_fetch.rs:82` as an `[ERROR]` and exits 1. Both runs use
  `--human` and record the `Scope N changed vs <base> file(s)` line as the
  weaker cardinality check. This probe ran during planning on a clone of `main` @
  `49a20be` and gave exactly that result. The file changed after `49a20be`. A
  repeat of the probe at `609d293` asserts `codex_fetch.rs:81` for the hardcoded
  URL and `:87` for the hardcoded ID.
- **CI wiring:** `node --test` over the four control-plane files, including the
  new FR-11 case; demonstrate the new case failing under each mutation.
- **Enablement sweep:** for every open pull request head listed by
  `gh pr list --state open` **at sweep time**, run
  `aislop ci --changes --base <main sha>` locally and record the result. A red
  result on a threshold-independent rule cannot be cleared by threshold choice,
  and Decisions 14 and 16 bar these seams from editing product code or writing a
  directive. So each still-open red head is resolved before CH-005 by one of
  three routes: the contributor fixes the finding, the contributor adds an
  `aislop-ignore` directive with a justification and a `#90` reference, or the
  pull request merges or closes. The seam report records the route per head.
- **Enforcement:** after the human applies the committed ruleset,
  `gh api repos/antiburn/antiburn/rules/branches/main` shows the
  `required_status_checks` rule naming `ci-required`. That live output is the
  only accepted evidence for CH-005.
- `node scripts/check-boundary.mjs` for tracked paths. Note: in an agent worktree
  the script also walks untracked `.pi/` and `.seams/` prompt files and exits 1 on
  them. CI checks out neither.
- **Success metrics:** every new pull request runs the gate; a seeded slop finding
  blocks a merge; an unchanged legacy file never turns a pull request red. A pull
  request that touches one of the 10 known files is red by design, and #90 removes
  that cost by clearing the findings.

## Rollback / Safety

Reversible and data-free. Reverting the CI job removes it from `ci-required`,
which recomputes from its own `needs` list. Reverting the config and the pin
removes the tool. Disabling or deleting the CH-005 ruleset restores today's
unenforced state within one API call, and the seam report records that command.
Noise after landing is a one-file fix: raise a threshold, drop a promotion, or
disable an engine in `.aislop/config.yml`. No schema, no user data, no release
artifact, no shipped dependency.

## Progress Log

(Appended as seams land.)
