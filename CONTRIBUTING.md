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

Local Skill Opportunities reference inputs retain bounded current skill names,
full descriptions, semantic frontmatter/metadata, optional filesystem birth
time, and selected native skill-use identity/lifecycle/timing. Current inventory
does not prove historical visibility or contents. This local adapter sends no
requests by itself. The production worker sends selected skill fields and use
limits to the configured provider; frontmatter and metadata can contain paths or other
private values. Keep these inputs and native record/call IDs out of first-party
analytics and ordinary report DTOs. Select individual reference snapshots rather
than serialize the entire inventory.

antiburn keeps its session index locally and needs no project-operated account.
When the user enables Smart Burn Checks with Jev, Ollama, Cloudflare, or Custom, the
current Ignored Instructions check sends selected instruction text, assistant
text excerpts, Bash command input, file-edit paths, read-file paths, search
queries with scope filters, and other-tool inputs to that provider. OpenCode
`apply_patch` requests expose paths from valid `patchText` input. Bash input can
include inline scripts, heredocs, and patches recorded inside the command.
Dedicated edit-tool content is excluded. Selected human text and exactly bound
Bash results can supply bounded context; unknown-origin text and completion labels
do not prove approval or passing tests. Read/search output, other results, typed
question/plan fields, and thinking remain excluded from Ignored Instructions.
Scope Creep, Over-exploring, and Skill Opportunities can send selected task/work,
edit and tool-result content, supported question/plan records, and current skill
reference fields. They reach pinned native Claude Code, Codex, OpenCode SQLite,
and Pi roots; see [session coverage](docs/session-coverage.md).
Selected paths can leave the machine in provider requests. Keep this
optional request separate from first-party product analytics. Do not send its
inputs, responses, keys, findings, or evidence to analytics. Store only bounded
local Smart Check usage aggregates, including the model and price version used for
estimates. Do not retain request histories or session identifiers for billing summaries.
Session deletion keeps already-incurred totals. Clear Local Data removes them
and the rolling usage reservations.

Instruction-file discovery compares supported files in their current state.
It does not prove historical contents or activation. Recover historical
instruction text only from an authoritative session record; do not infer it
from a matching current path or a read request. Keep unavailable evidence
unavailable. Ignored Instructions samples 256 high-priority rule/action pairs
per review by default. This sample is not exhaustive or a spending cap. Clean
means no finding among sampled comparisons, not that all content is safe. Keep
provider and evidence errors separate from the remaining sampling gap. Reuse a
result only when the same instruction rule and action can be identified across
an append or restart. Review new activity first, then older pairs not yet
sampled. Instruction changes govern future actions only; the first observed
version cannot establish historical activation. The roughly 60-second
ordinary-session goal after worker start is not a cutoff or guarantee. An
unknown outcome can trigger up to three total dispatch attempts while the worker
tries to recover it. An earlier attempt may already have incurred a charge. If
the result remains unknown, further dispatch of that work is blocked. Describe
incremental paid requests and compatible answer
reuse without promising a per-session cost cap.

Follow [the reusable Jev check contract](docs/smart-burn-checks.md#reusable-jev-check-contract) when adding a
check-owned projection, input window, question set, or reducer. See
[how Smart Burn Checks work](docs/smart-burn-checks.md) for the product flow.

The update check and anonymised application analytics are project-operated
network channels. Neither is required for the app to work.
Analytics must keep all properties documented in [docs/analytics.md](docs/analytics.md):
official configured builds can record launch and first-run progress before
setup completes, Settings → Privacy provides the opt-out, payloads contain no
work or credentials, identifiers
rotate, and builds without a configured endpoint send nothing.

For all four Smart Burn Checks, measure saved enablement and provider transitions, completed
execution outcomes, visible findings, evidence outcomes, and prompt actions
using closed values only. Exclude historical assessment outcomes from normal
adoption and automatic execution rates. Measure user-requested history runs separately.
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
cargo test
```

### Desktop checks

```bash
pnpm install
pnpm --filter @antiburn/desktop lint
pnpm --filter @antiburn/desktop type-check
pnpm --filter @antiburn/desktop test
pnpm --filter @antiburn/desktop build
```

### Smart Check diagnostics

Use the [Rust eval guide](apps/desktop/src-tauri/eval/README.md). Each check has
one ignored `live` entry point and shared provider/suite/case/limit/capture support.
Credentials come only from the authorized shell environment. Select exact cases
with `ANTIBURN_EVAL_CASES` and bound calls with `ANTIBURN_EVAL_LIMIT`. Reports
retain errors, abstentions, binding diagnostics, accuracy, usage, and latency.
Scores are diagnostics, not strict thresholds or delivery gates. No recipe,
frozen hash, previous passing report, or Python tool is required. Do not run every
ignored test; imported production transport probes can incur charges.

After coverage edits, run from the repository root:

```sh
cargo test --manifest-path crates/antiburn-local/Cargo.toml --test check_coverage_contract
```

When source claims or formats change, also run `source_contracts` and the
affected native characterization targets with the same engine manifest. Select
tests for the changed contract; live eval scores remain diagnostics, not gates.

### Desktop backend checks

The Linux remote helper is another standalone workspace. When its protocol or
engine inputs change, run `cargo fmt --check`, `cargo clippy --all-targets --locked
-- -D warnings`, and `cargo test --locked` from `crates/antiburn-remote` as well.
CI also builds its static Linux x64 and ARM64 archives. See
[remote sessions](docs/remote-sessions.md) for manual setup and evidence limits.

```bash
cd apps/desktop/src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

For analytics changes, also run these shell checks with the feature enabled:

```bash
cd apps/desktop/src-tauri
cargo clippy --all-targets --features analytics -- -D warnings
ANTIBURN_ANALYTICS_URL=http://127.0.0.1:8787 \
ANTIBURN_ANALYTICS_OPERATOR="Local development" \
cargo test --features analytics
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
