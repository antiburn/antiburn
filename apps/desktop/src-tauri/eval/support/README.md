# Shared evaluation support

`provider.rs` discovers a selected connection once and calls its production
transport. `run.rs` records response usage, latency, and errors. `selection.rs`
handles suite/case selection. `capture.rs` writes timestamped JSON reports.

## Provider presets

Set `ANTIBURN_EVAL_PROVIDER` to one of these values:

| Preset                                  | Model               | Route                     |
| --------------------------------------- | ------------------- | ------------------------- |
| `jev`                                   | Pinned Jev          | TypeSafe native           |
| `ollama-nimble`                         | `nimble:latest`     | Local Ollama native       |
| `ollama-clef-flash`                     | `clef-flash:latest` | Local Ollama native       |
| `ollama-tev-small`                      | `tev1:0.8b`         | Local Ollama native       |
| `cloudflare-clef`                       | `clef`              | Cloudflare native         |
| `cloudflare-clef-flash`                 | `clef-flash`        | Cloudflare native         |
| `custom-jev-direct`                     | Pinned Jev          | Exact TypeSafe direct     |
| `custom-ollama-nimble-direct`           | `nimble:latest`     | Exact local Ollama direct |
| `custom-ollama-clef-flash-direct`       | `clef-flash:latest` | Exact local Ollama direct |
| `custom-cloudflare-clef-envelope`       | `clef`              | Exact Cloudflare envelope |
| `custom-cloudflare-clef-flash-envelope` | `clef-flash`        | Exact Cloudflare envelope |

Unset selection preserves Jev. Unknown presets fail. Local routes use
`http://127.0.0.1:11434/v1/systemone`. Hosted routes use production endpoint
builders. Hosted credentials come only from `TYPESAFE_API_KEY` or
`CLOUDFLARE_AUTH_TOKEN`; Cloudflare also needs `CLOUDFLARE_ACCOUNT_ID`.
Keys never appear in configuration records, reports, or client Debug output.

Set `ANTIBURN_EVAL_MODEL` to override the preset model before connection
validation, endpoint construction, and discovery. Ollama discovery uses the
selected model, not the preset default. Production validation applies: native
Jev requires its pinned model and native Cloudflare accepts its supported model
names. Ollama and compatible Custom endpoints accept other model names.

Exact Custom presets include an explicit total-input bound: 8,192 tokens for
local Ollama routes and 65,536 for Jev and Cloudflare routes. These bounds exceed
the Custom rendering reserve. Discovery can lower these bounds; manual overrides
never raise a known provider limit.

## API

```rust
let provider = support::provider::configuration();
let client = support::provider::EvalClient::from_environment().await?;
let plan = check.prepare_with_capabilities(&context, &provider.capabilities)?;
```

`configuration() -> &'static EvalConfiguration` initializes a `OnceLock`.
Local discovery uses the production Ollama client in a worker-thread Tokio
runtime, so callers can use the synchronous accessor inside async tests.
Discovery records the model digest and effective limits. A matching loaded
`/api/ps` context takes priority over configured `num_ctx`; the advertised model
maximum only bounds that budget. A configured budget is not observed runtime
capacity. Missing loaded and configured limits do not imply 8,192 tokens.
The native small-Tev preset has no model-name context-size assumption. Execution reuses
that same record; it does not rediscover or require a snapshot file.
Hosted and exact Custom presets use their matching production capability data.

Use the check's capability-aware context constructor when it has one. Execute
the prepared plan or use `run_jev_check_with_capabilities`. Packing, validation,
unpacking, and reduction must receive the same capabilities.

`EvalClient::from_environment()` returns `Result<EvalClient, JevError>`.
`evaluate_batch(&client, &usage, case_id, stage, &batch)` accepts only this client.
It records validated responses and failed calls without a second transport layer.

The test binary must re-export `support::jev_cloudflare` at its crate root and
expose `support::config` as `crate::jev::config` because the imported production
transport modules use those paths.

No evaluation hash, snapshot, freeze, accuracy threshold, or previous capture
controls execution. Credentials stay in memory. Imported production validators
still reject invalid requests and responses.

