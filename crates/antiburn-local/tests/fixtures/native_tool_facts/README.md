# Native check facts

These fixtures contain synthetic records. They define a bounded accepted shape,
not a historical release range or live certification.

- Claude: local CLI 2.1.278 accepted log shape. The assistant's `message.model`,
  `message.usage.speed` (`fast` or `standard`), and record `cwd` bind to its exact
  direct-parent tool result. The private producer has no public schema pin.
  The log does not prove a provider route. A Claude model name does not supply one.
- Codex: 0.160.1, producer `d27764b82f7118f674371e6d6e76271d9d606edb`.
  `SessionMeta.model_provider` and `TurnContextItem.model` bind to a following
  call. Its exact call ID binds the result even after a later context change.
  Owned `ThreadSettingsSnapshot.service_tier` supplies a requested-preference hint
  through the next matching turn context. It is not the sent or served tier.
  Completed file changes use native `changes` keys and `move_path`,
  with the matching turn context's CWD. Shell text supplies no paths.
- OpenCode V1 journal: 1.18.35, producer
  `53d1eabb61e21162157817bf677da0a4ad3332e3`, schema
  `packages/schema/src/v1/session.ts` (`Assistant`). Requester attribution needs
  a retained `step-start` part with the exact assistant message and session IDs.
  `packages/opencode/src/session/processor.ts:424-432` writes that part from the
  model stream. Assistant ownership, tokens, and selected models do not supply it.
  Operation paths stay unavailable. Assistant `path.cwd`, `input.filePath`, and
  `step-start` do not identify the tool implementation. Same-name custom tools
  can replace `read`, `edit`, and `write`. Projection lookups still supply
  ownership only. They do not recover historical attribution.
- OpenCode V2: 2.0.32, producer `843a6c514e606b436a7fe713422ace0a93ad3d61`.
  `packages/schema/src/session-event.ts` (`Step.Started`) and
  `packages/core/src/session/runner/publish-llm-event.ts` bind `model.providerID`
  and `model.id` by `assistantMessageID`. The finite catalog mapping below records
  selected fast mode. `Model.Ref.variant` and display labels do not prove mode.
  Tool events have no exact operation CWD. Path facts remain unavailable.
- Pi: core V3 at 0.84.4, producer `b79e4cc834970cca69daebffab7df1da7d1e52c4`.
  `packages/ai/src/types.ts` (`AssistantMessage`) records the actual requesting
  provider and model. Native tool ID/name/direct-parent links preserve them.
  Operation paths stay unavailable for all Pi calls. The retained V3 records
  cannot distinguish local core tools from same-name remote overrides.
  `model_change` and thinking settings do not replace request attribution.
  A bounded recorded assistant model-ID suffix supplies the name hint below.

## Four-agent mode meanings

`NativeRequestAttribution` keeps its existing provider, model, and optional
`fast_mode` fields. The immutable source format identifies the meaning:

| Source        | Mode evidence                      | True / false / unknown                                                  |
| ------------- | ---------------------------------- | ----------------------------------------------------------------------- |
| Claude CLI    | Reported `message.usage.speed`     | `fast` / `standard` / other or absent                                   |
| OpenCode V2   | Exact selected catalog ID          | The 10 reviewed Fast/Ultrafast IDs / no false mapping / other or absent |
| Codex 0.160.1 | Owned requested-tier preference    | `priority` / `default` / other or absent                                |
| Pi V3         | Recorded model-ID suffix heuristic | Matching terminal token / no false mapping / other or absent            |

These meanings do not prove route availability, actual served tier, cost, or
savings. Missing evidence is unknown, not an explicit standard observation.

### Codex requested preference

`codex.jsonl` uses the real `event_msg.payload.type: thread_settings_applied`
shape with `thread_id` and `thread_settings.{model,model_provider_id,service_tier}`.
The pinned producer defines these fields in
`codex-rs/protocol/src/protocol.rs:2199-2214` and writes the owning thread in
`codex-rs/core/src/session/thread_settings.rs:113-129`. `priority` and `default`
follow `codex-rs/protocol/src/config_types.rs:528-552`. The `fast` request alias
is not a producer-positive checkpoint tier. Tests retain the unsupported
`turn_context.service_tier` only as an ignored-field negative case.

The parser replaces one bounded snapshot per owned settings event. Missing,
null, malformed, `auto`, `flex`, and unknown tiers clear the prior mode hint.
Foreign, inherited, or ownerless snapshots cannot supply it. At turn context,
the snapshot model/provider must match the request attribution. A settings event
is not a request. Captured call IDs keep their facts after later selections.
Metric speed state remains separate and keeps its existing carry rules.

`codex_mode_transitions.jsonl` exercises default/priority/default contexts with
delayed results. `codex_requested_mode_completed.jsonl` covers completed-only
file changes. `codex_mode_conflicting_context.jsonl` replaces the same turn's
context; completed-only mode stays unknown. Exact duplicate contexts retain the
hint. Conflicts stay unknown after a later restoration. The bounded fingerprint
history survives snapshots and live cursor splits. This subset does not use a
retained legacy call to repair a conflicting completed-only binding.

### Pi model-ID hint

`pi_mode_hint.jsonl` uses a synthetic custom model ID, `example-fast`, in the
assistant message that records the exact tool call. This is a heuristic/custom
alias case, not a verified built-in Pi Fast/Ultrafast alias. The pinned producer
stores the assistant `model` in `packages/ai/src/types.ts:427-446`; custom model
construction preserves IDs separately from display names in
`packages/coding-agent/src/core/provider-composer.ts:130-165`.

