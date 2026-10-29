# Provider and Burn Check diagnostics

These diagnostics exercise shared provider protocols and isolated YAML replay.
They do not restore retired native checks or establish native source coverage.
Live requests require explicit authorization and environment-only credentials.
Never select every ignored test; production transport probes can incur charges.
Use the [operator acceptance guide](../../../../docs/runbooks/burn-check-live-acceptance.md)
for request limits, native event probes, and report checks.

| Cargo target          | Purpose                                                                                         |
| --------------------- | ----------------------------------------------------------------------------------------------- |
| `provider_validation` | One synthetic mixed Choice/Score/Noul protocol request through a selected production transport. |
| Library `replay::`    | YAML replay through production publication, engine, SQLite, and reports.                        |

## Provider protocol validation

Run offline correctness checks first:

```sh
cargo nextest run --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test provider_validation
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test provider_validation -- -D warnings
```

The [provider diagnostic](provider_validation/README.md) sends one synthetic
System One request with Choice, Score, and Noul questions together. It records
normalized answers, capabilities, usage, errors, and latency. An authorized live
run must select that exact test:

```sh
ANTIBURN_EVAL_PROVIDER=cloudflare-clef cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --test provider_validation live -- --ignored --exact --nocapture --test-threads=1
```

Shared transports support Jev, compatible Ollama, Cloudflare, and exact Custom
routes. The [provider presets](support/README.md#provider-presets) define the
closed route names and environment variables. Hosted credentials come only from
the authorized shell environment. Protocol success does not establish model
quality, source support, or every output capability. The provider controls under
**Model-based checks** select the saved System One connection separately. Its typed
multi-decision YAML path admits only the selected route's verified per-type
outputs, criteria intervals, and required details.

## Session replay

The [replay suite](replay/README.md) has one synthetic session-evidence builder,
one case registry, one offline scripted runner, and one ignored `replay::live`
entry point. It compiles maintained YAML and feeds `ToolResultBatch` records with
frozen request context through production claim-fenced Store publication,
`StorePublishedEvidenceReader`, `FileEventSource`, core replay, and
`StoreCheckStore`. Each case gets a temporary Store and replay activation.
`RunScope::Replay` and `SequentialReplay` retain isolated, sequential prefix
acceptance. Burn Checks use the experimental `v1-alpha` draft, not a
standardized or backward-compatible language.

The six cases cover repeated failure, recovery, requested polling, uncertainty
after a failure, usable partial request context, and unavailable result context.
The synthetic source envelope satisfies current evidence types; it makes no
native parser, agent, or producer-version claim. Tests check selected input,
request prefixes, persisted current status, writing-assessment provenance after
each settled prefix, and replay activation isolation.

Run offline correctness first. Select exact cases and bound authorized live work:

```sh
cargo nextest run --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --lib replay::
ANTIBURN_EVAL_CASES=rolling-recovery ANTIBURN_EVAL_LIMIT=1 cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --lib replay::live -- --ignored --exact --nocapture --test-threads=1
```

Replay uses `ANTIBURN_EVAL_CASES`, positive `ANTIBURN_EVAL_LIMIT`, and optional
`ANTIBURN_EVAL_OUTPUT_DIR`. Its suite is `development`. The live registry permits
at most six cases, seven evaluations, and fourteen attempts. Each case permits
at most 16 applied events, its declared evaluation count, twice that number of
attempts, and a 180-second execution deadline. Offline tests verify the registry
fits those bounds. Selection captures declare aggregate bounds before dispatch.
Fatal errors stop further cases and retain a summary.

Usable partial request context can reach assessment with explicit quality limits.
The separate missing-result-context case requires zero calls, attempts, accepted
assessments, and status rows. The declared route requires `jev` and an inherited
`TYPESAFE_API_KEY`. Provider/model substitutions are rejected. These limits
do not change the app's selected connection or certify other routes for this
replay suite. Production YAML routing comes from Settings, not the definition.

Captures under `target/eval-captures/replay/development/` retain actual synthetic
inputs, labels, probabilities, action outcomes, persisted status, provider usage,
errors, and latency. Live label quality is diagnostic, not population accuracy or
a delivery gate. Valid accepted results and finite bounds remain required.
No prior capture, frozen score, or dogfood-equivalence gate controls execution.
Dogfood and eval definitions are independent; replay accepts synthetic evidence only.

## Validation and cancellation limits

Core Choice validation and the TypeSafe transport accept rounded probability
maps when `abs(sum - 1.0) <= 0.02` under `f64` arithmetic. Each declared label
must occur exactly once, the output must be declared, and every probability must
be finite and in `[0, 1]`. Raw probabilities are not normalized or clamped. A
0.99 sum can pass, but a raw 0.949 probability does not meet an authored 0.95 gate.
Invalid values remain permanent validation failures with available usage.
This tolerance is not an accuracy or calibration claim for another provider.

The evaluator checks `with_cancellation` during admission and in flight.
Cancellation before sending is known cancelled delivery. Cancellation after
sending is unknown delivery and does not permit an automatic resend. Earlier
attempts may have incurred charges. Serialize local inference across processes.
