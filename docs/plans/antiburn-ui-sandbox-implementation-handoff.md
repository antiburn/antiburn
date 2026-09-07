# Phase Handoff: UI sandbox — Planning → Implementation

**Worktree**: `/Users/keithlang/Documents/GitHub/antiburn/.claude/worktrees/antiburn-ui-sandbox-759ec8`
**Branch**: `claude/antiburn-ui-sandbox-759ec8` (same branch — do NOT create a new worktree or branch)
**PR**: None yet
**Date**: 2026-09-07 (Australia/Sydney)
**Run this phase on**: Opus 5, effort High

## You Are Continuing Existing Work

This is a phase handoff, not a new task. The worktree above already has the
work in it. Start there, on that branch. Do not run `/new`.

## What Phase Just Finished

Planning. Keith reviewed the idea in discuss and decided four things: personal
tool only (not shipped), popover first, changes reach Claude in batches behind
a Send button, and adding new UI is an Ask note rather than a drag-in. The
implementation plan was then written and Keith approved it with "Build it".
Both docs are committed on this branch. No code exists yet.

## What To Do Next

Build steps 0 and 1 of the plan only. Steps 2 and 3 are a global skill and come
in a later phase.

1. `git fetch origin && git rebase origin/main`. The branch was cut from a
   stale local main and is 558 commits behind. The only commit is docs, so the
   rebase is trivial. Then re-check that the files named in the plan still
   exist at the same paths (`apps/desktop/src/lib/ipc.ts`,
   `apps/desktop/src/views/PopoverView.test.tsx`,
   `apps/desktop/src/views/popover/PopoverSession.test.ts`,
   `apps/desktop/vite.config.ts`, `apps/desktop/knip.json`).
2. `pnpm install` from the repo root. The worktree has no `node_modules`.
3. `vite.config.ts`: when `mode === "sandbox"`, add `resolve.alias` for
   `@tauri-apps/api/core`, `@tauri-apps/api/event`, `@tauri-apps/plugin-dialog`
   pointing at `apps/desktop/src/sandbox/tauri-core.ts`, `tauri-event.ts`,
   `tauri-dialog.ts`. Add script `dev:sandbox` = `vite --mode sandbox`.
4. `src/sandbox/fixtures.ts`, `scenarios.ts` (`default`, `busy`),
   `commands.ts` (the `invoke` switch, unknown → `null`). Type fixtures against
   the payload types exported from `ipc.ts`.
5. `apps/desktop/sandbox.html`: copy of `index.html` plus inline `<style>`
   sizing `#root` to 380px wide, 700px max height, and an inline loader that
   fetches `http://127.0.0.1:4680/overlay.js` and appends it if it answers.
6. `knip.json`: add `src/sandbox/tauri-*.ts` to `entry`. One README paragraph
   under Commands. Comments in Simplified Technical English.
7. Run the validation below. Open `sandbox.html?scenario=busy` in the Browser
   pane and screenshot it for the PR. Update the Status table in the plan.
8. Open the PR with a placeholder line for the screenshot and tell Keith which
   screen to drop in. `git commit -s` on every commit.

## Key Decisions

- **Vite alias, not a runtime flag.** The discussion doc says `?sandbox=1`;
  the plan replaces it with `--mode sandbox` plus package aliases. Reason: ten
  product files import `@tauri-apps/*` directly and `hasShell()` gates on
  `isTauri()`. Aliasing the packages means zero product-code edits and the
  production build never sees the shims. `?scenario=` stays a query param.
- **Fixtures typed against `ipc.ts`.** A shell-side rename then fails
  `type-check` instead of silently blanking the popover.
- **Inline styles in `sandbox.html`, no new stylesheet.** `AGENTS.md` requires
  every new stylesheet to be listed in `design.md` and the drift check
  enforces it. An HTML file with an inline `<style>` sidesteps that honestly.
- **Overlay loader port 4680 is hard-coded.** Personal tool; acceptable.
- **Test-fixture sharing is optional.** Making `PopoverView.test.tsx` import
  from `src/sandbox/fixtures.ts` is nice but skip it if the test needs many
  overrides.

## Known Issues And Gotchas

- Bash working directory persists between calls in this harness. An earlier
  `cd apps/desktop` broke a relative path. Use absolute paths.
- zsh in this harness rejects `--include=*.ts` globs in grep. Use `grep -r`
  then filter with a second grep.
- No `useEffect` anywhere (`AGENTS.md`). The shims are plain modules; no React.
- `hasShell()` must return true in sandbox mode or every IPC call returns
  early. The shim's `isTauri` returns `true` for that reason.
- `windowReady` invokes `window_ready`; the fixture switch returns `null`,
  which is fine.
- Session detail is a lazy chunk (`SessionPane`). Nothing special needed.
- React 19 fibers have no `_debugSource`. That matters for the overlay
  (later phase), not for this one.
- Every PR needs an image and Keith uploads it. Hand him the screenshot path.

## State Of The Tree

- Commits on this branch: `71d1ffc3 docs: plan the UI sandbox (fixture mode + mouse overlay)`
- Uncommitted files: none — all committed (this handoff doc is added after).

## Validation

```bash
pnpm --filter @antiburn/desktop lint && pnpm --filter @antiburn/desktop type-check && pnpm --filter @antiburn/desktop test && pnpm --filter @antiburn/desktop knip && pnpm --filter @antiburn/desktop build
```

Then `pnpm --filter @antiburn/desktop dev:sandbox`, open
`http://127.0.0.1:1420/sandbox.html?scenario=busy`, and confirm rows, usage
bars, and a session detail that opens on click. `pnpm run slop:all` and
`pnpm run secrets` before pushing.

## Context Docs

- `/Users/keithlang/Documents/GitHub/antiburn/.claude/worktrees/antiburn-ui-sandbox-759ec8/docs/plans/ui-sandbox-implementation.md` — the plan; step 1 is the spec for this phase
- `/Users/keithlang/Documents/GitHub/antiburn/.claude/worktrees/antiburn-ui-sandbox-759ec8/docs/plans/ui-sandbox.md` — the discussion and the four decisions
- `/Users/keithlang/Documents/GitHub/antiburn/.claude/worktrees/antiburn-ui-sandbox-759ec8/AGENTS.md` — repo rules: no useEffect, design.md contract, STE comments, DCO sign-off
- `/Users/keithlang/Documents/GitHub/antiburn/.claude/worktrees/antiburn-ui-sandbox-759ec8/apps/desktop/README.md` — commands