The classifier accepts a terminal ASCII case-insensitive `fast` or `ultrafast`
token after `-`, `_`, or one ASCII space, with a nonempty prefix. It does not
trim or search for interior tokens. Raw IDs must be at most 4 KiB and contain
no control characters. Matches yield true; nonmatches remain unknown.
`Example Ultrafast` supplies this name hint but cannot become a normalized
provider/model key because that key rejects whitespace.

`model_change`, display names, `responseModel`, thinking settings, result model
fields, and output text cannot supply or replace the hint. Pi has no accepted
persisted tier field in this shape. Existing V3 identity, ancestry, call-name,
and direct-parent result gates remain. Tests cover every live split and stream
snapshot, later model changes, invalid/oversized IDs, and broken result joins.

## Producer characterization cases

`opencode_v1.jsonl` includes a model-stream `step-start`. The tests remove,
misbind, replace, delay, and delete that proof before the terminal tool record.
They check replay, cold live attachment, every live cursor split, and eviction.
`opencode_v1_user_shell.jsonl` follows `shellImpl` in the same pinned
`packages/opencode/src/session/prompt.ts:451-518`.
`opencode_v1_direct_subtask.jsonl` follows `handleSubtask` at lines 263-300.
Both have assistant ownership and a selected or worker model without a model
request. Requester facts stay absent even when tests add nonzero token counts.

`opencode_v1_custom_override.jsonl` models a custom tool registered as `read`
with a `filePath` argument. The same pinned producer accepts custom IDs in
`packages/opencode/src/tool/registry.ts:125-180,188-203`, returns them after
built-ins at lines 256-258, and assigns each tool to `tools[item.id]` in
`packages/opencode/src/session/tools.ts:92-111`. A same-name callback can map
the argument to another directory or host. Its result metadata has no required
core-origin marker. The fixture follows the untruncated string-result shape
(`title: ""`, `metadata.truncated: false`); it does not run a plugin or remote call.
Tests cover `read`, `edit`, and `write`, normal and special-character inputs,
replay, cold live attachment, and every cursor split. Exact requester facts and
recorded-name categories remain available, but no V1 operation paths do.
The correction test also rejects path facts retained in a pre-fix live cursor.

`pi_ssh_override.jsonl` models a write through the pinned
`packages/coding-agent/examples/extensions/ssh.ts:47-78,128-168`. That extension
keeps core tool names and arguments but translates local-looking paths to a
remote CWD. No transcript field identifies that execution choice. The fixture
does not claim that the test ran SSH. The tests also cover core-shaped calls;
neither shape proves local paths, but both preserve exact requester attribution.

Pi tests cover `@src/api.rs`, `@/outside/file.rs`, and Unicode-space paths for
`read`, `edit`, and `write`. The pin's `src/core/tools/path-utils.ts:40-49` and
`src/utils/paths.ts:75-105` strip `@` and normalize spaces before resolution.
Read tools can also try filesystem-dependent filename variants. Pi paths stay
unavailable rather than asserting a target from raw arguments. Claude tests
preserve literal `@` and Unicode spaces under the unchanged shared POSIX rules.
V1 tests preserve those raw arguments without asserting local targets.

## OpenCode V2 recorded fast mode

`opencode_v2_fast.jsonl` binds a selected catalog ID to its exact step and call.
The reviewed catalog is `packages/core/src/models-dev/snapshot.txt` at producer
`843a6c514e606b436a7fe713422ace0a93ad3d61`. Its SHA-256 is
`69c8fa4b9a563aeadc381bb11c59d3e62d286fea8bdd6c87dde6ada5681c7715`.
`packages/core/src/models-dev.ts:111-126` creates `${model.id}-${mode}` from
`experimental.modes`. The accepted subset uses exact provider ID `openai`:

| Exact catalog ID        | Catalog request tier |
| ----------------------- | -------------------- |
| `gpt-5.5-fast`          | `priority`           |
| `gpt-5.6-luna-fast`     | `priority`           |
| `gpt-5.6-sol-fast`      | `priority`           |
| `gpt-5.6-terra-fast`    | `priority`           |
| `gpt-6-astra-fast`      | `priority`           |
| `gpt-6-astra-ultrafast` | `ultrafast`          |
| `gpt-6-luna-fast`       | `priority`           |
| `gpt-6-sol-fast`        | `priority`           |
| `gpt-6.1-sol-fast`      | `priority`           |
| `gpt-6.1-sol-ultrafast` | `ultrafast`          |

These IDs set `requester.fast_mode` to `Some(true)`. They record the selected
choice, not actual served tier, endpoint, cost, or savings. Custom configuration
can override catalog behavior. Base IDs, other providers, arbitrary variants,
display labels, and unknown suffixes stay `None`, not `Some(false)`. Tests cover
every positive ID with and without captured user text, wrong joins, and later
model selections that must not replace the call's frozen attribution.

## Categories and paths

Tool categories use exact source-specific identities. Claude's documented
`mcp__server__tool` identity supplies `mcp`. An unknown named tool is `other`;
an unjoined tool has no check category. OpenCode provider-executed tools do
not inherit local tool classes. V2 needs explicit local execution proof before
it emits a local category. V1 and Pi categories describe recorded tool names,
not core implementation or local execution. Unrecognized extension names do not
establish a core delegate, skill, or MCP identity.

Path normalization uses recorded POSIX paths only. It rejects parent traversal,
Windows paths, URI paths, and incomplete target lists. It never reads current
files, resolves symlinks, or parses shell or patch text. Known outside-repository
targets produce an empty repository-relative list. An operation on the repository
root itself stays unavailable because this path subset has no root-path value.
