# Contributing to antiburn

Thank you for contributing.

## Ground rules

- The project uses the [MIT License](LICENSE). Contributions use the same
  license. There is no CLA.
- Every authored commit needs a
  [Developer Certificate of Origin](https://developercertificate.org/)
  sign-off. Use `git commit -s`. CI rejects unsigned commits.
- Follow the [Code of Conduct](CODE_OF_CONDUCT.md).
- Keep pull requests focused, reviewable, and reversible.
- Follow the engineering constraints in [AGENTS.md](AGENTS.md).

## Privacy and safety

antiburn is a local application. It needs no project-operated account, server,
or backend. It can read local coding-agent data and contact a provider with the
credentials that provider issued to the user. It must not send session content,
credentials, or other user data to the project or to an unrelated third party.

The update check and anonymised application analytics are the only
project-operated network channels. Neither is required for the app to work.
Analytics must keep all properties documented in [docs/analytics.md](docs/analytics.md):
it starts automatically only after onboarding completes, Settings → Privacy
provides the opt-out, payloads contain no work or credentials, identifiers
rotate, and builds without a configured endpoint send nothing.

Take extra care with operations that modify files, stop processes, or can cost
the user money. Require a clear user action, keep credentials in memory only,
and state the cost and its bound in the pull request.

Use synthetic test fixtures. Do not commit real transcripts, user names, home
paths, repository names, credentials, or captured machine output. Redaction is
not sufficient.

antiburn is an always-running utility. Keep reads, allocations, concurrency,
retained data, CPU work, and disk I/O bounded by the visible feature's needs.

## Development checks

Run the engine checks from its standalone workspace:

```bash
cd crates/antiburn-local
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Run desktop checks from the repository root:

```bash
pnpm install
pnpm --filter @antiburn/desktop lint
pnpm --filter @antiburn/desktop type-check
pnpm --filter @antiburn/desktop test
pnpm --filter @antiburn/desktop build
```

Run shell checks from its standalone workspace:

```bash
cd apps/desktop/src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Run `pnpm run slop:all` and `pnpm run secrets` before you push. Pull-request CI
also runs `pnpm run slop` against the changed files. See
[docs/debugging.md](docs/debugging.md) for isolated desktop profiles, logs, and
developer tools.

Popover memory changes also need a local macOS report. Follow
[docs/runbooks/memory-reporting.md](docs/runbooks/memory-reporting.md). The live
report requires macOS 13+, a logged-in GUI session, Steve 0.5.1 with
Accessibility permission, and no other antiburn instance. CI runs only the pure
Node report tests.

### Optional Antigravity usage credentials

The Google installed-app client ID and secret are optional for local
development. They are only required to test refresh of Antigravity 2.0, IDE, or
`agy` live-usage credentials. Local session analysis and other providers do not
need them.

Use a current official Antigravity installation as the primary source. Inspect
the installed `language_server` or `agy` executable for the
`*.apps.googleusercontent.com` client ID and its `GOCSPX-` client secret. On
macOS, the standard IDE command is:

```bash
strings "/Applications/Antigravity.app/Contents/Resources/bin/language_server" \
  | rg 'apps\.googleusercontent\.com|GOCSPX-'
```

On Linux or Windows, locate the equivalent executable in the official IDE or
CLI installation and use the platform's printable-string tool with the same
patterns. Inspect only the application executable. Do not read or share access
tokens, refresh tokens, keychain entries, credential databases, or account
state.

Confirm the pair against a second source before use. The pinned
[`jcode` Antigravity OAuth implementation](https://github.com/1jehuang/jcode/blob/435fb4a8/crates/jcode-base/src/auth/antigravity.rs)
records the official desktop-client constants. Google also documents that
[installed applications cannot keep client credentials confidential](https://developers.google.com/identity/protocols/oauth2/native-app),
but the values must still stay out of this repository.

Put the values in `apps/desktop/.env` under the names in
`apps/desktop/.env.example`. The Tauri development scripts load this ignored
file, and explicit shell variables take precedence. Maintainers store the same
names as repository secrets for CI and as `release` environment secrets for
signed builds.

## Pull requests

Describe the user impact, tests, privacy or performance effects, and any known
limits. Do not include credentials or private session content in issues, logs,
screenshots, or pull requests. Use the confidential reporting channel in
[SECURITY.md](SECURITY.md) for security problems.
