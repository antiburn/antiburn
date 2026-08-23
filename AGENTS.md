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

Fix every aislop finding your own change caused before you finish the turn. Do not fix a standing finding that issue #90 owns. If you must change a legacy file and cannot fix its standing finding, suppress it with `<comment marker> aislop-ignore-next-line <rule id…> -- <justification naming #90>`. Run `pnpm run slop` before you finish; the hook scan is narrower and is not authoritative.
Do not run `aislop hook install` in this repository. Read `CONTRIBUTING.md` for the hook mechanics.

## Commits

Every commit must carry a DCO sign-off: run `git commit -s` (or add `Signed-off-by: Name <email>` to the message by hand).

The DCO check fails the whole PR on any commit that is missing one — it does not average out across the branch. Sign off from the first commit to avoid rework.