## Native desktop benchmarks

The test-only `crate::jev::benchmark` module exposes:

```rust
pub(crate) async fn run_benchmark(
    check: &'static dyn JevCheckDescriptor,
    fixture: impl Fn(usize) -> anyhow::Result<(Store, BurnCheckCandidate, serde_json::Value)>
        + Send + Sync + 'static,
) -> anyhow::Result<()>
```

Each check adapter constructs its native fixture. The index starts at zero.
Return a fresh Store, its native candidate, and JSON fixture metadata. Keep
source files alive through execution. An unexecuted queued assessment is allowed;
do not seed a result, prior request, or response cache.

`ANTIBURN_BENCHMARK_PROVIDER` uses the presets above and defaults to `jev`.
`ANTIBURN_BENCHMARK_MODEL` overrides the model before validation.
`ANTIBURN_BENCHMARK_SESSIONS` defaults to 3 and accepts 1 through 1,000.
`ANTIBURN_BENCHMARK_TIMEOUT_SECS` defaults to 300 and accepts 1 through 3,600.
Credentials use the same environment variables as evaluation.

`provider_config.rs` contains only preset connection and credential configuration.
Eval and benchmark import it with aliases to their parent production config and
Cloudflare endpoint builder. The benchmark does not import eval transport or
usage-recording code. It installs the connection and credential in
`WorkerHandle`, obtains the execution client and current generation, and calls
the descriptor's production `run_candidate` with `TestWorkerEffects`.
Continuations wait for the Store's scheduled deadline. No result bypass is used.

Sessions run sequentially. Latency starts before candidate execution, includes
preparation, capability discovery, requests, reduction, persistence, and
continuation delays, and excludes fixture indexing. Indexing is timed separately.
Completion throughput is completed sessions divided by total measured execution
time. JSON and console output include session latency, fixture metadata, durable
status and request counts, Store usage deltas, findings, zero-call sessions, and
errors. A measured completion requires a durable completed assessment, a partial
assessment with a findings array, or a terminal `no_candidates` state, with no
unknown provider outcomes. Zero-call sessions count as local terminal executions;
they do not establish provider throughput or useful findings. `durable_completed` records the Store status
separately. `zero_calls` means no confirmed calls and no unknown outcomes, even
when the assessment request count is nonzero. Failed sessions are reported before
the runner returns an error.

## Baseline question comparisons

Set `ANTIBURN_EVAL_BASELINE_QUESTIONS` to a local JSON map of production
`JevQuestion` values to compare old questions on the same selected state. Use a
`default` key for a single question, or `unrelated`, `breadth`, and `extent` for
Over-exploring. This diagnostic requires a native Jev or Ollama preset. Jev
baseline dispatch requires explicit authorization for the extra hosted calls.
Each new batch gets one baseline preflight and at most one extra dispatch.
Requests that exceed the discovered limits remain undispatched. Baseline answers do not enter
the new execution. Captures retain separate baseline reductions through the
current reducer. Old choices map to the current schema with zero probability
for added choices. This compares question changes, not old reducer behavior.
Raw captures record fit errors, baseline responses, request sizes, estimated
payload tokens, and per-call latency. These estimates differ
from provider-reported input tokens and do not include the rendering reserve.
Question substitution preserves request state, question IDs, context routes,
and the selected instruction-target index. Usage keeps new and baseline totals
separate. Provider-call latency excludes baseline preflight and the other call;
case latency includes both executions.
Read baseline selection uses the routed state's typed reason, so prompt wording
changes do not select the wrong old question. Check captures record the actual
projection, chunking, question, and reducer revisions used for that execution.

All four harnesses use the production runner. Over-exploring rows record
`production_runner` so new captures can be distinguished from earlier local
compact-prompt diagnostics that bypassed the empty-shared-evidence defect.
The Ollama adapter preserves `ContextRejected`. The four compact production
checks disable post-dispatch context replanning: the rejected request remains
unassessed and creates no child dispatches. Other engine callers can retain
their separate bounded replan policy. Capability-driven reduction happens locally
before the first request. The one-logical-request policy excludes finite transport
retries and separately authorized baseline comparisons.
