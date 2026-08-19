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
  and the anonymised usage-analytics channel recorded as D-026 and deviations
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

## Pull request boundaries

Prefer one pull request per independently reviewable and reversible change.
Corrections found while rehearsing one unpublished release belong in that
release-hardening pull request rather than a new pull request per correction;
split one out only when it can be released or rolled back independently. Small
documentation-only changes may remain separate because CI recognizes them and
does not compile unrelated application code.
