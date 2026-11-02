# Shared provider diagnostics

`protocol.rs` loads the production transports, shared configuration, and direct
request runner. `provider.rs` resolves the selected connection once. `run.rs`
records validated responses, reported usage, latency, and failures.
`selection.rs` selects cases. `capture.rs` writes timestamped local JSON reports.

## Provider presets

Set `ANTIBURN_EVAL_PROVIDER` to one of these values:

| Preset                                  | Model               | Route                                   |
| --------------------------------------- | ------------------- | --------------------------------------- |
| `jev`                                   | `jev-latest`        | TypeSafe native, long context           |
| `ollama-nimble`                         | `nimble:latest`     | Local Ollama native                     |
| `ollama-clef-flash`                     | `clef-flash:latest` | Local Ollama native                     |
| `ollama-tev-small`                      | `tev1:0.8b`         | Local Ollama native                     |
| `cloudflare-clef`                       | `clef`              | Cloudflare native                       |
| `cloudflare-clef-flash`                 | `clef-flash`        | Cloudflare native                       |
| `custom-jev-direct`                     | `jev-1.14.0`        | Exact TypeSafe direct, standard context |
| `custom-ollama-nimble-direct`           | `nimble:latest`     | Exact local Ollama direct               |
| `custom-ollama-clef-flash-direct`       | `clef-flash:latest` | Exact local Ollama direct               |
| `custom-cloudflare-clef-envelope`       | `clef`              | Exact Cloudflare envelope               |
| `custom-cloudflare-clef-flash-envelope` | `clef-flash`        | Exact Cloudflare envelope               |

The default is Jev. Unknown presets fail. Local routes use
`http://127.0.0.1:11434/v1/systemone`. Hosted routes use production endpoint
builders. Hosted credentials come only from `TYPESAFE_API_KEY` or
`CLOUDFLARE_AUTH_TOKEN`. Cloudflare also requires `CLOUDFLARE_ACCOUNT_ID`.
Keys do not enter configuration records, reports, or client debug output.

Set `ANTIBURN_EVAL_MODEL` to override the preset model before validation,
endpoint construction, and discovery. Native Jev uses the route-owned
`jev-latest` alias, not a release-specific model selection.
Native Cloudflare accepts its supported model names. Ollama names require
verified discovery data. A Custom model selector does not establish support by
itself.

Presets do not supply manual input bounds. Known TypeSafe and Cloudflare routes
use their production capability profiles. Local Ollama presets, including the
exact Custom transports, use explicit Ollama discovery below. That lookup belongs
to these closed diagnostic presets. The app does not discover arbitrary Custom
endpoints or infer their limits from model names. Missing required limits block
execution.

Native Jev uses the production long-context capability profile and sends
`TypeSafe-Danger-Zone-Long-Context: true`. Custom does not receive that header or
profile. The immutable request alias and the observed served model are separate;
an alias echo leaves the concrete release unknown. No live call is needed to
resolve the native route before preparation.

## Direct requests

`EvalClient::from_environment()` returns `Result<EvalClient, JevError>`.
`evaluate_request(&client, &usage, case_id, stage, &request)` sends a shared
`JevRequest` and records its exact question IDs. The provider diagnostic does
not use check work items, packing, reducers, or baseline comparisons.

`configuration() -> &'static EvalConfiguration` initializes a `OnceLock`.
Local discovery uses the production Ollama client in a worker-thread Tokio
runtime. It records the selected model, digest, and effective limits. A matching
loaded `/api/ps` context takes priority over configured `num_ctx`. The advertised
model maximum only bounds that budget. A configured budget is not observed
runtime capacity. Missing loaded and configured limits do not imply 8,192
tokens. Execution reuses the resolved configuration.

The test binary re-exports `support::jev_cloudflare` at its crate root and
exposes `support::config` as `crate::jev::config`. The production transport
modules use those paths.

Production validators reject invalid requests and responses. Failed calls
remain in the request count. A failed response does not establish known usage
in this protocol diagnostic. No snapshot, accuracy threshold, or prior capture
controls execution. Run live probes only with explicit authorization.
