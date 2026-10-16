# Smart Check diagnostics

These evals are a repeatable prompt testbed. Built-in smart-check execution and
historical enrollment are suspended in the app; their diagnostic harnesses and
provider implementations remain. YAML replay uses Jev only. Diagnostic provider
support does not establish a selectable app route or new built-in findings.
Scores describe the selected
synthetic cases; they are not delivery gates or population accuracy estimates.
Production evidence validation and deterministic correctness tests still apply.

| Cargo target           | Purpose                                                                        |
| ---------------------- | ------------------------------------------------------------------------------ |
| `ignored_instructions` | Instruction/action comparisons                                                 |
| `scope_creep`          | Performed work against recorded task scope                                     |
| `over_exploring`       | Redundant recorded output and clearly irrelevant reads                         |
| `skill_opportunities`  | Concrete current procedures for selected workflows                             |
| `provider_validation`  | One Choice/Noul protocol request                                               |
| Library `replay::`     | Burn Checks replay through production publication, engine, SQLite, and reports |

Each check has one ignored `live` entry point. Run it explicitly:

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test scope_creep live -- --ignored --exact --nocapture --test-threads=1
```

### Session replay

The [replay suite](replay/README.md) has one synthetic session-evidence builder,
one case registry, one offline scripted runner, and one ignored `replay::live`
entry point. It compiles the maintained YAML and feeds `ToolResultBatch` records
with frozen request context through production claim-fenced Store publication,
`StorePublishedEvidenceReader`, `FileEventSource`, core replay, and
`StoreCheckStore`. Each case gets a temporary Store and replay activation.
`RunScope::Replay` and `SequentialReplay` retain isolated, sequential prefix
acceptance. Burn Checks uses the `v1-prealpha` specification: experimental,
not standardized, and subject to breaking changes.

The six cases cover repeated failure, recovery, requested polling, uncertainty
after a failure, usable partial request context, and unavailable result context.
The source envelope exists
only to satisfy current evidence types. It makes no native parser, agent, or
producer-version coverage claim. The suite verifies production session-to-domain
conversion, exact selected inputs and request prefixes, and actual persisted
`burn_check_current_status` rows and writing-assessment provenance after every
settled prefix. The runner also checks replay activation isolation.

Run offline correctness first, then explicitly select the live diagnostic:

```sh
cargo nextest run --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --lib replay::
ANTIBURN_EVAL_CASES=rolling-recovery ANTIBURN_EVAL_LIMIT=1 cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --lib replay::live -- --ignored --exact --nocapture --test-threads=1
```

Use the shared `ANTIBURN_EVAL_CASES`, positive `ANTIBURN_EVAL_LIMIT`, and
`ANTIBURN_EVAL_OUTPUT_DIR` controls. The suite is `development`. The live runner
allows at most six cases, seven evaluations, and fourteen attempts. An offline
test verifies that the entire registry fits these same bounds. Each case has
at most 16 applied events, its declared evaluation count, twice that number of
attempts, and a 180-second execution deadline. The selection capture declares
aggregate bounds before dispatch. Partial request context still reaches assessment
with explicit quality limits; it is not a zero-call gate. Fatal errors stop
further cases and retain a summary. The separate missing-result-context case
must produce zero calls, attempts, accepted assessments, and status rows.

The declared model route currently requires `jev` and an inherited `TYPESAFE_API_KEY`.
Provider/model substitutions are rejected. Captures under
`target/eval-captures/replay/development/` retain actual inputs, labels,
probabilities, action outcomes, persisted status rows, provider usage, errors,
and latency. Live label and action quality is diagnostic; valid accepted results
and finite bounds are required. No broad dataset, frozen score, prior capture,
or dogfood-equivalence gate controls execution. Dogfood and eval definitions are
independent. This runner accepts synthetic evidence only.

Core Choice validation and the supported TypeSafe transport accept rounded
probability maps when `abs(sum - 1.0) <= 0.02` under `f64` arithmetic. Every
declared label must occur exactly once, the output must be declared, and every
individual probability must be finite and in `[0, 1]`. The adapter preserves
each raw probability. It does not normalize, clamp, or change the authored 0.95
action thresholds. For example, a 0.99 sum is accepted, but a raw concern
probability of 0.949 still fails the 0.95 gate. Sums outside tolerance and invalid
individual values remain permanent validation failures with available usage.
This intentional tolerance covers the supported provider's rounded maps; it is
not an accuracy or calibration claim for another provider.

The evaluator's `with_cancellation` callback is checked during shared admission
and while the request is in flight. Cancellation before sending returns a known
cancelled failure. Cancellation after sending returns unknown delivery and does
not permit an automatic resend. Runtime code supplies the callback.

Keep three test boundaries: unit tests and focused correctness integration tests,
case-by-case `live` quality evals, and desktop throughput benchmarks.
Each desktop worker has an ignored `benchmark_large_sessions` test. It indexes
large native synthetic sessions, loads Store candidates, runs the production
worker and transport, and reads durable results and production usage totals.
Only Tauri UI notification effects are replaced. The runner directly executes
candidates; scheduler wake, native session discovery, and debounce waits are not timed.

The large fixtures use these fixed default sizes:

| Check                | Native part records per session | Background records | Main-thread target evidence near source end                                            |
| -------------------- | ------------------------------: | -----------------: | -------------------------------------------------------------------------------------- |
| Ignored Instructions |                          25,004 |             25,000 | Fresh parser task and one complete Bash command against the current project rule       |
| Scope Creep          |                          25,028 |             25,000 | Fresh parser task, 24 complete unauthorized billing writes, and a closing user message |
| Over-exploring       |                          25,000 |             24,994 | Fresh parser task, four matching complete reads, and a closing user message            |
| Skill Opportunities  |                          25,005 |             25,000 | Current two-operation skill and a fresh recorded verification workflow                 |

These are synthetic native OpenCode sessions, not captures of real sessions.
Each fixture retains at least 25,000 source parts and more than 150 MiB of native
part JSON. Background work uses about 100 completed main-thread investigations
with at most 250 records each. Every background read has a distinct path and a completed result.
The part mix has about 32% tools. Most read outputs are 8 KiB, medium outputs are
24 KiB, and sparse large outputs are 90 KiB. Reasoning bodies are 1,857 bytes;
progress bodies are 480 bytes, with small native step-start/step-finish records.
Tool outputs dominate source bytes. This approximates the measured tool-heavy
shape; it does not reproduce the observed median, p95, or full distribution.

Real parser visits index the entire main thread and produce fenced Store rows
and evidence. Production scope assembly keeps at most 128 recent scope records
and 32 KiB of selected fields. Activity keeps at most 2,048 selected parts and
4 MiB, then prefers the latest task episode. Investigation spans keep the latest
256 episodes. These lossy limits keep large sessions processable and can omit
all provider candidates. No finding or provider-call requirement gates timing.
The regression `large_main_thread_session_is_processable` verifies the full
25,000-record workload, bounded retained inputs, and exact main-thread bindings.
Payload metadata counts
UTF-8 bytes of serialized native `part.data` JSON, excluding message rows, SQLite
schema/page overhead, and companion instruction/skill files. Separate database
file byte fields include SQLite overhead. Skill Opportunities reports actual part
row count rather than deriving it from the last message index.

The ignored throughput tests use these large defaults. The unignored Scope Creep
processability regression also uses 25,000 background records. Smaller correctness
fixtures use the same payload shape. Fixture constructors accept a record count.

`indexing_ms` measures the entire fixture callback: payload construction, SQLite
source writes, companion files, fingerprinting, real parser indexing, fenced Store
publication, candidate admission, local input preparation, and evidence assertions.
It also includes callback thread/runtime setup and fixture retention. It is not
parser-only time. `latency_ms` starts after fixture setup and measures candidate
execution through the durable assessment, including input reload, production
transport, and continuation waits. Throughput uses only this execution time.
The report includes both measurement definitions.

Each fixture prints phase timings as phases finish and records them in
`setup_phase_ms`. The nested `native_source_phase_ms` separates source index setup,
bulk record construction/inserts, and transaction commit. These nested values are
included in `native_source_build`; do not add them to the outer phase totals.
`native_publication` includes the shared fixture's fingerprint, parser visit,
fenced Store row writes, coverage/evidence publication, and initial candidate
query. Further separation inside that phase requires timing the shared publication
helper. Instructions also reports candidate admission, input preparation, and
plan validation separately. The outer `indexing_ms` still measures the whole callback.

The benchmark source schema includes the upstream OpenCode indexes defined in
[`session.sql.ts` at commit `51e310c9`](https://github.com/anomalyco/opencode/blob/51e310c9/packages/opencode/src/session/session.sql.ts):
`session_parent_idx(parent_id)`,
`message_session_time_created_id_idx(session_id, time_created, id)`,
`part_message_id_id_idx(message_id, id)`, and `part_session_idx(session_id)`.
The minimal shared fixture schema has only primary-key indexes. Without the
message-to-part index, the parser's per-message `WHERE message_id = ?` query scans
all part rows for every message. One message per part makes that work quadratic.
The benchmark builder creates the upstream indexes before bulk inserts; index
maintenance remains inside the timed setup. No parser shortcut or extra index
beyond those upstream definitions is used.

Expect roughly 165–180 MiB of native part JSON per root cluster, plus SQLite and indexed
evidence overhead. Benchmark-only source builders write the bulk native message
and part rows in one SQLite transaction. The existing shared correctness fixture
helpers remain unchanged. Source writes, parser work, and local evidence preparation
all grow;
the compact selected provider request does not grow in proportion to the source.
Ignored Instructions, Scope Creep, and Skill Opportunities use disk-backed Stores;
Over-exploring uses an in-memory indexed Store and releases its temporary native
source after publication. Scope Creep and Skill Opportunities retain all fixtures
for a check run; Ignored Instructions retains the latest fixture. Increasing the
session count therefore increases retained disk use for the first two checks.
Memory can exceed the source payload size while JSON, parser outputs, projections,
and input snapshots coexist. Measure peak memory and disk use on the benchmark
machine; the harness does not report them. The session timeout applies only to
worker execution and does not bound `indexing_ms`.

The shell also has an ignored Store-backed compact-read benchmark:

```sh
cargo nextest run --manifest-path apps/desktop/src-tauri/Cargo.toml --lib --run-ignored only -E 'test(=over_exploring_worker::tests::benchmark::measures_compact_smart_check_pipeline)' --nocapture
```

Its loopback workloads run eight positive/negative cases through the production
adapters, plus a large repeated-read session, constrained-capability cases, and a
24-session scheduler-selection workload. They use indexed Store fixtures, the
production HTTP transport, cache, reduction, and publication paths. The loopback
provider applies a fixed synthetic response delay and synthetic usage; this measures
local processing and scheduling behavior, not live model quality or hosted provider
latency. The 24-session workload currently measures one manually driven slice, not
an external wake-loop continuation. It does not include native discovery or parsing.
Use the large-session tests for live worker throughput and desktop profiling for
scheduler and discovery timing.

Run the complete development suite for one check and provider at a time. Jev
reads `TYPESAFE_API_KEY`. Cloudflare reads `CLOUDFLARE_ACCOUNT_ID` and
`CLOUDFLARE_AUTH_TOKEN` from the shell environment:

```sh
cargo nextest run --release --manifest-path apps/desktop/src-tauri/Cargo.toml --lib --run-ignored only -E 'test(benchmark_large_sessions)' --no-run
ANTIBURN_BENCHMARK_PROVIDER=jev cargo nextest run --release --manifest-path apps/desktop/src-tauri/Cargo.toml --lib --run-ignored only -E 'test(benchmark_large_sessions)' --nocapture --no-fail-fast
```

Set `ANTIBURN_BENCHMARK_PROVIDER=cloudflare-clef` or
`ANTIBURN_BENCHMARK_PROVIDER=cloudflare-clef-flash` for either Cloudflare model.
`ANTIBURN_BENCHMARK_MODEL` selects a model override. The default session count
is three; set `ANTIBURN_BENCHMARK_SESSIONS` to change it. `--nocapture` serializes
these tests. `ANTIBURN_BENCHMARK_TIMEOUT_SECS` defaults to 300 and accepts 1–3600
seconds per worker execution. For an initial large-fixture release run, use one
session per check and allow up to 900 seconds for worker execution:

```sh
ANTIBURN_BENCHMARK_PROVIDER=jev ANTIBURN_BENCHMARK_SESSIONS=1 ANTIBURN_BENCHMARK_TIMEOUT_SECS=900 cargo nextest run --release --manifest-path apps/desktop/src-tauri/Cargo.toml --lib --run-ignored only -E 'test(benchmark_large_sessions)' --nocapture --no-fail-fast
```

Completed sessions count terminal local worker executions, including partial
coverage and zero-call abstentions. `no_candidates` identifies a durable terminal
state without a findings payload. Read status, coverage, findings, decisions, and
call counts beside timing; completion does not prove Clean, useful findings, or
provider throughput. Unknown delivery and execution errors fail the benchmark. Use the
existing `ANTIBURN_EVAL_SUITE`, `ANTIBURN_EVAL_CASES`, and
`ANTIBURN_EVAL_LIMIT` controls to repeat a focused case or select the broader
`controls` suite. Do not run all ignored tests.

Set `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_AUTH_TOKEN` in the environment.
Then run one Cloudflare model across all four checks with:

```sh
ANTIBURN_BENCHMARK_PROVIDER=cloudflare-clef cargo nextest run --release --manifest-path apps/desktop/src-tauri/Cargo.toml --lib --run-ignored only -E 'test(benchmark_large_sessions)' --nocapture --no-fail-fast
```

Use `cloudflare-clef-flash` in place of `cloudflare-clef` for Clef-flash.

Compare successful runs with the same provider, model, suite, and case
selection. Provider latency varies. Compare reviewed-work and missing-answer
counts as well as elapsed time; an incomplete run is not a performance gain.

Common selection variables:

- `ANTIBURN_EVAL_PROVIDER`: one closed preset from `support/README.md`; defaults to Jev.
- `ANTIBURN_EVAL_SUITE`: `development` or `controls`; defaults to `development`.
- `ANTIBURN_EVAL_CASES`: optional comma-separated exact case IDs.
- `ANTIBURN_EVAL_LIMIT`: optional positive count, applied after case-ID selection.
- `ANTIBURN_EVAL_OUTPUT_DIR`: optional output root; defaults to `target/eval-captures`.
- `ANTIBURN_EVAL_RAW`: optional response details; absent by default to keep reports light.

Example bounded diagnostic:

```sh
ANTIBURN_EVAL_PROVIDER=ollama-nimble ANTIBURN_EVAL_LIMIT=3 cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test over_exploring live -- --ignored --exact --nocapture --test-threads=1
```

Reports contain provider/model capabilities, per-case outcomes and exact-binding
scores, errors, abstentions, usage, and latency. Keep semantic expectations,
expected bindings, and compact-policy admission separate. Broad legacy cases
can remain scope diagnostics; an intentionally omitted candidate is not proof
of a correct semantic rejection. Each execution writes a new
timestamped JSON report. No snapshots, source/request hashes, freezes, previous
passing reports, consumed-case locks, or Python tools are required. Production
binding IDs and model digests remain where they identify real evidence or models.

Keep weak scores and errors visible. A transport failure, missing outcome, or
unsafe publication stops further calls in that diagnostic and is recorded in
the report. It does not remove remaining selected cases from score denominators.
Do not change labels to improve results. `controls` contains reusable contrasting
examples, not a claim of unseen confirmation inputs.

Live calls require authorization and environment-only credentials. Never select
every ignored test: imported production transport tests also contain paid probes.
Serialize local inference across processes. Protocol success does not establish
check quality or native source coverage.

## Compact production policy

Each input snapshot selects at most two Ignored Instructions comparisons, two
Scope Creep groups, three Over-exploring targets, or one Skill Opportunities pair.
The four checks fit retained candidates into one logical request, excluding finite
transport retries. Zero qualifying candidates means zero calls. Selection is
nonexhaustive; later turns do not drain policy-omitted candidates from the same
snapshot. Context rejection remains unassessed without child requests.

Positive decisions need a unique winning choice at probability 0.95 or higher,
plus the check's local publication gates. This is not a 95% accuracy claim.
Current rules do not prove historical activation. Current skills do not prove
historical availability or whole-session non-use. Scope Creep requires bound
performed work and sufficient authorization history. Over-exploring does not
judge general file breadth or the size of a first read.

Capabilities determine fit before dispatch. Preserve separate byte, total-input,
rendered-question, question-count, and criteria limits. A smaller capacity can
reduce evidence or cause zero-call abstention; it must not cause overflow calls.
For the verified Ollama renderer, the fit estimate is the largest scoring input,
not aggregate scoring work or actual provider usage. Report those quantities
separately. Report requests, request bytes, findings, abstentions, and completion
time, including zero-call cases, where the harness captures them. A transport
failure is a failed assessment, not a successful cheap result. Old captures from
the broad policy do not establish compact-policy quality or a measured speedup.

Offline checks:

```sh
cargo nextest run --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --no-fail-fast --test ignored_instructions --test scope_creep --test over_exploring --test skill_opportunities --test provider_validation
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test ignored_instructions --test scope_creep --test over_exploring --test skill_opportunities --test provider_validation -- -D warnings
```
