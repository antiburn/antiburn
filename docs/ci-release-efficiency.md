# CI and release efficiency: before and after

Status: baseline measured 2026-08-17; the "after" figures are a conservative
model until this workflow revision has enough completed GitHub Actions runs.

This report is observational documentation. No workflow reads it, it is not a
release artifact, and changing it cannot alter a required check, package,
signature, checksum, updater manifest, SBOM, draft, or published release.

## Executive comparison

| Flow                                                     |    Before |                     After model |      Expected saving |
| -------------------------------------------------------- | --------: | ------------------------------: | -------------------: |
| Review CI, mean gate time across the 15-run sample       | 13.71 min |                       10.07 min |     3.64 min (26.5%) |
| Review CI, mean raw runner time across the 15-run sample | 39.17 min |                       25.88 min |    13.29 min (33.9%) |
| Pure app-version PR gate, mean of 3 historical diffs     | 14.06 min |                        1.00 min |    13.06 min (92.9%) |
| Pure app-version PR raw runner time                      | 40.62 min |                        1.10 min |    39.52 min (97.3%) |
| Frontend-only PR gate, mean of 2 historical diffs        | 13.74 min |                        3.60 min |    10.14 min (73.8%) |
| Frontend-only PR raw runner time                         | 38.29 min |                        5.69 min |    32.60 min (85.1%) |
| App release workflow after exact `main` CI has passed    | 28.52 min | about 15 min before cache gains | about 13.5 min (47%) |
| Engine release workflow after exact `main` CI has passed | 13.55 min |                     about 1 min | about 12.5 min (93%) |

The review-CI median remains approximately 14 minutes because 7 of the 15
sampled changes fail closed to the full matrix. The improvement is concentrated
where it should be: small, isolated, and semantic version-only changes. Full
changes preserve the existing coverage and pay about 30 seconds of classifier
and aggregate-check overhead before any Rust-cache benefit is counted.

## What changed

- A semantic classifier selects frontend, engine, desktop-backend, release
  metadata, and license jobs from the actual diff. Unknown and dependency
  configuration changes fail closed to the full matrix.
- The stable `ci-required` result aggregates selected and intentionally skipped
  jobs, so branch protection needs one invariant required-check name.
- Rust jobs restore trusted, platform-specific caches. Only successful pushes
  to `main` save ordinary CI caches.
- Pure app release merges compile the unsigned release targets on `main`. The
  tag workflow restores those caches but never saves secret-bearing output.
- Tag workflows require the successful `main` push run for the exact tagged SHA
  instead of executing a second copy of the full CI matrix.
- Release draft validation now performs deterministic asset-count, signature,
  checksum, URL, and platform checks before leaving the remaining install and
  publish decisions to a person.

## Review-CI model by change class

The historical merge diffs were reclassified with
`scripts/classify-ci-changes.mjs`. Existing job durations were retained for
every selected job; skipped jobs contribute zero. The model adds 21 seconds for
classification, 9 seconds for `ci-required`, and 30 seconds for release
metadata. It does **not** credit the new Rust caches, so Rust-heavy projections
are intentionally conservative.

| Reclassified historical path | Runs | Gate before | Gate after | Runner before | Runner after | Interpretation                                               |
| ---------------------------- | ---: | ----------: | ---------: | ------------: | -----------: | ------------------------------------------------------------ |
| Pure app version             |    3 |   14.06 min |   1.00 min |     40.62 min |     1.10 min | Metadata and boundary only on the PR; warming is `main`-only |
| Frontend only                |    2 |   13.74 min |   3.60 min |     38.29 min |     5.69 min | Rust matrices and license scan are skipped                   |
| Frontend + desktop backend   |    2 |   14.13 min |  14.63 min |     39.11 min |    32.14 min | Windows backend remains critical; engine jobs are skipped    |
| Engine + desktop backend     |    1 |   14.02 min |  14.50 min |     38.75 min |    33.68 min | Frontend jobs are skipped; cache gains are not modeled       |
| Full/fail-closed             |    7 |   13.38 min |  13.88 min |     38.87 min |    39.37 min | Coverage is unchanged; the fixed routing overhead is visible |

No sampled merge was documentation-only. Historical engine releases included
an additional runbook or dependency-resolution change, so the classifier
correctly treated them as full changes; no engine-only fast-path saving is
claimed from this sample.

