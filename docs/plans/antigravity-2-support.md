# Antigravity 2 Support Plan

## Status

The implementation is present in the current worktree. The required engine,
shell, frontend, design, quality, and secret checks are complete. Native Windows
execution remains unavailable from the macOS development host. The native
SQLite benchmark harness compiles, but its measurements are not recorded yet.

## Goal

Add complete support for Antigravity 2.0, the `agy` CLI, and Antigravity IDE.
Track local session usage and provider-reported subscription usage without
unbounded reads or large retained buffers.

Provider and account attribution now continues in
[`provider-account-attribution-and-pi-parity.md`](provider-account-attribution-and-pi-parity.md).
That plan owns canonical Google account identity shared with Pi and OpenCode.

## Original Problems

These problems describe the baseline that this plan addressed. They are not the
current implementation status.

- File-backed Antigravity sessions failed the claimed analysis path because the
  adapter did not implement `visit_claimed`.
- Brain discovery copied the complete transcript into an inline string to add
  metadata. This used memory proportional to the transcript size.
- The adapter did not report model identity, so parsed token usage did not reach
  the Google provider totals or pricing table.
- Antigravity had no live usage source. The app could not show the plan, quota
  windows, reset times, or AI credit balance.
- The provider benchmark did not include Antigravity and always constructed
  evidence with Claude capabilities.

## Implementation

### 1. Stream Session Analysis

- Implement `visit` and `visit_claimed` for `AntigravityAdapter`.
- Process brain JSONL with `BoundedJsonlReader` and emit one record at a time.
- Process nested cascade steps with a streaming Serde visitor. Do not retain the
  complete steps array.
- Prefer native `conversations/<uuid>.db` sessions when the database and brain
  transcript describe the same session.
- Read `steps.metadata` and `gen_metadata.data` from one SQLite snapshot. Stream
  one row at a time and reject a blob larger than 1 MiB before loading it.
- Keep the brain transcript as the activity and tool source. Use the database
  for token usage, retries, model identity, and generation timestamps.
- Preserve source claims, cancellation, partial-source evidence, and oversized
  record handling.
- Keep file cache-write capability unset. Enable it for native databases because
  descriptor field `ModelUsageStats.cache_write_tokens` is present there.

### 2. Bound Discovery Reads

- Keep brain transcripts as file sources instead of synthesizing a complete
  inline payload.
- Derive the session ID from the brain UUID path.
- Read only bounded transcript records for the title, start time, and workspace
  hints.
- Stream `antigravity-cli/history.jsonl` under a record and total-entry bound.
- Support the `antigravity-cli`, `antigravity-ide`, legacy `antigravity`, and
  `GEMINI_HOME` roots.
- Keep `agy` and the IDE mapped to the stable `AgentKind::Antigravity` identity.
- Use the database filename stem as the session ID. Fall back to database-only
  metadata when the optional brain transcript is absent or delayed.

### 3. Complete Local Usage Evidence

- Parse the descriptor-backed `ModelUsageStats` subset from protobuf wire data:
  input `#2`, output total `#3`, cache write/read `#4/#5`, output split
  `#9/#10`, and invocation IDs `#11/#12/#7`.
- Treat `#1` as the numeric model enum. Never add it to input tokens.
- Scan top-level length-delimited wrapper fields for `ChatModelMetadata` instead
  of depending on one wrapper field number.
- Read primary and retry usage from generation and step metadata. Deduplicate by
  the bounded invocation identity set before metrics receive an event.
- Require the output total to equal the output split when both forms are present.
  Reject inconsistent rows instead of publishing plausible incorrect usage.
- Reject binary model strings even when their bytes are valid UTF-8.
- Add model aliases only for observed Antigravity 2 model identifiers.
- Preserve unknown models as observed and unpriced.
- Detect explicit quota failures without classifying generic tool failures as
  quota incidents.
- Keep subagent usage from being counted as independent top-level usage.

### 4. Add Subscription Usage

Use a cloud-first source with a local process fallback.

- Read the existing Antigravity or `agy` OAuth credential under a strict size
  limit.
