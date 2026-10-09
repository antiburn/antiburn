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
