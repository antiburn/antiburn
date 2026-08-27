# Repository instructions

These instructions apply to the entire repository.

## React

Do not add `useEffect`. Derive values during render, handle work in the event
that caused it, or move synchronization to the external-system boundary.

Only add `useEffect` when no simpler design works. Explain why and get explicit
maintainer agreement first.

## Rust

Do not suppress dead-code or deprecated-code lints. Remove dead code and replace
deprecated APIs instead.

Only add a suppression when it is strictly necessary. Explain why and get
explicit maintainer agreement first.

## Desktop design

Read `apps/desktop/design.md` before styling work in `apps/desktop`. Its YAML
front matter defines the tokens, and its listed stylesheets are the source of
truth.

Use the documented semantic utilities: `bg-/text-/border-<token>`, the `type-*`
scale, `rounded-control`, and `duration-*`. Do not hard-code colors, type sizes,
radii, or durations.

When a token or stylesheet changes, update `apps/desktop/design.md` in the same
change. Add each new stylesheet to its `sources:` list. CI checks this contract
with `scripts/check-design-drift.mjs`.

## Comments

Write code comments in ASD-STE100 Simplified Technical English:

- Use the active voice and present tense.
- Keep instructions to 20 words or fewer and descriptions to 25 words or fewer.
- Put one idea in each sentence.
- Use simple words and keep articles such as "the" and "a".
- Do not use idioms, humor, slang, or telegraphic fragments.
- Keep identifiers and API names unchanged.
- Add a comment only when it states important information the code cannot show.

## Tests and commits

Run the relevant formatter, linter, type checks, and tests for every change. Use
the commands in `CONTRIBUTING.md` and `apps/desktop/README.md`.

Every commit must include a Developer Certificate of Origin sign-off. Use
`git commit -s`. CI rejects a pull request if any authored commit lacks it.
