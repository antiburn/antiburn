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

Keep `docs/` current rather than recording delivery status. Use stable guide
filenames without suffixes such as `-v1`; document format versions inside the
contract. Do not link delivery plans or local run evidence from maintained docs.

## Solution design

- Start with the user outcome.
- Treat the current design as one option, not a fixed limit.
- Match each claim to a source that can support it.
- If a broad claim is not safe, make a narrower useful claim and state its limit.
- Consider a separate source when the current source cannot support the feature.
- Use hypotheses for investigation. Label them as hypotheses and do not present them as facts.
- Prefer the smallest complete solution. Do not add full support when scoped support solves the user problem.
- Do not use `_inner` as a helper-function suffix. Keep a helper only when it
  makes the code clearer, and name it for the work it does. Inline unnecessary
  wrappers. Preserve established external API names.

## Cloud boundaries

Read the public contracts in `docs/cloud/` before changing Cloud features.
Keep Cloud behind the explicit, non-default Rust feature `cloud`. Ordinary
release builds and release workflows must not enable it. The desktop development
launcher selects it for local testing. Use `#[cfg(debug_assertions)]` for local
debug configuration; do not add a separate build-profile flag.

Keep source, documentation, commits, and PR text limited to public client
behavior, public API contracts, and synthetic fixtures. Do not include private
server design, infrastructure, plans, links, or copied private fixtures.

Use desktop or local capture and sync names. Use native only for the provider's
original session format, source bytes, or source identity. Do not call desktop
controllers, capture jobs, IPC adapters, or upload status native. Preserve exact
provider fields and existing serialized format identifiers.

## React

Do not add `useEffect`. Derive values during render, handle work in the event
that caused it, or move synchronization to the external-system boundary.

Only add `useEffect` when no simpler design works. Explain why and get explicit
maintainer agreement first.

## Rust

Use Tokio async I/O. Run blocking filesystem, SQLite, or library work with bounded
`spawn_blocking` tasks or workers, never on UI or async runtime threads.

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

Design for the normal task first. Show actions the user can take and status
that changes what they do next. Hide empty optional sections, obsolete warnings,
and background recovery that needs no action. Do not ask users to manage
internal cleanup. Keep actions and focus stable as data loads. Keep the same
editor after Save when a saved choice can still change. See the
[Desktop design principles](apps/desktop/design.md#desktop-design-principles).
Show a switch only when it can change the setting. Use a setup action or a
short status when the setting is not ready.
Recover automatically when it is safe. Otherwise, give each actionable error a
button or link that starts the next step.

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

## Burn Checks

Use `docs/burn-check/specification.md` and the supported subset in
`docs/burn-check/authoring.md` for Burn Checks work. The exact draft label is
`v1-alpha`: experimental, not yet standardized, and open to breaking syntax,
semantic, and capability changes with no backward compatibility promise.
Keep the current draft precise. Revisit proposed remaining work against user
needs and source evidence before implementing it. When the actual contract
changes, review its impact and update the specification, schema, code, examples,
and session/check coverage together. Do not add a YAML version field or confuse
the draft label with internal serialization versions.

## Comments

Write code comments in ASD-STE100 Simplified Technical English. Use active
voice and present tense. Add a comment only for important information the code
cannot show. Keep identifiers and API names unchanged.

Add doc comments to public functions, types, traits, and modules, and to important
internal functions. Explain purpose and caller rules rather than repeating
signatures. Keep these contracts beside their definitions.
Cover frontend component inputs, state and cache ownership, backend interfaces,
and side effects when callers or maintainers depend on those rules.
For each trait, explain why it exists, which code uses it, and what implementors
must provide. State relevant error, ordering, concurrency, and data-safety rules.
Explain non-obvious state transitions, retry inputs, and side-effect ownership.
Update these contracts when behavior changes. Do not add abstractions just to
document them or comments that repeat syntax.

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
cargo nextest run --manifest-path crates/antiburn-local/Cargo.toml --test check_coverage_contract
```

When source claims or formats change, also run the relevant characterization
test and:

```bash
cargo nextest run --manifest-path crates/antiburn-local/Cargo.toml --test source_contracts
```

## Product analytics

Cover features well with analytics so we can improve the product further.

Read [docs/analytics.md](docs/analytics.md) before instrumentation work. Follow
the measurement definitions and event review contract in
[docs/analytics-measurement.md](docs/analytics-measurement.md).

## Tests and commits

Keep test files focused:

- Small Rust test blocks can stay inline when the full source file remains short and readable. Put public-API-only tests in the external `tests/` target. For private access, move larger blocks to feature-local `tests/<file>_tests.rs` and include them with `#[cfg(test)]` and `#[path = "tests/<file>_tests.rs"] mod tests;`. Every test file under `src/` needs a module declaration.
- Tests that open test database connections are integration tests, including temporary SQLite stores. Name them `tests/<file>_integration_tests.rs`. Use an external test target or a path-mounted module; do not expose production internals just for test placement.
- Keep TypeScript/React tests in colocated `*.test.ts`/`*.test.tsx` files, or a nearby `__tests__/` folder for larger feature suites. Keep fixtures and render helpers near their tests.
- Use `*.integration.test.ts(x)` for real service boundaries and the existing browser suite for E2E checks. Mocked component tests do not prove database or native-service behavior.
- Follow [testing](docs/testing.md). Apply the layout to new or substantially changed tests without unrelated mass moves.

Run the relevant formatter, linter, type checks, and tests for every change. Use
the commands in `CONTRIBUTING.md` and `apps/desktop/README.md`.

Use `cargo nextest run` for Rust test execution. Select the changed package,
test target, or test-name filter during implementation. Run the full affected
workspace suite at final integration instead of repeating it after every edit.
Use `cargo test --doc` for doctests, which nextest does not run. Run named
ignored live-model tests separately with the eval guide's `cargo test` commands
when their capture workflow requires that runner. Do not enable all ignored
tests. If nextest is unavailable, install the supported runner or report the
environment blocker before using a slower fallback.

Every commit must include a DCO sign-off. Use `git commit -s`. CI rejects a pull
request if any authored commit lacks it.
