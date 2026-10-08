# Over Exploring diagnostics

`development` covers unrelated files, unnecessary file breadth, unnecessary
within-file extent, near negatives, requested review, protocol literals, and
authority/evidence controls. `controls` retains additional semantic contrasts
from the former confirmation examples. Both suites are reusable prompt tests.

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test over_exploring
ANTIBURN_EVAL_LIMIT=5 cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test over_exploring live -- --ignored --exact --nocapture --test-threads=1
```

Reports show overall and per-reason exact outcome/read-binding scores,
abstentions, clean decisions, errors, and usage. They do not assert an accuracy
floor. Production probability and evidence requirements remain in the engine.
Offline native tests characterize source-specific read facts separately from
model diagnostics. See `../README.md` for common selection and provider options.
