# Smart Check diagnostics

These evals are a repeatable prompt testbed. Scores describe the selected
synthetic cases; they are not delivery gates or population accuracy estimates.
Production evidence validation and deterministic correctness tests still apply.

| Cargo target           | Purpose                                      |
| ---------------------- | -------------------------------------------- |
| `ignored_instructions` | Instruction/action comparisons               |
| `scope_creep`          | Performed work against recorded task scope   |
| `over_exploring`       | Read relevance, breadth, and observed extent |
| `skill_opportunities`  | Current skill fit, benefit, and recorded use |
| `provider_validation`  | One Choice/Noul protocol request             |

Each check has one ignored `live` entry point. Run it explicitly:

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test scope_creep live -- --ignored --exact --nocapture --test-threads=1
```

Each check also has an ignored `benchmark_session` entry point. It runs the
selected synthetic suite through that check's production preparation, provider,
continuation, and reduction path. Reports include per-case stage times and
per-request latency. Cases are separate synthetic sessions; these benchmarks do
not measure desktop source discovery, native parsing/publication, Store queries,
or the desktop scheduler. Use the desktop profiler for those stages.

Run the complete development suite for one check and provider at a time. Jev
reads `TYPESAFE_API_KEY`. Cloudflare reads `CLOUDFLARE_ACCOUNT_ID` and
`CLOUDFLARE_AUTH_TOKEN` from the shell environment:

```sh
ANTIBURN_EVAL_PROVIDER=jev cargo test --release --manifest-path apps/desktop/src-tauri/Cargo.toml --test ignored_instructions benchmark_session -- --ignored --exact --nocapture --test-threads=1
ANTIBURN_EVAL_PROVIDER=jev cargo test --release --manifest-path apps/desktop/src-tauri/Cargo.toml --test scope_creep benchmark_session -- --ignored --exact --nocapture --test-threads=1
ANTIBURN_EVAL_PROVIDER=jev cargo test --release --manifest-path apps/desktop/src-tauri/Cargo.toml --test over_exploring benchmark_session -- --ignored --exact --nocapture --test-threads=1
ANTIBURN_EVAL_PROVIDER=jev cargo test --release --manifest-path apps/desktop/src-tauri/Cargo.toml --test skill_opportunities benchmark_session -- --ignored --exact --nocapture --test-threads=1
```

Set `ANTIBURN_EVAL_PROVIDER=cloudflare-clef` or
`ANTIBURN_EVAL_PROVIDER=cloudflare-clef-flash` to benchmark either Cloudflare
model. Run each check/model separately to keep reports attributable. Use the
existing `ANTIBURN_EVAL_SUITE`, `ANTIBURN_EVAL_CASES`, and
`ANTIBURN_EVAL_LIMIT` controls to repeat a focused case or select the broader
`controls` suite. Do not run all ignored tests.

For example, run one Cloudflare model across all four checks with:

```sh
source ~/dev/cloudflare-env
ANTIBURN_EVAL_PROVIDER=cloudflare-clef cargo test --release --manifest-path apps/desktop/src-tauri/Cargo.toml --test ignored_instructions benchmark_session -- --ignored --exact --nocapture --test-threads=1
ANTIBURN_EVAL_PROVIDER=cloudflare-clef cargo test --release --manifest-path apps/desktop/src-tauri/Cargo.toml --test scope_creep benchmark_session -- --ignored --exact --nocapture --test-threads=1
ANTIBURN_EVAL_PROVIDER=cloudflare-clef cargo test --release --manifest-path apps/desktop/src-tauri/Cargo.toml --test over_exploring benchmark_session -- --ignored --exact --nocapture --test-threads=1
ANTIBURN_EVAL_PROVIDER=cloudflare-clef cargo test --release --manifest-path apps/desktop/src-tauri/Cargo.toml --test skill_opportunities benchmark_session -- --ignored --exact --nocapture --test-threads=1
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
scores, errors, abstentions, usage, and latency. Each execution writes a new
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

Offline checks:

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --no-fail-fast --test ignored_instructions --test scope_creep --test over_exploring --test skill_opportunities --test provider_validation
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test ignored_instructions --test scope_creep --test over_exploring --test skill_opportunities --test provider_validation -- -D warnings
```
