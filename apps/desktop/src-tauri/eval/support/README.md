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
Discovery records actual model digest and effective context. Execution reuses
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