Across all 15 runs, the model reduces aggregate gate time from 205.58 to 151.05
minutes and raw runner time from 587.50 to 388.23 minutes. That is 54.53 minutes
less reviewer waiting and 199.27 fewer raw runner-minutes for the same historical
change mix.

## Release workflow comparison

The last three successful app release workflows had these medians:

- 28.52 minutes wall time and 76.30 raw runner-minutes in total.
- 14.28 minutes and 41.35 raw runner-minutes were the duplicated `Checks`
  matrix.
- The four signed/package builds took 14.43 minutes on the critical path and
  33.97 raw runner-minutes.

Trusting the exact successful `main` SHA removes the 41.35 duplicated
runner-minutes on every app release. If the tag is pushed after `main` CI is
green, the uncached model is about 15 minutes to a draft, or roughly 47% faster.
The warmed release cache should improve that further, but this report assigns
it no numeric saving until real post-change runs exist.

The last three successful engine release workflows had medians of 13.55 minutes
wall time and 38.68 raw runner-minutes. Their duplicated checks accounted for
12.92 minutes and 38.15 raw runner-minutes. Once exact-SHA `main` CI is green,
verification, trust lookup, packaging, and draft creation are expected to take
about one minute, removing roughly 93% of release-workflow latency and 98% of
its runner use.

When a tag is pushed immediately with its merge, the release workflow waits for
that SHA's `main` run. In that case the wall-clock result includes the remaining
`main` time, but the duplicate matrix is still eliminated. Waiting is not a
coverage reduction: the accepted run must be a completed, successful `push` to
`main` with the exact tag SHA.

## Baseline and method

The review baseline is the 15 latest successful `main` CI runs from
[run 31779916222](https://github.com/antiburn/antiburn/actions/runs/31779916222)
through
[run 31995021874](https://github.com/antiburn/antiburn/actions/runs/31995021874).
It covers merges from 2026-08-14 07:26 UTC through 2026-08-17 04:50 UTC.

The app-release baseline uses:

- [0.1.0-rc.2](https://github.com/antiburn/antiburn/actions/runs/31779938951)
- [0.1.0-rc.3](https://github.com/antiburn/antiburn/actions/runs/31854715252)
- [0.1.0-rc.4](https://github.com/antiburn/antiburn/actions/runs/31995042417)

The engine-release baseline uses the latest three successful runs:

- [run 31742985954](https://github.com/antiburn/antiburn/actions/runs/31742985954)
- [0.1.1](https://github.com/antiburn/antiburn/actions/runs/31770357330)
- [0.1.2](https://github.com/antiburn/antiburn/actions/runs/31992990461)

Gate time is measured from the first successful job start through the last
successful job completion, excluding queue time. Raw runner time is the sum of
successful job durations; it intentionally ignores GitHub billing rounding and
operating-system multipliers. Skipped jobs contribute zero. DCO is absent from
the `main` sample and is unchanged by this design.

## Output and safety invariants

Efficiency work must not change the release contract:

- Application drafts still contain the same installers, updater archives,
  detached signatures, SBOMs, checksums, and `latest.json` entries.
- Engine drafts still contain the deterministic source archive, inventory,
  checksums, notes, and eligible provenance bundle.
- Release workflows still create drafts only. Installation checks, release-note
  review, update testing, and Publish remain human decisions.
- Cache-warming jobs receive no signing or release-write secrets. Release jobs
  restore caches read-only after secrets become available.
- A missing, ambiguous, failed, non-`main`, non-`push`, or wrong-SHA CI result
  blocks the release.
- The efficiency report is neither a CI input nor an uploaded artifact.

## Ratification measurements

After five completed runs in each available change class, replace projections
with observed medians and record cache hit status. Ratify the change when:

- frontend-only and pure release PRs reduce gate time by at least 50%;
- the representative aggregate reduces raw runner use by at least 20%;
- full/fail-closed changes regress by no more than 10%;
- release workflows execute no duplicate full matrix;
- app asset names, counts, updater keys, signatures, and checksums match the
  pre-change contract; and
- no release draft can proceed without an exact successful `main` push run.

If those thresholds are not met, keep the exact-SHA safety gate and revisit the
classifier granularity or cache keys rather than weakening required outputs.
