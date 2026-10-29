# Provider protocol diagnostic

One ignored `live` test sends one synthetic System One request with Choice,
Score, and Noul questions together through the selected production transport. It
records normalized answers, usage, errors, and discovered capabilities. It does
not establish check quality or native source coverage.

```sh
ANTIBURN_EVAL_PROVIDER=cloudflare-clef cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test provider_validation live -- --ignored --exact --nocapture --test-threads=1
```

See `../support/README.md` for all presets and environment credentials.
The report is repeatable and has no snapshot or prior-result prerequisite.
`ANTIBURN_EVAL_SUITE` labels the report directory; both suites use the same
protocol request. Execution or preparation errors save a report and fail the test.
The request uses the selected model and shared request validation. It does not
use check work items, packing, or baseline question comparisons.
Shared offline tests exercise real production Custom and Ollama transports
against loopback HTTP, including strict response and failed-usage handling.
