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

## Product analytics

For each user-facing feature or behavior change, state the product question and
how analytics answers it. Cover discovery, meaningful use, and the result where
each matters. Add missing coverage in the same change, or explain why existing
events suffice or measurement is not useful. Do not add events just to count
every click.

Read [docs/analytics.md](docs/analytics.md) before instrumentation work. Follow
the measurement definitions and event review contract in
[docs/analytics-measurement.md](docs/analytics-measurement.md).

- Use the shell's analytics module and closed Rust event and interaction types.
  Keep analytics out of the local engine. Do not add generic tracking maps,
  arbitrary strings, or third-party analytics SDKs.
- Record user intent in its event handler and results at the boundary that
  knows the outcome. Record views after actual visibility. Do not add a React
  effect or count rendering, prewarming, polling, retries, or automatic restores
  as deliberate use.
- Define each event's trigger, allowed properties, owner, and duplicate or rate
  limit rule. Distinguish attempts from success and background work from use.
- Preserve opt-out, identifier rotation, inert unconfigured builds, bounded
  queues, and silent failure. Never send work content, local identifiers,
  paths, credentials, raw errors, or unreviewed setting values.
- Update the public event catalog and affected privacy disclosures in the same
  change. Changes to existing event meanings also require documentation and a
  version boundary for analysis.
- Test the trigger and outcome, duplicate suppression, rejected properties, and
  disabled behavior as relevant. Run the analytics-enabled shell checks listed
  in `CONTRIBUTING.md`; default Cargo checks omit most analytics code.

Include the analytics coverage and validation in the pull request. Documentation,
styling, and internal refactors can state that measurement is unchanged.

## Tests and commits

Run the relevant formatter, linter, type checks, and tests for every change. Use
the commands in `CONTRIBUTING.md` and `apps/desktop/README.md`.

Every commit must include a Developer Certificate of Origin sign-off. Use
`git commit -s`. CI rejects a pull request if any authored commit lacks it.
