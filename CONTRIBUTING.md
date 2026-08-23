# Contributing to antiburn

Thanks for your interest in contributing!

## Ground rules

- **License:** the project is licensed under the [Mozilla Public License 2.0](LICENSE).
  By contributing, you agree your contributions are licensed under MPL-2.0.
  There is no CLA.
- **DCO sign-off (required):** every commit must carry a
  [Developer Certificate of Origin](https://developercertificate.org/) sign-off:

  ```bash
  git commit -s
  ```

  which adds a `Signed-off-by: Your Name <you@example.com>` trailer. CI rejects
  pull requests containing unsigned commits.

## Boundaries that pull requests must respect

antiburn is a **local** application in one exact sense: it needs no connection
to any service of ours. There is no antiburn account, server, or backend, and
there never has to be one — everything antiburn does happens on the reader's own
machine, as the reader. That is the whole of what "local" claims, and CI
enforces exactly that much and no more.

antiburn watches cloud coding agents — tools that only work connected to
their model providers — so it is an ordinary online application about online
tools. It is the reader's own agent, running in the reader's own security
context, and may do anything the reader could do on their machine: read the
credential and configuration files the reader's tools wrote, call a provider's API with the reader's own
credentials, inspect local processes and ports, call a locally-running agent
over loopback, run the reader's tools as child processes. None of that is fenced
off, because none of it depends on us or discloses anything the reader has not
already disclosed. A pull request needs no ceremony to add such a capability —
technique is not the boundary.

- **The one hard line is that antiburn reaches no service of ours, and hands the
  reader's data to no one who does not already have it.** It sends nothing to an
  antiburn-operated server (there is none), nothing to a third party, and
  nothing to a telemetry or analytics endpoint. Returning the reader's own
  credential to the provider that issued it, to read that provider's own
  figures, is not a disclosure: the provider already holds both. This is the
  boundary CI keeps mechanically (`crates/antiburn-local/tests/boundary.rs`,
  `scripts/check-boundary.mjs`, `apps/desktop/tests/no-exfiltration.test.ts`):
  no telemetry or analytics SDK, and no reporting endpoint carrying the reader's
  work, may enter the tree. Two first-party calls are permitted and no more: the
  update check — antiburn's own release feed, carrying nothing about the reader —
  and the anonymised usage-analytics channel recorded as D-027 and deviations
  D-28. Four properties keep the second one inside this boundary and must all
  hold for any change to it: the reader is shown the control before a single
  event is sent, the payload carries no session content and no credential, the
  installation identifier rotates so events cannot be joined into a history, and
  a build with no configured endpoint transmits nothing. Neither call is
  something the app needs to function.
- **Genuinely risky local operations still earn care.** Deleting or modifying
  the reader's files, terminating processes, or anything that could damage the
  machine or the reader's standing with a provider needs clear, present intent —
  an explicit action, never a silent background pass. A credential, once read,
  stays in memory: never written somewhere new, logged, or left where a crash
  report could carry it off. When a capability could plausibly cost the reader
  something, the pull request names the cost and how it is bounded, and a
  decision of that shape is recorded in `docs/deviations.md`.
- Test fixtures must be **synthetic**: no real transcripts, usernames, home
  paths, repository names, or captured machine output — redaction is not
  sufficient. The more antiburn is trusted to read, the less any of it belongs
  in the repository.
- **Performance and memory use are product constraints.** antiburn is an
  always-running background utility: avoid eager or repeated work, keep reads,
  allocations, concurrency, and retained data bounded, and do not load the
  reader's machine beyond what the visible feature actually needs.
- The source-boundary manifests in `docs/oss/` are governance records; changes
  to them require a maintainer-approved governance decision, not a routine PR.

## Development

```bash
cd crates/antiburn-local
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

All three must be clean before review. Behavior suites live in `tests/`;
keep inline `#[cfg(test)]` modules small and tightly scoped.

## Slop gate

The `slop gate` check runs on pull requests only. A failure also fails
`ci-required`. The gate judges the files that the pull request changes, and it
judges each touched file as a whole. It is not a repository score.
`ci-required` is a required check on `main`, so a red slop gate blocks the merge.

Run the local check before you push:

```bash
pnpm run slop
```

This command expands to `aislop ci --changes --base origin/main` and uses the
pinned `aislop 0.14.0`. CI runs the same script against the pull request base
SHA. Run `git fetch origin main` first to make the local result close to the CI
result. The results are not identical because the bases differ.

Nine promoted rules have `error` severity and fail the build. Seven rules are
threshold-independent, so any new instance fails:

- `ai-slop/empty-function`
- `ai-slop/narrative-comment`
- `ai-slop/meta-comment`
- `ai-slop/hardcoded-id`
- `ai-slop/hardcoded-url`
- `ai-slop/rust-non-test-unwrap`
- `ai-slop/todo-stub`

Two rules are threshold-bounded and fail after a size limit is crossed:

- `complexity/file-too-large`
- `complexity/function-too-long`

The size limits have three layers:

1. **Configured value:** `maxFileLoc: 1500` and `maxFunctionLoc: 1000`.
2. **Language-adjusted budget:** aislop multiplies each configured value by a
   per-extension multiplier. Rust files use ×2.5 for a 3750-line budget. Rust
   functions use ×1.5 for a 1500-line budget. `.tsx` and `.jsx` files use ×1.5.
   The finding message reports this budget.
3. **10% grace trigger:** the check fires only above a further 10% grace on the
   language-adjusted budget. A Rust file first fails at 4126 lines. A Rust
   function first fails at 1652 lines because `Math.ceil(1500 * 1.1)` is 1651
   in IEEE-754 and the test uses strict `>`.

Other languages use different multipliers. Issue #90 tightens the configured
values.

At this commit, these 11 files contain 21 promoted findings. Touching any file
makes the pull request red until all of its promoted findings are fixed or
suppressed:

- `apps/desktop/src-tauri/crates/sound/src/synth.rs`
- `apps/desktop/src-tauri/src/provider_usage/live/sources/codex_fetch.rs`
- `apps/desktop/src-tauri/src/provider_usage/live/sources/cooldown.rs`
- `apps/desktop/src/components/activity/TruncatedText.tsx`
- `apps/desktop/src/components/activity/useActivityGroupPinning.ts`
- `apps/desktop/src/lib/ipc.ts`
- `apps/desktop/src/lib/useDialogDismissal.ts`
- `apps/desktop/src/lib/useElementWidth.ts`
- `apps/desktop/src/lib/useGlobalKeydown.ts`
- `apps/desktop/src/views/overlay/OverlaySession.ts`
- `apps/desktop/src/views/popover/PopoverSession.ts`

Issue #90 clears these standing findings. Refresh the list with this command,
which selects every diagnostic where `severity === 'error'`:

```bash
pnpm exec aislop scan --format json 2>/dev/null | sed -n '/^{/,/^}/p' | node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{const j=JSON.parse(s);const m={};for(const x of j.diagnostics||[]){if(x.severity!=='error')continue;const k=x.filePath+' '+x.rule;m[k]=(m[k]||0)+1}for(const k of Object.keys(m).sort())console.log(m[k],k)})"
```

There are two ways out of a finding: fix it, or suppress it with an aislop
directive. Use this shape: `<comment marker> aislop-ignore-next-line <rule id…> -- <justification naming #90>`.
The `--` separator keeps the justification from being parsed as rule IDs.
`aislop-ignore-next-line` covers the line below the directive.
`aislop-ignore-line` covers the line that contains the directive.
`aislop-ignore-file` covers the whole file from any line in it. Rule IDs are
optional. Omitting them silences every rule on the target. Write the
justification in ASD-STE100 and name #90.

## Edit-time agent hooks

The repository commits `PostToolUse` hooks that give advisory aislop findings.
Claude Code runs one after each matched `Edit`, `Write` or `MultiEdit`. Codex
runs one after each `apply_patch`, through the committed adapter
`scripts/codex-aislop-hook.mjs`, after no other Codex edit path, and only when
you complete both trust steps below. Both hooks run the repository-pinned
`node_modules/.bin/aislop` 0.14.0. The findings are advisory, so no hook blocks
an edit.

The hook judges the whole edited file, so it can report standing #90 findings
in a file you touch. Hook runs also disable the `format` and `lint` checks, so
the hook scan is narrower than the gate. Run `pnpm run slop` before you push,
because it stays the authoritative check.

Codex needs two trust steps. Trust the folder, so Codex discovers
`.codex/hooks.json` as a project source. Then trust the hook with `/hooks`.
Codex feedback stays inert until you do both steps.

Do not run `aislop hook install` here. It deletes each hook group that has a
non-null `__aislop` sentinel, then writes an unpinned command, and that destroys
the repository pin. A global hook causes duplicate feedback. For Claude Code,
aislop generates the global hook, so run
`aislop hook uninstall --claude --global`. For Codex, aislop generates no hook,
because its Codex installer writes rules text only. A global Codex hook is
hand-authored, so remove it from your Codex configuration by hand. The Claude
uninstall command does not remove a Codex hook.

## Pull request boundaries

Prefer one pull request per independently reviewable and reversible change.
Corrections found while rehearsing one unpublished release belong in that
release-hardening pull request rather than a new pull request per correction;
split one out only when it can be released or rolled back independently. Small
documentation-only changes may remain separate because CI recognizes them and
does not compile unrelated application code.
