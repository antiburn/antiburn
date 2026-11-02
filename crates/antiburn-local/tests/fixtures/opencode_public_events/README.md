# OpenCode public-event fixtures

These fixtures use authored synthetic values. They contain no private captures.

`v2.jsonl` follows public producer pin
`843a6c514e606b436a7fe713422ace0a93ad3d61` (v2.0.32):

- `packages/schema/src/session-event.ts` defines inbox delivery, full calls,
  text ordinals, final text, provider-step completion, and execution settlement.
- `packages/core/src/session/message-updater.ts` resets a reused step ID and
  settles a message at `step.ended` or `step.failed`.
- `packages/core/src/session/{execution,run-coordinator}.ts` defines one execution
  busy period, which can contain several delivered prompts.

The sequence skips an internal event. A public sequence jump does not prove loss.
The second provider step has no public text. It still contributes attribution.
Reasoning occupies ordinal 1 and must not enter the public messages or cycle.

V1 tests create synthetic SQLite journals using the existing 1.18.35 pin
`53d1eabb61e21162157817bf677da0a4ad3332e3`. They require an observed assistant
header, full running input, final text-end time, and a later completed header.
They do not establish a V1 response-cycle boundary or a historical version range.
