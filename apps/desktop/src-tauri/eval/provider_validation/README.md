# Provider protocol diagnostic

One ignored `live` test sends a synthetic work item with a Choice question and
a Noul question through the selected production transport. It records normalized
answers, usage, errors, and discovered capabilities. It does not establish check
quality or native source coverage.

```sh
ANTIBURN_EVAL_PROVIDER=cloudflare-clef cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test provider_validation live -- --ignored --exact --nocapture --test-threads=1
```

See `../support/README.md` for all presets and environment credentials.
The report is repeatable and has no snapshot or prior-result prerequisite.
`ANTIBURN_EVAL_SUITE` labels the report directory; both suites use the same
protocol work item. Execution or packing errors save a report and fail the test.
Shared offline tests exercise real production Custom and Ollama transports
against loopback HTTP, including strict response and failed-usage handling.
