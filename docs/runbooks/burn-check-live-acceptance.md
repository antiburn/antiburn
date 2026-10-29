# Burn Check live acceptance

These are optional operator-run checks. Automated tests use synthetic evidence
and loopback transports. They do not establish live provider quality or support
for every agent version. See [session coverage](../session-coverage.md) for the
accepted native shapes and [check coverage](../check-coverage.md) for product limits.

## Set the scope first

- Select disposable local sessions and a test repository that you may send to the
  chosen provider. Do not use private work unless you approve that disclosure.
- Approve current global and repository inventory inputs separately before any
  inventory check. They can contain instruction text, skill descriptions,
  metadata, and absolute paths outside the test repository. The canonical
  inventory checks select whole `global` and `repo` objects, which can include
  both inventories even when the question uses only one. Repository approval
  alone does not approve these inputs. Leave these checks off if you cannot
  approve them, or use an approved synthetic test environment.
- Record the agent versions, connection profiles, request limit, and cost cap in
  ignored local notes. Keep notes and captures under `.agent-artifacts/reviews/`.
  Do not commit credentials or real session content.
- Turn off **Run checks** under Model-based checks before setting up the test. Saved connections
  remain available. Enabling checks in a repository with model-based definitions
  can send requests. The app's usage estimate is not a spending cap.
- Run providers one at a time. Load credentials into the approved shell
  environment without printing them. Do not change credentials just to run a test.

## Check one provider protocol

Run the offline diagnostic first:

```sh
cargo nextest run --manifest-path apps/desktop/src-tauri/Cargo.toml --locked --offline --test provider_validation
```

Then select one approved preset and run exactly one live test:

```sh
ANTIBURN_EVAL_PROVIDER=jev \
ANTIBURN_EVAL_LIMIT=1 \
ANTIBURN_EVAL_OUTPUT_DIR="$PWD/.agent-artifacts/reviews/burn-check-live" \
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --locked --offline \
  --test provider_validation live -- --ignored --exact --nocapture --test-threads=1
```

Cargo's `--offline` flag prevents dependency downloads, not provider requests.
The diagnostic sends one synthetic request with Choice, Score, and Noul questions.
Do not use `--include-ignored` or run all ignored tests.

Repeat only within your approved request and cost limits. Supported presets include:

| Connection              | Example preset                    | Required environment                                          |
| ----------------------- | --------------------------------- | ------------------------------------------------------------- |
| Jev                     | `jev`                             | `TYPESAFE_API_KEY`                                            |
| Compatible local Ollama | `ollama-nimble`                   | A compatible runner and model at the documented local address |
| Cloudflare              | `cloudflare-clef`                 | `CLOUDFLARE_AUTH_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`              |
| Custom direct           | `custom-jev-direct`               | `TYPESAFE_API_KEY` for this exact preset                      |
| Custom envelope         | `custom-cloudflare-clef-envelope` | The Cloudflare variables above                                |

The [preset guide](../../apps/desktop/src-tauri/eval/support/README.md) defines
models, addresses, and other presets. These commands do not test arbitrary chat
endpoints or change the app's selected connection.

Inspect the saved report for the selected route, exact declared answers,
capabilities, errors, and reported usage. Missing usage stays unknown. A valid
answer proves that request's protocol path, not model accuracy or calibration.
Stop after an error or unknown outcome. Do not automatically repeat the request.

For native Jev, distinguish the requested `jev-latest` alias from the response's
served model. The native route enables long context; Custom presets do not
inherit its header or limits. An alias echo does not prove a concrete release.
A future or unknown served model can have confirmed tokens without a cost
estimate. Do not retry a valid response to obtain a known model or price.

## Check native events without model calls

Use an isolated test OS account with only approved sources and local probe
definitions. Keep the normal Antiburn instance stopped. Before enabling checks,
verify the effective global definitions and every active repository scope, not
just files in the test repository. No model-based definition may be active in
any scope for this section. A separate database or an open test repository alone
does not isolate source discovery or global definitions. If you cannot verify
this isolation, stop before enabling the master switch.

