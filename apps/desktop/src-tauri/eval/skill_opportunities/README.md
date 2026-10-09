# Skill Opportunities diagnostics

`development` covers specialist opportunities, adequate direct work, irrelevant
and near skills, recorded use, unknown use, timing, incomplete history, duplicate
identities, description changes, and injection. `controls` adds certificate,
calendar, database-restoration, and CSV examples. Neither suite is embargoed.

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test skill_opportunities
ANTIBURN_EVAL_LIMIT=5 cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test skill_opportunities live -- --ignored --exact --nocapture --test-threads=1
```

Scoring uses the actual selected-provider plan, exact skill definition/work
bindings, and typed decisions. Mechanical skips do not erase positive labels
from recall. Missing judgments, semantic abstention, clean decisions, and failed
execution stay separate. Current inventory does not prove historical visibility.

Each run writes a light JSON report with diagnostic scores, provider/model
capabilities, errors, usage, and latency. No Python summarizer, frozen manifest,
accuracy assertion, or passing-development prerequisite is required.
See `../README.md` for common suite, case, provider, and output selection.
