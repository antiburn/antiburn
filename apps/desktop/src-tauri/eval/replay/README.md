# Burn Checks replay diagnostics

Run the [synthetic retry-review fixture](../../../../../crates/antiburn-checks/tests/fixtures/retry-review.yml) against small synthetic session
evidence. Burn Checks uses the `v1-alpha` specification: experimental, not
standardized, and subject to breaking changes.

One fixture builder creates `ToolResultBatch` values with linked
`ToolResultObservation` records, frozen `ToolResultContext` prefixes, and valid
`UserMessage` evidence. The runner uses production claim-fenced Store publication,
`StorePublishedEvidenceReader`, `FileEventSource`, core replay, and
`StoreCheckStore`. It does not supply domain events or prepared decision inputs
as fixtures. The fixed source envelope satisfies the current evidence types;
it makes no agent, parser, or producer-version coverage claim.

Replay retains `RunScope::Replay` isolation and `SequentialReplay` execution.
Each eligible prefix settles before the next event. Replay bypasses the authored
cooldown and retains provider admission, deadlines, and safe retry rules.

## Run

Run offline correctness before authorized live diagnostics:

```sh
cargo nextest run --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --lib replay::
ANTIBURN_EVAL_CASES=rolling-recovery ANTIBURN_EVAL_LIMIT=1 cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --lib replay::live -- --ignored --exact --nocapture --test-threads=1
```

The live test requires an inherited `TYPESAFE_API_KEY` and the compiled `jev`
route. The diagnostic selects its decision model; YAML does not route it. Cargo's `--offline` flag does not
prevent the selected provider request.

Both runners use this registry:

| Case                         | Evaluations | Scripted behavior                                                                              |
| ---------------------------- | ----------: | ---------------------------------------------------------------------------------------------- |
| `repeated-failure`           |           1 | Write failure after three matching failures.                                                   |
| `rolling-recovery`           |           2 | Write failure, then success after a correction.                                                |
| `requested-retry`            |           1 | Write success for requested startup polling.                                                   |
| `uncertain-after-failure`    |           2 | Write failure, then write success for an uncertain answer.                                     |
| `partial-request-context`    |           1 | Review available evidence with incomplete request history; an uncertain answer writes success. |
| `unavailable-result-context` |           0 | Reject missing frozen result context as unavailable before evaluation.                         |

Use the shared `ANTIBURN_EVAL_CASES`, positive `ANTIBURN_EVAL_LIMIT`, and
`ANTIBURN_EVAL_OUTPUT_DIR` controls. The suite is `development`. The live runner
allows at most six cases, seven evaluations, and fourteen attempts. An offline
registry test checks the entire registry against the same live admission budget.
Each case has
a 16-event limit, twice its evaluation count in attempts, and a 180-second
execution deadline. Fixture publication happens before that deadline.

Captures use shared timestamped JSON files under
`target/eval-captures/replay/development/`, or the selected output directory.
One selection capture records bounds. One summary retains actual decision inputs,
labels, probabilities, action outcomes, provider usage, errors, latency, and the
persisted `burn_check_current_status` rows after every settled prefix.

## Assertions and limits

The suite checks production session-to-domain conversion, exact linked arguments
and results, native order, and exact request prefixes before evaluation. The
recovery case adds a later request to detect future-context leakage. Both runners
check accepted assessments, settled attempts, actual current-status rows and
their writing assessment IDs, no-write provenance, and replay activation
isolation. Partial request history remains assessable and reaches the evaluator
with `request_history_complete: false`. The scripted uncertain answer writes
success through the authored complement action; the live model's label remains
diagnostic. This case does not claim that
partial evidence requires zero calls. Unsafe identity/order and unavailable
required-expression gates belong to focused source and engine correctness tests.
The separate unavailable-context case omits `ToolResultContext` entirely. It must
produce source gaps, zero evaluator calls, no attempts or accepted assessments,
and no status rows. The fixture does not inject gaps or prepared decision input.

Scripted labels test deterministic engine behavior. Live labels and action
quality are diagnostics, not accuracy or release gates. The suite has no frozen
scores, broad dataset, prior-capture gate, or dogfood-equivalence check. The replay
reads a test-only definition and keeps separate execution and saved state. It
does not validate the five installed repository definitions.

Add a case in `cases.rs` with an explicit test-fixture YAML reference, synthetic session
evidence, expected trigger prefixes, and scripted labels and status writes. Keep
the finite bounds explicit. Native parser characterization and real-session
evaluation are separate work.
