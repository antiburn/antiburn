# Over Exploring diagnostics

`development` covers repeated identical outputs, clearly unrelated sampled
reads, explicit verification, relevant dependencies, near negatives, and
authority/evidence controls. `controls` retains additional semantic contrasts
for those two supported reasons. Both suites are reusable prompt tests.

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test over_exploring
ANTIBURN_EVAL_LIMIT=5 cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test over_exploring live -- --ignored --exact --nocapture --test-threads=1
```

Cases record the expected semantic decision, exact source bindings, and policy
admission separately. Admission comes from the production selector. Reports
expose outcome and binding correctness, plus unsupported and policy-limited
counts. Reports do not assert an accuracy floor. Production probability and
evidence requirements remain in the engine.
Offline native tests characterize source-specific read facts separately from
model diagnostics. See `../README.md` for common selection and provider options.
