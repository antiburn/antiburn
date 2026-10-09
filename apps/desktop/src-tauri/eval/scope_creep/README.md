# Scope Creep diagnostics

`development` retains ordinary scope contrasts, source-format controls, long
scope, approval timing, uncertain authority, failed work, and injection cases.
`controls` adds varied recorded episodes. Labels and exact production work/scope
bindings are scored together. Missing evidence stays unassessed.

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test scope_creep
ANTIBURN_EVAL_LIMIT=5 cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test scope_creep live -- --ignored --exact --nocapture --test-threads=1
```

Use `ANTIBURN_EVAL_SUITE=controls` or exact `ANTIBURN_EVAL_CASES` IDs to select
contrasts. See `../README.md` for the shared provider, selection, and output
variables. Each live run uses production preparation, packing, transport,
validation, and reduction. Precision and recall are diagnostics, not assertions.
Synthetic source-format cases do not establish native parser or worker support.
