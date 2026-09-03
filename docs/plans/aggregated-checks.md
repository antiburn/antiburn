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

Use a versioned cohort policy. For each ready session, add input, output, cache-read, and cache-creation tokens. Prefer this token-weighted result when complete model-token evidence covers every ready session and has no unattributed turns.

Version 1 uses these avoidable-token shares for a session where the detector finds a problem: session overdepth 10%, model overthinking 25%, overpowered subagents 35%, unused MCP servers 5%, unused built-in tools 5%, unused skills 3%, old model usage 20%, fast mode overuse 20%, and cache churn 25%. These shares are counterfactual policy assumptions, not observed token attribution.

For one detector, multiply each affected session's token total by that detector's share. Add those values and divide by all cohort tokens to get the detector percentage.

For the aggregate estimate, combine the finding shares inside each session with `1 - product(1 - share)`. Cap each session at 50%, weight by that session's tokens, then divide by all cohort tokens. This prevents overlapping checks from adding directly.

When complete cohort token totals are unavailable, estimate each detector from `finding sessions / eligible sessions * detector share`. Combine those detector percentages with the same complement-product formula and 50% cap. This fallback keeps an estimate beside each finding without presenting incomplete token totals as observed data.

Use checked integer arithmetic and publish percentages as basis points bounded to `0..=5000`. Round displayed estimates up to a whole percentage and show `~`. Explain token burn with one short sentence inside the companion. Do not calculate a USD or provider weekly-limit estimate for Aggregated checks. Other usage features keep their existing weekly-limit percentages.

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
- Use `~N% token burn` as the compact estimate wording.
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
- Test token totals, incomplete coverage, fixed shares, overlap arithmetic, and the 50% cap.
- Test session-card and Session detail assessed-only denominators and zero-assessed behavior.
- Test folding, preview concealment, hover, focus, all failed rows, and all passed rows.
- Review light and dark rendering against `apps/desktop/design.md`.
- Run frontend format, lint, type checks, tests, and build.
- Run Rust formatting, Clippy, and tests in both Rust workspaces.
- Run design drift, AI-slop, secret, diff, and macOS popover memory checks.