- Resolve the plan, tier, account, and managed project with `loadCodeAssist`.
- Read the native shared pools with `retrieveUserQuotaSummary`:
  - Gemini five-hour quota.
  - Gemini weekly quota.
  - Claude and GPT five-hour quota.
  - Claude and GPT weekly quota.
- Use project-scoped `fetchAvailableModels` only as a compatibility fallback.
- Accept partial shared summaries. Current Google responses can report only the
  weekly pools while model quota remains available through other responses.
- Merge shared and model-scoped windows by stable ID. Never replace one valid
  set with another.
- Read AI credits as a separate overage balance.
- When the cloud path is unavailable, detect running `agy` and current
  Antigravity IDE language servers and call loopback `GetUserStatus`.
- Keep refreshed tokens in memory. Do not modify provider-owned files.
- Perform no credential, process, or network work when live usage or the Google
  meter is disabled.

### 5. Bound Every Probe

- Keep the shared 15-second network timeout, redirect refusal, and 512 KiB
  response cap.
- Bound credential files, process output, process candidates, port candidates,
  loopback responses, retries, and cached samples.
- Probe loopback addresses only.
- Use the existing cooldown and last-good snapshot policy.
- Treat a missing remaining fraction as unknown, not empty or full.

### 6. Present Antigravity Usage

- Keep `google` as the canonical provider ID so live limits join local session
  usage.
- Use Google's stable OAuth subject for the canonical account key. A managed
  project, email, or credential fingerprint is not an account identity.
- Identify the live source as Antigravity and show its reported plan.
- Show all shared quota windows with provider-specific labels and reset times.
- Show AI credits separately from recurring quota.
- Include live-only Antigravity readings in the full Usage view.
- Always list Google in Settings > Usage so the meter can be hidden before the
  first successful reading or while its source is in error.
- Show authentication, throttling, schema, unavailable, and stale states.
- Reuse the current semantic design tokens and Google mark.

### 7. Benchmark Antigravity

- Add brain JSONL and nested cascade cases to `pipeline_baseline`.
- Measure claimed processing at 1 MiB, 10 MiB, and 50 MiB.
- Add synthetic native SQLite tiers at 100, 1,000, and 10,000 generations.
- Run each native tier with its sibling brain transcript through the real
  adapter and composite sink. Keep a database-only control.
- Include descriptor-backed primary and retry usage, deduplication, failed
  retries, step-only usage, generation-only usage, and a padded 280 KiB row.
- Add retained-memory and oversized-record probes.
- Select evidence capabilities by provider instead of always using Claude.
- Record the new results in `benches/BASELINE.md`.

## Tests

- Add synthetic fixtures for `agy`, Antigravity IDE 2.0, v1 files, cascades,
  the seven-table SQLite schema, protobuf wrappers, model usage, retries,
  step-only and generation-only calls, subagents, and quota errors.
- Test file mutation, cancellation, malformed records, and oversized records.
- Test malformed and oversized database rows followed by valid rows. Test the
  bounded identity state and nonzero analyzed input, output, and context.
- Test plan and quota parsing, credits, account changes, token refresh, HTTP
  failures, and local RPC fallback.
- Test partial weekly summaries, duplicate semantic buckets, remote model-quota
  fallback, and merging weekly plus model-scoped windows.
- Test that disabled usage settings cause no side effects.
- Test Google aggregation, live-only cards, quota labels, stale readings, and
  accessible text.
- Run all engine, shell, frontend, design drift, quality, and secret checks
  listed in `CONTRIBUTING.md`.

## Privacy and Performance

- Session content stays on the device.
- Credentials go only to Google endpoints selected by this integration.
- Antiburn does not persist refreshed credentials.
- Background work remains behind the existing reader-controlled usage switch
  and cooldown.
- Parsing memory is bounded by one record or one capped provider response, not
  by total transcript size.
- Native database analysis retains no result set and no complete blob list. It
  retains one capped row plus bounded invocation-to-model and deduplication maps.
- The protobuf field subset is reverse-engineered from descriptors embedded in
  official binaries. Unknown fields are skipped, and malformed wire data stops
  only the affected row.
