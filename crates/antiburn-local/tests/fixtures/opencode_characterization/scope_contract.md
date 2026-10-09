# OpenCode scope fixture contract

All records are synthetic. The JSONL wrapper is Antiburn's message/part export,
not OpenCode's CLI export or event stream. Tests also load these payloads into
the accepted SQLite message and part tables. Row IDs remain outside stored JSON.

Producer pin: `anomalyco/opencode@772392050500e0ddcd2ad2193411a22a3824372f`.
This is one accepted producer shape, not a historical release range.

Pinned source paths under
`https://github.com/anomalyco/opencode/blob/772392050500e0ddcd2ad2193411a22a3824372f/`:

- `packages/opencode/src/tool/question.ts`: input, ordered metadata answers,
  and exact output rendering.
- `packages/schema/src/v1/question.ts`: prompts, options, descriptions,
  optional multiple selection, and string-array answers.
- `packages/opencode/src/question/index.ts`: submitted replies and dismissal error.
- `packages/opencode/src/session/processor.ts`: completed/error tool states and
  interrupted cleanup (`Tool execution aborted`, `metadata.interrupted: true`),
  and part-level `metadata.providerExecuted` for provider-executed tool results.
- `packages/opencode/src/tool/tool.ts`: `Tool.init` wraps execution and adds
  `truncated: false` to short results before the processor stores them.
- `packages/schema/src/v1/session.ts`: tool and synthetic text part schemas.
- `packages/core/src/session/sql.ts`: V1 message/part tables omit native IDs
  from JSON; IDs and session/message links come from their row columns.
- `packages/opencode/src/tool/plan.ts`: empty `plan_exit` input, completion title,
  output, and synthetic build-user message with a relative plan path. The tool
  returns empty metadata, but its wrapped persisted metadata is `{truncated:false}`.
- `packages/opencode/src/session/prompt.ts`: `createUserMessage` expands plain-text
  data attachments and MCP resources into arbitrary synthetic user text. These
  records can use the build agent and contain the exact plan approval string.

Structured answers preserve all strings, including delimiters and custom text.
Metadata must match the ordered native output. Text-only fallback accepts one
question with one delimiter-free answer. It does not guess multi-question or
comma-separated answers. Missing, interrupted, compacted, malformed, and
unmatched results do not establish submitted user responses. The dismissal error
means cancelled; it cannot distinguish explicit No from dismissal in `plan_exit`.
Part-level `metadata.providerExecuted: true` excludes recognized user workflow
authority. Matched provider answers retain their strings with unknown status,
origin, and provenance. Provider plan results also retain unknown status,
origin, and provenance, even when title, output, metadata, and timestamps match.

Local plan completion records establish only the recorded workflow transition.
Approval requires the persisted `{truncated:false}` shape; empty raw metadata,
truncated output, and other metadata shapes do not establish approval.

The synthetic text has no persisted native relation to the approving tool call.
Same-session location, message order, the build agent, and exact wording cannot
prove that relation. The parser preserves its path with synthetic origin,
unknown provenance, unknown status, and no call binding. It does not join that
path to a nearby approved `plan_exit`. The attachment fixture tests identical
text produced by both data attachments and MCP resources, with and without a
nearby local plan completion. Synthetic text is not ordinary user authority.

Neither record contains the approved file version. Plan contents, revision,
digest, and approved revision remain unavailable (`Unresolved`). The parser does
not read current mutable plan files or infer approval from assistant plans,
read/write tools, or arbitrary tool text.
