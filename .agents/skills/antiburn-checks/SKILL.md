---
name: antiburn-checks
description: >-
  Walk a user through writing, reviewing, or fixing Antiburn Burn Check .yml
  and .yaml definitions. Use for trigger and evidence selection, local status
  rules, typed model questions, input fitting, validation, and safe installation.
---

# Author Burn Checks

Write, review, or fix one check for a specific recorded behavior. For a review,
report findings without changing the file. For a fix, preserve behavior outside
the requested change. For a new check, deliver a draft file. Report the evidence
limits and validation that actually ran. Do not change Antiburn's engine to make
an unsupported definition work.

This skill targets the experimental `v1-alpha` draft. Syntax, semantics, and
capabilities can break without backward compatibility. Unsupported future work
is unassigned; do not promise a beta, stable release, or delivery date.

## 1. Verify the contract before drafting

Read the bundled [source map and limits](references/sources-and-limits.md).
It contains full upstream URLs because installing this skill does not copy the
Antiburn repository or its documentation.

- If an Antiburn checkout is available, use its `docs/burn-check/specification.md`,
  `authoring.md`, schema, and coverage documents from the same checkout. Follow
  its repository instructions. Record the revision and any relevant local edits.
- Otherwise, resolve the chosen published Git ref to one commit. Fetch all needed
  documents, schema, examples, and validation files at that commit. The source
  map links to `main` for discovery; replace `main` with that commit before use.
  Do not mix files from moving branch reads or different releases.
- Confirm that the specification and authoring guide both identify `v1-alpha`
  and that the schema belongs to that same revision. If the versions disagree,
  stop and ask for the intended release or local checkout. Do not combine a
  different schema with this skill's contract.
- If the required sources cannot be read, explain the gap and stop before
  drafting version-dependent YAML. The summary and quickstart below do not
  replace the source check.

Read the supported subset as well as the schema. Schema acceptance alone does
not prove compiler support, native source proof, or provider compatibility.

## 2. Agree on the behavior and result policy

Ask only for missing information, in one short batch:

1. What recorded behavior should cause failure? What is the smallest useful
   claim: one message, selected results, a proven response cycle, or an observation?
2. Which agent, source format, and accepted producer version supply the evidence?
   Ask for a small synthetic example if the recorded facts are unclear. Do not
   collect a private transcript or credential.
3. What similar work is allowed? Include an example that must not fail.
4. If a valid answer is uncertain, should an action write `success`, or should no
   action match? Explain that no matching action preserves the saved status and
   can leave an earlier failure visible. Recommend no matching action when missing
   evidence could hide allowed work, but get agreement before choosing the policy.

Define a failure case, an allowed case, and an uncertain case before writing
questions or conditions. Missing evidence is not proof that work did not happen.
An unavailable collection is not a known empty collection.

Choose a draft path outside active check directories. Ask about repository or
global scope before installation, not as a reason to install automatically.

## 3. Select a trigger that the source can prove

Choose exactly one event family:

| Family              | Events                                                                 | Evidence root                                   |
| ------------------- | ---------------------------------------------------------------------- | ----------------------------------------------- |
| Message             | `user_message`, public `assistant_message`, `tool_call`, `tool_result` | `message`; optional `window.messages`           |
| Response cycle      | `turn_end`                                                             | `turn` with a proven native completion boundary |
| Session observation | `session_start`, `session_idle`                                        | `session` observation fields                    |

Check source coverage for the exact event, finality, ordering, paths, and model
facts the definition needs. A transcript row or inactivity does not prove a
completed response cycle. Private reasoning and streaming fragments are not
public assistant messages.

Message alternatives share one filtered rolling window. Only message checks
support `on.window`; use an integer or `{size, min}`, with `min <= size` and
size at most 40. A window bounds selected messages, not request history or
inventory size. Local cooldown defaults to `0s`; model cooldown defaults to
`60s`. Zero cooldown still uses single-flight execution.

Keep these claims narrow:

- Tool result status describes tool execution, not whether a program or test
  succeeded. Use recorded outcome facts only where the source supplies them.
- Model/mode filters read the selected event's scope. Current session selection
  cannot fill past attribution. Missing mode matches neither true nor false.
- Current instructions do not prove past activation. Skill descriptions support
  next-step advice, not proof that a skill was loaded or never used.
- Request history through the trigger does not put every request before every
  earlier selected action. Keep approval order separate from tool claims.

## 4. Prefer local rules when recorded facts are enough

Omit `decisions` for local evaluation. Do not use an empty or null `decisions`
field. This small teaching example is not a canonical repository definition:

```yaml
name: Recorded read cancellation
on:
  tool_result:
    tools: [read]
actions:
  - type: status
    if: message.tool.result.status == 'cancelled'
    input:
      result: failure
      message: The selected read operation was cancelled.
  - type: status
    if: message.tool.result.status == 'completed'
    input:
      result: success
```

Failure means only that a selected read was cancelled. It does not prove waste
or fault. A completed read writes success for that assessment, not proof that an
earlier operation was repaired. An `error` or `unknown` result writes nothing and
leaves any saved status unchanged. Confirm that this policy fits the user's goal
before adapting it.

Local checks send no model request and create no provider attempt or usage
record. They still require **Run checks**, a configured selected connection, its
credential/authentication rules, and repository, source, generation, and shutdown
gates. Do not bypass those gates to test a local check.

