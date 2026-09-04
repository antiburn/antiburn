# Aggregated Checks

## Goal

Summarize the local detector results from recent sessions in the Activity header and its anchored companion. Keep the result compact, honest about incomplete evidence, and focused on token burn.

## Data Contract

- Reuse the existing 30-day `EfficiencyReport` and its nine production detectors.
- Record one finding, clean, unavailable, or not-applicable outcome for every detector and ready session.
- Define assessed sessions for one detector as its finding sessions plus its clean sessions.
- Return only the category counts used by All checks through a bounded Checks payload.
- Compute token estimates from the same ready, current, native 30-day cohort as the detector report.
- Keep every counterfactual token value explicitly estimated. Keep observed counts unmarked.
- Keep all processing local. Do not add a network request or read source transcripts again.

## Token Burn Estimate

Token burn is the estimated percentage of used tokens that went to avoidable work. It is estimated wasted tokens divided by total used tokens.

For each ready session, add input, output, cache-read, and cache-creation tokens from valid assistant turns with model attribution. This raw turn total is the preferred denominator. Use attributed model totals from complete or partial session evidence when raw rows do not provide a denominator.

Use three estimate tiers in order: an observed counterfactual, a documented assumption, and a conservative positive floor of 1 basis point. Every detector with findings publishes a positive numeric estimate. A clean detector can publish zero.

Session overdepth counts cache-read and cache-write tokens above the context cap. Cache churn counts observed paid context beyond positive growth, including values present in partial evidence. Unused source checks count each unused definition's tokens once per compatible main turn. One measured session is sufficient. Any observed invocation in the window suppresses that source estimate. Scope project and unknown-origin skills by agent and working directory. Publish no exact skill estimate when an unknown or project origin has no working directory.

For model overthinking, first use a positive output difference from a lower-effort turn on the same model and scope with similar context. Otherwise estimate 20% of affected `xhigh` output or 35% of affected `max` and `ultra` output. Use 10% of affected output when those values are unavailable.

For overpowered subagents, compare each delegated premium turn's token-class cost with the family replacement: `gpt-5.6-luna` for OpenAI, `claude-sonnet-5` for Claude, and `gemini-3.8-flash` for Google. For old model usage, compare post-release turns with the registry replacement. For delegated fast mode, compare the fast model price with the standard model price. Convert a positive cost saving to token-equivalent affected tokens. Use 10% of affected tokens when prices are absent or do not save cost.

For one detector, divide its avoidable tokens by all cohort tokens. For the aggregate estimate, first add the mutually exclusive unused-source types per session. Then take the per-session maximum across that source sum, overdepth, cache churn, model overthinking, overpowered subagents, old model usage, and fast mode. Do not add potentially overlapping model or mechanism estimates.

Use checked integer arithmetic and publish percentages as basis points bounded to `0..=10000`. Round estimates to the nearest basis point. Floor displayed token burn to a whole percentage, but show a positive value below 1% as `<1%`. Use green for zero, yellow below 5%, and red from 5%. Explain token burn with one short sentence inside the companion. Do not calculate a USD or provider weekly-limit estimate for Aggregated checks. Other usage features keep their existing weekly-limit percentages.

## Status Semantics

- A finding can be proven from partial evidence. A clean session result requires complete detector evidence.
- Keep the strict report-level `clean` status for a detector whose entire eligible cohort is clean.
- Show every detector with a confirmed finding under Failed checks. Include its passed and unavailable session counts.
- Show a detector under Passed checks when it has no findings and at least one confirmed clean session. State any remaining evidence gap.
- Do not render detectors without a confirmed finding or clean session.
- On session cards and Session detail, calculate the denominator as `finding + clean` only.
- Do not show not-assessed counts or rows. Show the initial computing state, but keep background refreshes silent.
- If a settled session has no assessed checks, omit the check result instead of showing `0/0`.

## UI

- Put Usage and All checks inside one folding Activity header.
- Conceal the passive preview when Activity scrolling starts.
- Give each boundary one `border-separator` owner.
- Use `N% token burn` as the compact estimate wording.
- Use `Last 30 days` and remove every mock-data label.
- Use a red status icon on a neutral card when findings exist. Use green icon tiles for passed checks.
- Give each detector a familiar Lucide icon in the anchored window.
- Show each failing detector's estimated token percentage.
- Show every passed detector at the bottom with green icon tiles. Do not truncate wins.
- Use red for token burn above 15%. Use yellow for every other token burn estimate.
- Keep pointer hover and anchor focus previews passive and free of controls.
- Open one passive companion when the reader hovers or focuses All checks.
- Do not show an Unavailable checks section.
- Do not add a fourth popover surface or navigate away from Activity.

## Resource Limits

- Share the existing deduplicated Insights reduction.
- Keep the IPC category list bounded to the nine detector IDs.
- Load the report once per mounted popover session and refresh it after relevant scan completion.
- Calculate estimates during the existing linear report reduction.
- Do not retain transcript content or unbounded turn collections in React.

## Verification

- Test report serialization and privacy boundaries.
- Test all nine numeric estimates, denominator fallbacks, measured counterfactuals, source gates, assumption fallbacks, and overlap arithmetic.
- Test session-card and Session detail assessed-only denominators and zero-assessed behavior.
- Test folding, preview concealment, hover, focus, all failed rows, and all passed rows.
- Review light and dark rendering against `apps/desktop/design.md`.
- Run frontend format, lint, type checks, tests, and build.
- Run Rust formatting, Clippy, and tests in both Rust workspaces.
- Run design drift, AI-slop, secret, diff, and macOS popover memory checks.
