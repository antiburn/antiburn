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

antiburn keeps its session index locally and needs no project-operated account.
**Run checks** under Settings → Checks → Model-based checks controls YAML check
activation. Turning it off does not stop local definition discovery. The app can
read and parse bounded YAML files and watch their directories, including at cold
startup, to show names, invalid-file diagnostics, paused check cards, and file
counts. That discovery does not restore provider configuration, discover model
capabilities, catch up session sources, or run local/model assessments. Keep the
existing limits of 256 KiB per definition and 128 files per folder, plus
directory-access gates.
The provider controls under **Model-based checks** select the active System One connection: Jev, compatible
Ollama, Cloudflare, or Custom. Settings owns routing; decision-level YAML `model`
is an unknown field, not a migration input. Observed coding-agent model fields
remain evidence, not routing controls. Definitions select available public
messages, tool calls and results, response-cycle or session facts, current
instruction text, and declared skill descriptions under
the [supported subset](docs/burn-check/authoring.md#supported-subset). Selected
arguments, results, messages, inventories, and paths can leave the machine in
provider requests and can contain private data. Do not upload full transcripts
or private thinking. Collect declared descriptions, not full skill bodies, for
skill inventory inputs. Selected tool results can still contain file text. Keep provider inputs, responses,
credentials, findings, evidence, and
native record/call IDs out of first-party analytics. Store only bounded local
usage aggregates, including the actual provider, model, and price version used
for estimates. Keep unpriced usage distinct in stored accounting; Settings omits
absent estimates rather than showing zero. A matching model name does not
make a Custom or Cloudflare request a TypeSafe-priced request.
Do not retain request histories or session identifiers for billing summaries.
Session deletion keeps already-incurred totals. **Clear index…** in Settings →
Privacy removes them and local usage records.

Terminal result state does not prove complete content or history. Preserve usable
partial evidence with explicit quality fields. Unsafe identity, scope, order, and
bindings remain unavailable. Missing required expression values can block one
candidate without blocking later results. User-slot text can come from an API or
extension; it does not grant human approval. Exclude explicitly synthetic
OpenCode parts from request history, preserve real text in mixed messages, and
mark generated-only replacements partial. Retain exact
scope, generation, evidence, and frozen-prefix bindings through acceptance.
Unknown delivery does not permit an automatic resend. Earlier attempts may have
incurred charges. Explicit status success covers assessed work only, not repair,
whole-session cleanliness, verification, or savings. A probability threshold is
author policy, not an accuracy guarantee.

Validate YAML language support separately from selected-connection capabilities.
Check criterion count, label-probability support, and confidence support without
substituting one for another. A provider feature does not enable unsupported YAML
syntax. Numeric limits cannot bypass failed discovery or establish output support.
See the [connection contract](docs/burn-check/authoring.md#decision-connections)
for setup and provider limits.

Keep shared source capability separate from product reachability in coverage claims.
Current instruction files cannot prove historical activation, and current skill
inventory cannot prove historical visibility or whole-session non-use. See
[session coverage](docs/session-coverage.md) for pinned source shapes and
[check coverage](docs/check-coverage.md) for reachable behavior.

Use [Burn Checks authoring](docs/burn-check/authoring.md) for YAML changes and
[the architecture](docs/burn-check/architecture.md) for ownership boundaries.

The update check and anonymised application analytics are project-operated
network channels. Neither is required for the app to work.
Analytics must keep all properties documented in [docs/analytics.md](docs/analytics.md):
official configured builds can record launch and first-run progress before
setup completes, Settings → Privacy provides the opt-out, payloads contain no
work or credentials, identifiers
rotate, and builds without a configured endpoint send nothing.

Model-based check analytics record saved master enablement and provider
setup/test outcomes with closed values. They do not classify authored YAML checks
or measure findings, assessment outcomes, or historical runs. Generic prompt
actions carry no check label. Keep these operations separate from engagement.
See the [measurement contract](docs/analytics-measurement.md).

Take extra care with operations that modify files, stop processes, or can cost
the user money. Require a clear user action, keep provider credentials in native
credential storage or memory rather than the app database, and state the cost
and its bound in the pull request.

Use synthetic test fixtures. Do not commit real transcripts, user names, home
paths, repository names, credentials, or captured machine output. Redaction is
not sufficient.

antiburn is an always-running utility. Keep reads, allocations, concurrency,
retained data, CPU work, and disk I/O bounded by the visible feature's needs.

## Development checks

Always run the formatter and focused tests for the changed code. Run lint, type
checks, and builds when the changed workspace has them. Run the full workspace
checks when a change crosses workspace boundaries or changes a public interface.

### Engine checks

```bash
cd crates/antiburn-local
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo nextest run
```

### Desktop checks

```bash
pnpm install
pnpm --filter @antiburn/desktop lint
pnpm --filter @antiburn/desktop type-check
pnpm --filter @antiburn/desktop test
pnpm --filter @antiburn/desktop build
```

### Provider and Burn Check diagnostics

Use the [Rust eval guide](apps/desktop/src-tauri/eval/README.md) for shared
provider protocol validation and isolated YAML replay. Live diagnostics require
explicit authorization and credentials from the shell environment. Select exact
cases and bound requests. Captures can contain synthetic inputs, answers, usage,
errors, and latency. A protocol pass does not prove check quality or native source
coverage. Do not run every ignored test; production transport probes can incur
charges.

After coverage edits, run from the repository root:

```sh
cargo nextest run --manifest-path crates/antiburn-local/Cargo.toml --test check_coverage_contract
cargo nextest run --manifest-path crates/antiburn-local/Cargo.toml --test source_contracts
```

When source claims or formats change, also run `source_contracts` and the
affected native characterization targets with the same engine manifest. Select
tests for the changed contract; live eval scores remain diagnostics, not gates.

### Desktop backend checks

The Linux remote helper is another standalone workspace. When its protocol or
engine inputs change, run `cargo fmt --check`, `cargo clippy --all-targets --locked
-- -D warnings`, and `cargo nextest run --locked` from `crates/antiburn-remote` as well.
CI also builds its static Linux x64 and ARM64 archives. See
[remote sessions](docs/remote-sessions.md) for manual setup and evidence limits.

```bash
cd apps/desktop/src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo nextest run
```

For analytics changes, also run these shell checks with the feature enabled:

```bash
cd apps/desktop/src-tauri
cargo clippy --all-targets --features analytics -- -D warnings
ANTIBURN_ANALYTICS_URL=http://127.0.0.1:8787 \
ANTIBURN_ANALYTICS_OPERATOR="Local development" \
cargo nextest run --features analytics
```

The endpoint and operator are build-time inputs. Use the loopback collector in
[docs/analytics.md](docs/analytics.md#verifying-this-yourself) for manual delivery
checks. Never use the production collector for synthetic test events.

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

For user-facing changes, describe the product question, existing or new analytics
coverage, and the validation. Explain any intentional measurement gap. Follow
the [event review contract](docs/analytics-measurement.md#event-review-contract)
and update the [public catalog](docs/analytics.md) when instrumentation changes.

Describe the user impact, tests, privacy or performance effects, and any known
limits. Do not include credentials or private session content in issues, logs,
screenshots, or pull requests. Use the confidential reporting channel in
[SECURITY.md](SECURITY.md) for security problems.
