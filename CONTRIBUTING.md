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

antiburn is local-first by contract, and CI enforces it mechanically:

- `antiburn-local` performs **no network or socket I/O** and gains no
  network-capable dependencies.
- Discovery reads **documented files, read-only databases, and bounded WSL
  paths only** — no process probing, port scanning, credential/token access,
  loopback HTTP, or provider IPC.
- One opt-in setting, default off, **runs a coding agent** so that the agent
  refreshes its own usage file, which antiburn then reads. That agent goes
  online; antiburn opens no connection of its own, reads no credential, and
  parses nothing the agent prints. Any future source of the same shape must
  keep all four of those properties, and must be recorded in
  `docs/deviations.md` before it ships. Reading a credential to talk to a
  provider directly remains prohibited by the rule above.
- Test fixtures must be **synthetic**: no real transcripts, usernames, home
  paths, repository names, or captured machine output — redaction is not
  sufficient.
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