A local check has no `decisions` block and makes no model request. It still
requires **Run checks** to be enabled and a configured, selected
connection. Do not use **Test connection** during this no-model procedure.
Record prior enablement and pause settings so you can restore them afterward.

For example, save this as `.antiburn/checks/user-event-probe.yml` in the test
repository:

```yaml
name: User event probe
on:
  user_message:
  cooldown: 0s
actions:
  - type: status
    input:
      result: success
      message: Observed an admitted user message.
```

Use separate probe files for `assistant_message`, `tool_call`, `tool_result`,
`turn_end`, `session_start`, and `session_idle`. Select one event per file. These
forms use the experimental `v1-alpha` draft, which has no compatibility promise.
Do not add a YAML version field.

For each approved agent session:

1. Open its test repository, enable the probes, and confirm that discovery admits
   its local main-session source and exact working directory.
2. Send a short user request. Make one explicit read or search call, wait for its
   result, and let the agent finish. Confirm that a call and its result have
   separate evidence identities. Private reasoning and unfinished text must not
   appear as public assistant evidence.
3. Expect public assistant messages only where finality is supported. Claude
   assistant messages remain unavailable. Complete response cycles require the
   accepted Codex start/terminal interval or a continuously observed OpenCode V2
   execution period. Claude, Pi, and OpenCode V1 cycles remain unavailable.
4. Confirm one `attached` observation per attachment. Do not interpret it as a
   newly created or resumed native session.
5. Leave the admitted source quiet for at least 120 seconds after catch-up.
   Confirm one idle observation. Restart Antiburn and confirm that the same idle
   period does not create another assessment. New native work must start a fresh
   interval. UI actions and model-selection changes must not reset it.
6. Check exact saved inputs in the assessment detail. Message checks have message
   context; cycles have turn context; start and idle events have session context.
   A cycle or session event must not contain a fabricated tool result.

Missing source proof is an expected unavailable result. Do not relax ownership,
working-directory, version, or continuity checks to make a probe pass.

## Check model-based assessment and Settings

Keep the isolated source and definition setup for this section. Test one approved
definition against one approved session at a time. The app has no global request
or cost cap. Define the exact assessment count and stop condition before enabling
each check. Include every decision, safe retry, and connection test in your
request budget. Use a provider-enforced cap when a hard cost bound is required.
If you cannot isolate or bound the paid work, do not run this section.

Within that separate approved budget:

- Test the five [canonical definitions](../../.antiburn/checks/) one at a time in
  the test repository. Enable an inventory definition only after approving its
  current global and repository inputs as described above. Allow enough admitted
  results for each authored window minimum.
  Record which definitions actually reach assessment; a missing mode hint or
  inventory must not become invented evidence.
- Review exact input and fitting omissions for each decision. Confirm that
  current instruction and skill snapshots describe collection time, not past
  activation or historical skill use.
- Check fast-mode advice against the recorded source qualifier. Requested-tier
  preferences and model-name hints do not prove served tier, speed, or savings.
- In **Settings → Checks**, save and select each approved connection. Editing
  or testing a draft must not select it. **Test connection**
  itself sends synthetic input and can incur charges.
- Open checks through navigation and search. Confirm that source/setup errors
  give a useful next action and that no action points to a retired Smart check.
- Confirm that a no-write assessment keeps the prior status-writing evidence.
  After a connection switch, accepted results and source progress must remain.
  Do not induce paid unknown-delivery failures as a retry experiment.
- For approved OpenCode V2 work, record a newly accepted assessment, then allow
  the ordinary stored metrics refresh. Verify that the same acceptance remains
  valid. A real correction to selected evidence can still invalidate it. An idle
  detach alone must not create a current capture alert, before or after its
  publication is consumed. Do not restore old invalid evidence for this check.

The [replay guide](../../apps/desktop/src-tauri/eval/replay/README.md) provides a
separate bounded synthetic replay. Its state must remain isolated from live
status. Record only what you observed. Remove only the temporary probe and
canonical test definitions that you added, then restore your original enablement
and pause settings. Turn off model-based checks when the planned observations
settle or the approved budget is reached.
Already-sent requests may still incur charges; disabling checks cannot withdraw
them from the provider.
