# Agent instructions

These instructions apply to the entire repository.

## React useEffect

There should be no `useEffect`. Prefer deriving values during render, handling work in the event that caused it, or moving synchronization to the external-system boundary.

Only add a `useEffect` when it is _strictly_ necessary because no simpler design works. Before adding it, explain why and get explicit agreement from the dev.

## Rust dead and deprecated code

There should be no Rust lint suppressions for dead or deprecated code.

Only add a suppression when it is _strictly_ necessary. Before adding it, explain why and get explicit agreement from the dev.

## Desktop design system

Read `apps/desktop/design.md` before you do styling work in `apps/desktop`. Its YAML front matter is the token reference, and the stylesheets it names are the source of truth.

Use the semantic utilities the contract documents (`bg-/text-/border-<token>`, the `type-*` scale, `rounded-control`, `duration-*`). Do not hard-code a color, a type size, a radius, or a duration.

CI runs `scripts/check-design-drift.mjs`. When you change a token or a stylesheet, update `design.md` in the same change, and add a new stylesheet to its `sources:` list.

For a review of a surface against the wider interface rules, use the `design-review` skill.

## Comments

Write all code comments in ASD-STE100 (Simplified Technical English). The rules that matter most for comments:

- Use the active voice and the present tense. Write "The scanner reads the file", not "The file is read".
- Write short sentences. Keep instructions to 20 words or fewer, and descriptions to 25 words or fewer. Put one idea in each sentence.
- Use simple approved words: "use" not "utilize", "start" not "initiate", "do" not "perform". Use each word with one meaning only.
- Keep articles ("the", "a") — do not write telegraphic fragments.
- Do not use idioms, humor, or slang.
- Keep identifiers, API names, and other technical names exactly as they appear in the code.

The main reason for a comment:

- it states something important that the code cannot show.

Reasons a comment shouldn't exist:

- if it restates the code.
- if it's out of date.
- if it's code commented out for later.

Machine-read directives (`eslint-disable`, `@ts-expect-error`, `#[allow(...)]`, etc.) are not prose; keep them, but write their explanation text in STE.

## Agent slop feedback

Claude Code gives advisory aislop findings after `Edit`, `Write`, and `MultiEdit` operations. Codex gives advisory aislop findings only after `apply_patch` operations. The Codex hook does not cover other edit paths. Both hooks run the repository-pinned `node_modules/.bin/aislop` version 0.14.0. The Codex hook uses `scripts/codex-aislop-hook.mjs`.

The hook scan is narrower than `pnpm run slop`. Hook runs disable the `format` and `lint` checks. Run `pnpm run slop` as the authoritative check before you open a pull request.

Codex must trust the folder before it discovers `.codex/hooks.json` as a project source. You must then trust the hook with `/hooks` before it runs. Codex feedback stays inert until you complete both steps.

The hook judges the whole edited file. It can report standing findings that issue #90 owns. Do not fix these findings unless your change caused them. If you must change a legacy file and cannot fix a standing finding, follow `CONTRIBUTING.md`. Use `<comment marker> aislop-ignore-next-line <rule id…> -- <justification naming #90>`.

Do not run `aislop hook install` in this repository. It deletes each group that has a non-null `__aislop` sentinel. It then writes an unpinned command and destroys the repository pin.

A global hook can cause duplicate feedback. For Claude Code, aislop generates the global hook. Run `aislop hook uninstall --claude --global` to remove it. For Codex, aislop generates no hook because its Codex installer writes rules text only. A global Codex hook is hand-authored, so remove it from your Codex configuration by hand. The Claude uninstall command does not remove a Codex hook.

## Commits

Every commit must carry a DCO sign-off: run `git commit -s` (or add `Signed-off-by: Name <email>` to the message by hand).

The DCO check fails the whole PR on any commit that is missing one — it does not average out across the branch. Sign off from the first commit to avoid rework.
