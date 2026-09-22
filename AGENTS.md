# Repository instructions

These instructions apply to the entire repository.

## Local working artifacts

Plans, handoffs, and review reports are local working artifacts. Never commit
them to this public repository. Store plans and handoffs under
`.agent-artifacts/plans/` and review reports under `.agent-artifacts/reviews/`.
Keep review captures alongside their reports. These directories are ignored.
Do not create these artifacts elsewhere in the repository or force-add them.

Keep maintained product and contributor documentation in the repository,
including design-system rules, coverage documents, runbooks, and reusable
agent skills.

## Solution design

- Start with the user outcome.
- Treat the current design as one option, not a fixed limit.
- Match each claim to a source that can support it.
- If a broad claim is not safe, make a narrower useful claim and state its limit.
- Consider a separate source when the current source cannot support the feature.
- Use hypotheses for investigation. Label them as hypotheses and do not present them as facts.
- Prefer the smallest complete solution. Do not add full support when scoped support solves the user problem.

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

Read `apps/desktop/design.md` before styling work in `apps/desktop`. It explains
design intent and shared rules. CSS owns exact token values; shared components
own control appearance and behavior.

Use semantic utilities: `bg-/text-/border-<token>`, the `type-*` scale, named
radii, and `duration-[var(--duration-*)]`. Reuse shared primitives. Keep justified
one-off visualization or native geometry values with their owner and explain
non-obvious constraints there.

Update `apps/desktop/design.md` when a shared design rule or rationale changes.
Do not copy token values, stylesheet inventories, or feature layouts into it.
`scripts/check-design-drift.mjs` discovers CSS and checks theme completeness,
System/explicit palette agreement, shared type leading, and native/CSS corners.

## Desktop navigation

Declare main-window views in the shared navigation registry. Keep Settings panes,
Settings controls, session filters, and checks in their feature-owned metadata.
Derive sidebar labels and search destinations from these definitions. Exclude
fixed session filters from search; agent filters remain searchable. Keep
renderers, counts, and side effects outside metadata, and native routes explicit.
Use the Settings adapters for searchable controls; generic UI primitives must not
depend on Settings metadata. Test rendered destinations against search, including
platform labels, availability, and focus without value changes.
See [main-window navigation](docs/main-window-navigation.md) for the boundaries.

## Comments

Write code comments in ASD-STE100 Simplified Technical English. Use active
voice and present tense. Add a comment only for important information the code
cannot show. Keep identifiers and API names unchanged.

## Session and check coverage

Keep `docs/session-coverage.md` and `docs/check-coverage.md` current.

Update `docs/session-coverage.md` when agent discovery, source formats,
framing, parsing, companion inputs, provider routes, or supported versions
change.

Update `docs/check-coverage.md` when parsed evidence, agent or provider support,
check requirements, or finding and clean-result eligibility change.

Update both documents when a session parsing change affects check coverage.

Keep coverage documents as current baselines, not phase plans. Apply these
rules:

- List every `SourceFormat` exactly once, with its exact enum name, in each
  source inventory, source matrix, coverage matrix, and source-format
  remediation matrix.
- List every first-tier agent/check pair exactly once. Keep `Finding`, `Prompt`,
  `Auto Fix`, `Verification`, and `Estimate` as separate typed columns.
- Mark support only for behavior that the product can reach. Prompt support
  needs both a production recommendation gate and a reachable finding.
- Derive Auto Fix from production editor policy and exact target binding. Use
  conditional status when only some reachable findings bind safely, and keep
  prompt-only behavior separate.
- Mark verification only when accepted evidence can prove the transition and
  the desktop can enroll and resolve the watch. An engine verifier alone is not
  product support.
- Mark burn-estimate support only when a reachable finding has a typed estimate
  or bounded fallback path.
- Put source, version, evidence, and clean-result limits beside the claim they
  constrain. A pinned schema, header, or producer commit with synthetic
  fixtures can define an accepted shape when a release range is unavailable.
  Do not claim all historical versions.
- Add or update second-tier parser characterization fixtures and behavior tests
  when a second-tier claim or format changes. Contract-table tests and synthetic
  capability gates do not establish parser support.

After any coverage edit, run:

```bash
cargo test --manifest-path crates/antiburn-local/Cargo.toml --test check_coverage_contract
```

When source claims or formats change, also run the relevant characterization
test and:

```bash
cargo test --manifest-path crates/antiburn-local/Cargo.toml --test source_contracts
```

## Product analytics

Cover features well with analytics so we can improve the product further.

Read [docs/analytics.md](docs/analytics.md) before instrumentation work. Follow
the measurement definitions and event review contract in
[docs/analytics-measurement.md](docs/analytics-measurement.md).

## Tests and commits

Run the relevant formatter, linter, type checks, and tests for every change. Use
the commands in `CONTRIBUTING.md` and `apps/desktop/README.md`.

Every commit must include a DCO sign-off. Use `git commit -s`. CI rejects a pull
request if any authored commit lacks it.
