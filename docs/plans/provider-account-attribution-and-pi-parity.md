# Provider Account Attribution and Pi Parity Plan

## Goal

Attribute local usage to the billing provider and, when provider-issued identity
is available, to the same opaque account used by live limits. Complete Pi and
OpenCode support without retaining transcripts, credentials, raw account IDs,
or memory proportional to session size.

## Implementation Status

Implemented in the current worktree:

- Pi and OpenCode retain bounded explicit provider observations, including
  failed and zero-token requests.
- Desktop analysis persists those observations and reprocesses stale rows.
- Provider resolution follows the locked precedence and never duplicates tokens
  for conflicting hints.
- Live and local account groups use provider-scoped HMAC-SHA256 keys derived
  from a random per-install secret.
- OpenAI uses the ChatGPT account ID. Anthropic uses the OAuth profile UUID.
  Google uses the OAuth `sub`. Pi and OpenCode OAuth stores use the same rules.
- API-key usage stays provider-known and account-unassigned.
- Session account observations are append-only, rollout-gated, bounded, and
  retain only opaque keys plus fixed provenance and confidence values.
- The expanded Usage view joins matching local and live accounts, renders one
  unassigned group, keeps source-agent detail, and preserves first-seen order.
- Unattributed sorts last and starts collapsed. Google always appears in the
  Settings > Usage meter list.
- Google accepts partial quota summaries, merges model windows, and uses the
  bounded project-scoped compatibility endpoint when required.
- Usage source failures render as one actionable line.

Automated validation is complete for the engine, desktop shell, frontend,
design contract, quality scan, and secret scan. Native Windows and WSL behavior
is covered by platform seams; macOS cannot execute an MSVC-linked Windows
binary.

## Production Finding

The release database contained one Pi session with a ready analysis and an empty
model breakdown. Its only assistant request failed Anthropic OAuth refresh and
reported zero tokens, but the transcript still named `anthropic` and a Claude
model. The release aggregator discarded that provider fact and sent every
bring-your-own session with no billable model rows to `unknown`, which the UI
showed as Unattributed.

Pi records `provider` and OpenCode records `providerID`, but both adapters
previously discarded them. Model-family inference could therefore attribute an
OpenCode GitHub Copilot request to OpenAI or lose an OpenCode-billed model.

## Locked Decisions

1. Provider, model, and account are separate facts.
2. Provider means the party that bills the reader, not the model laboratory.
3. Fixed-route agents keep their fixed provider.
4. Pi and OpenCode use explicit provider metadata before model inference.
5. Zero-token and failed requests retain provider detection without adding spend.
6. Unattributed means the provider is unknown. A known provider without a proven
   account uses an unassigned account group.
7. Account keys come from provider-issued account subjects, never tokens.
8. OpenAI uses the ChatGPT account ID. Anthropic uses the OAuth profile account
   UUID. Google uses its OAuth subject.
9. Raw subjects exist only in memory. Persistence and IPC receive a keyed,
   provider-scoped opaque value.
10. Identity network calls use the existing live-usage opt-in.
11. Historical sessions remain unassigned when the account used at that time
    cannot be proved.

## 1. Bounded Provider Observations

- Add a compact `ProviderHint` to the engine session summary.
- Retain unique provider and model pairs only.
- Cap the collection at the existing model limit and cap each string at the
  existing evidence-name limit.
- Read Pi `provider` from assistant messages and `model_change` rows.
- Read OpenCode `providerID` from assistant messages.
- Keep hints when usage is absent or all token counts are zero.
- Carry the current Pi provider with the current model.
- Do not add a second transcript scan or use discovery payload helpers for
  attribution.

## 2. Durable Projection

- Add nullable `provider_hints_json` to `session_analysis` through an appended
  migration.
- Use `NULL` for old analysis, `[]` for completed analysis with no hints, and a
  bounded array for observed hints.
- Add the field to analysis and usage-evidence store records.
- Build it once when analysis publishes instead of parsing source summaries on
  every usage request.
- Bump the parser or analysis revision so existing Pi and OpenCode sessions are
  reprocessed.
- Preserve clear, delete, retention, and privacy behavior.

## 3. Provider Resolution

Resolve each usage row in this order:

1. Fixed agent route.
2. Explicit Pi or OpenCode provider hint for the model.
3. Model namespace.
4. Canonical model family.
5. Unknown.

Characterize aliases for Anthropic, OpenAI, Google, GitHub Copilot, OpenRouter,
OpenCode, AWS Bedrock, and Azure. An unknown explicit gateway stays unknown; it
must not fall through to the model laboratory.

If one aggregated model maps to conflicting explicit providers, retain both
provider detections but place the indivisible token total under unknown. Never
duplicate tokens.

## 4. Usage States

- `estimated`: all attributed token rows are priced.
- `observed`: attributed token rows exist but pricing is incomplete.
- `detected`: explicit provider evidence exists with no billable tokens.
- `unknown`: analysis or attribution evidence is absent or unreadable.

The production zero-token Pi regression must produce an Anthropic detected card
with one session and zero tokens, not an Unattributed card.

## 5. Canonical Opaque Accounts