## 5. Use model questions only for judgments

Read the closest canonical example from the source map. Do not copy every field
by default. Select only the evidence needed for the agreed claim, with its
completeness and source-quality facts. Explain that selected messages, arguments,
results, paths, and inventories can contain private data and can leave the
machine in model requests. Review that exposure before enabling a model check.

- Use named independent decision blocks. Each has nonempty `input` and `output`
  and one to eight questions. Blocks cannot read each other's decisions.
- Use Choice for a declared label, Score for ordered levels, or Noul for a
  yes-probability. Include an uncertain Choice label when evidence can be unclear.
  The reference lists the exact scalar and detail types.
- State the behavior, allowed cases, scope, and missing-evidence limits in the
  question. Ask for a typed judgment, not private thinking or generated reasoning.
- Settings selects the decision connection and model. Do not add YAML `version`,
  `$schema`, or decision-level `model` fields. A coding-agent model filter does
  not select the assessment model.
- Verify the selected connection's question types, criterion limits, and every
  output detail the actions read. Label probability and provider confidence are
  different. Do not substitute one for the other, invent missing probabilities,
  or lower a gate just to fit a provider.

Use only `type: status` actions in the supported runtime. A result is literal
`success` or `failure`, or a whole expression that returns exactly one of those
strings. Conditions are independent; the last valid applied write wins. Quote
negated conditions so YAML does not treat `!` as a tag.

An uncertain label has no built-in behavior. Write the agreed policy explicitly.
No matching action preserves the earlier status, message, evidence, and write
time. Missing required evidence or execution errors do not write success.

### Authorize input loss explicitly

Omitting `fit` is valid. Add it only when the user accepts the loss and the
question can still make a useful bounded judgment.

`recent` removes oldest array items; `truncate` shortens string middle text.
Paths address evaluated decision input names, with `.field` and `[*]`, not raw
context roots. Rules cannot overlap. Preserve evidence needed for the claim.
Never silently drop or truncate instructions, requests, approvals, or arguments
to make a request fit. Do not add broad loss permissions as a fallback.

The model must account for omissions and choose uncertainty when retained
evidence is insufficient. A no-fit or preparation failure is not a passing
assessment. Local checks have no decision input or fitting step.

## 6. Validate without activating the check

In a verified Antiburn checkout with validator dependencies already available,
run this from its root, using the draft file's absolute path:

```sh
node docs/burn-check/scripts/validate-checks.mjs /absolute/path/to/check.yml
```

This command checks restricted YAML 1.2 and JSON Schema structure only. It does
not activate checks or call a provider. Use a schema modeline or editor mapping
that points to the verified schema; do not add a `$schema` instance field.

Outside a checkout, fetch the validator script, schema, `package.json`, and
`package-lock.json` at the same verified commit into a user-approved workspace.
Keep their repository-relative layout and pass an explicit absolute check path;
the validator's default canonical examples are not bundled with this skill.
Review downloaded code and dependency files. Ask before installing dependencies
or executing downloaded code. Do not use `curl | sh`, install an unrelated
validator, or treat a generic YAML parser as the runtime compiler.

Keep validation layers separate:

1. Validate restricted YAML and schema structure.
2. Use the matching checkout's compiler and synthetic behavior tests to check
   expressions, phases, references, window relationships, fit rules, and actions.
   If no compiler test path is available, report that layer as unverified rather
   than claiming runtime validity. Do not activate the check to hide this gap.
3. Test failure, allowed, and uncertain cases. Also test missing evidence, a
   nonmatching event, and any recovery case that should clear a prior failure.
   Check that error/unknown input does not accidentally become success.
4. Review invalid fields and unsupported actions, including behind `if: false`.
   Test duplicate filename extensions and invalid overrides when installation
   behavior matters. Do not change production gates to make a fixture pass.

Live source testing, developer replay, and **Test connection** are separate
operations. Ask before provider calls, live execution, or work that can incur
charges. A synthetic pass does not prove live source support or model accuracy.

## 7. Install or remove only when asked

Antiburn discovers immediate `.yml` or `.yaml` files in a known native
repository's `.antiburn/checks/` or in `~/.antiburn/checks/`. These directories
belong to the user; neither this skill nor the app should create them or copy a
draft there without the user's request.

The filename stem is the logical ID and must match `[a-z][a-z0-9-]*`; `name` is
display text. A repository file replaces the global file with the same stem.
Files never merge. Duplicate `.yml`/`.yaml` stems in one scope invalidate that
ID. An invalid or unreadable override does not fall back to the global file.

Installing a definition does not authorize enabling checks, switching a
connection, changing credentials, or bypassing source eligibility. Explain the
selected connection and **Run checks** requirements for local and model checks.

For requested removal, remove only the exact authorized definition. Removing
an override can expose a global definition. Removing all effective definitions
stops the check; it does not write success or erase saved assessments. Include
this case in the user's test or removal plan.

Finish with the file path and the task result: a new draft, review findings, or
a summary of the requested fixes. Include the bounded claim and limits, the
source revision, validation results by layer, and whether installation or
live/provider testing actually occurred. Do not promise native fix prompts,
Auto Fix, verification watches, burn estimates, or savings from a YAML status check.