- Create one shell-owned account-key service.
- Persist one random per-install secret in internal app state.
- Derive a provider-scoped HMAC-SHA256 key from the canonical provider subject.
- Persist and expose only a bounded opaque key.
- Remove source-specific and double-hashing account paths.
- Key live history by the canonical opaque key.
- Start a new live-history namespace when legacy account keys cannot be mapped
  safely.

Provider resolvers:

- OpenAI: read the account ID already stored by Codex, Pi, and OpenCode.
- Anthropic: resolve each OAuth token through `/api/oauth/profile` and use the
  account UUID. Separate OAuth grants for one account must converge.
- Google: resolve the OAuth subject for Antigravity, Pi, and OpenCode. Do not use
  an email, access token, refresh token, or managed project as canonical identity.
- API keys: name the provider but leave the account unassigned.

All credential files, keychain reads, identity responses, and decoded fields use
strict byte and field bounds. Identity calls use the existing timeout, response
cap, redirect refusal, provider-only destination, and cooldown policy.

## 6. Session Account Binding

- Persist an optional account key per session and provider.
- Bind new sessions to the active account observed during analysis.
- Do not rewrite a completed session because the active login later changes.
- Permit one session to use several providers.
- Keep ambiguous historical sessions unassigned.
- Record only bounded provenance and confidence enums, not account material.
- Observe future account changes so a session spanning a switch can remain
  honest rather than being assigned wholesale to the newest login.

## 7. Account-Grouped Presentation

- Aggregate local usage by provider and account key.
- Join local usage to live limits by the same canonical key.
- Render one explicit unassigned account group when provider identity is known
  but account identity is not.
- Do not repeat provider-wide local totals under every live account.
- Keep account numbering stable by first-seen order rather than digest order.
- Use provider labels that remain correct for bring-your-own agents.
- Keep a source-agent breakdown so Pi, OpenCode, and native-agent usage remain
  distinguishable.
- Sort any remaining Unattributed provider group after every known provider in
  the expanded usage view.
- Start the Unattributed card collapsed. Keep its summary visible and use the
  standard disclosure, card, type, spacing, radius, color, and motion utilities.
- Keep the expanded or collapsed choice local to the current renderer. Do not
  add a new persisted preference for one diagnostic card.
- Always include Google in the Settings > Usage meter roster so the reader can
  hide it even before the first successful Google response.

## 8. Pi Parity

- Bound Pi's first-line CWD read, including oversized and unterminated records.
- Preserve explicit provider metadata and zero-token detections.
- Read `PI_AGENT_DIR` and bounded `auth.json` account data.
- Resolve OpenAI, Anthropic, and Google OAuth identities under the rules above.
- Remove the unused whole-transcript provider payload reader.
- Mark Pi files as record streams in source-version metadata.
- Characterize and document native Windows and WSL behavior.
- Document Pi v3, CLI-only discovery, Insights support, fork ownership, and the
  absence of a Pi-specific live meter.
- Mark the older Pi plan as historical where its matrix no longer matches code.

## 9. Memory and Performance

- Stream every transcript through the existing bounded adapter.
- Retain at most the bounded provider-hint set per session.
- Persist the compact projection once and query it directly.
- Do not load raw turn content, source summaries, or credential payloads for the
  usage view.
- Bound provider/account maps and fold overflow into unknown or unassigned.
- Add Pi claimed-file benchmarks at 1 MiB, 10 MiB, and 50 MiB.
- Add long identity-chain, oversized-record, cancellation, and retained-memory
  probes.
- Record results in `crates/antiburn-local/benches/BASELINE.md`.

## 10. Diagnostics

- Add release-safe counts for attribution outcomes by agent, provider, and reason.
- Count known-provider/unassigned-account and genuinely unattributed sessions.
- Count identity resolver outcomes by category.
- Never log account keys, raw subjects, credentials, models, paths, session IDs,
  or source content.

## 11. Tests

- Failed zero-token Pi requests remain attributed to Anthropic.
- Pi and OpenCode OpenAI usage joins the same account as Codex.
- Pi Anthropic and Claude join through the profile UUID despite different tokens.
- Pi or OpenCode Google joins Antigravity through the Google subject.
- Two accounts at one provider remain separate.
- Account switches do not rewrite completed sessions.
- OpenCode GitHub Copilot remains GitHub.
- OpenCode-billed models remain OpenCode.
- Unknown gateways remain Unattributed.
- Unattributed sorts last and starts collapsed in the expanded usage view.
- Google is always available in Settings > Usage hide controls.
- API-key sessions remain provider-known and account-unassigned.
- Fixed routes ignore contradictory transcript hints.
- Conflicting hints never duplicate tokens.
- Raw identity sentinels do not appear in the database, logs, DTOs, or history.
- Disabling live usage prevents profile and userinfo calls.

## 12. Validation

- Run engine and desktop Rust formatting, clippy, and full tests.
- Run frontend lint, type-check, tests, production build, and design drift.
- Run provider attribution, privacy, migration, memory, and benchmark suites.
- Run `aislop scan --changes`, secret scanning, and `git diff --check`.
- Smoke-test a release build against a copy of a production database.
