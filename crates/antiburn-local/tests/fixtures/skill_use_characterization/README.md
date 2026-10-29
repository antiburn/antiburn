# Local skill-use characterization

These fixtures are synthetic. `skill_use_characterization.rs` runs each native
shape through the reader, private turn store, selected-content query, and shared
normalization. It tests `JevRecordedSkillResult` and its native proof bindings.
See the [retained native content contract](../../../../../docs/session-coverage.md#retained-native-content).
These tests characterize selected evidence, not model quality or full desktop
publication.

## Accepted shapes and sources

- `claude.jsonl` uses assistant `Skill` calls with `input.skill`, `id`, and native
  `uuid`. A user result joins by `tool_use_id`. The fixture includes the reported-launch
  shape is `toolUseResult: {success: true, commandName: name}` plus the exact
  `Launching skill: name` result. This shape is reported for Claude Code 2.1.122
  in [issue 54535](https://github.com/anthropics/claude-code/issues/54535).
  Claude's producer is private. This observed shape and synthetic fixture do not
  establish a historical version range. Shared normalization retains requested
  identity and unknown result status. The launch text does not prove that the
  body loaded or the task succeeded.
- `opencode.jsonl` contains a SQLite-native tool part. The request uses
  `state.input.name`; the completed result retains `state.metadata.name`, `dir`,
  and the full document wrapper. The producer is
  [`anomalyco/opencode@77239205`](https://github.com/anomalyco/opencode/blob/772392050500e0ddcd2ad2193411a22a3824372f/packages/opencode/src/tool/skill.ts).
  The test inserts it into the existing `session`/`message`/`part` schema.
  Exported `OpenCodeJsonl` stays outside this adapter's source gate.
- `codex.jsonl` contains a full user document, including its name,
  path, optional resource metadata, and nonempty body. The producer shape is
  [`openai/codex@e7637306`](https://github.com/openai/codex/blob/e7637306bc9246a3e42e407cb94f96b7ed345e3e/codex-rs/ext/skills/src/fragments.rs).
  The positive test adds the pinned 0.160.1 retained-message metadata and
  `content_item_kinds=[skills.selected_skill_instructions]` producer tag from
  `openai/codex@d27764b82f7118f674371e6d6e76271d9d606edb`.
  The unqualified fixture alone is a negative control. The adapter records
  `DocumentSelected`, not human approval or task success. It retains the path
  in private metadata with the exact source position. It does not
  infer producer origin from an ordinary user string or support implicit script
  execution and document reads.
- `pi.jsonl` uses the version 3 header, assistant `toolCall`, `id`/`parentId`,
  and `toolResult` envelope from the
  [pinned Pi session contract](https://github.com/badlogic/pi-mono/blob/b2602be77cb7b0de45dd616407fd210daa48aa75/packages/coding-agent/docs/session-format.md).
  An explicit `Skill`/`skill` request can identify a requested skill. No Pi
  extension-specific successful skill-result contract is claimed.
  Separate generated core V3 cases use the 0.84.4 producer pin
  `b79e4cc834970cca69daebffab7df1da7d1e52c4` and a complete native skill wrapper.
  They retain document selection at unknown authority and separate the exact
  trailing user argument range. Quoted, empty, and incomplete wrappers do not
  establish selection.

Claude 2.1.278 failed Skill results are characterized separately in
`claude_characterization/retained_native_results.jsonl`. They do not establish
successful document delivery or execution.

## Identity and completeness limits

Exact event references retain native call IDs, request/result references,
source/thread digests, roles, record positions, available timestamps, and the
publication fence. Native adapters validate and persist generic
`recorded_skill_result` facts in private operation metadata. Selection retains
only facts bound to the selected fields, exact ranges, digests, identities, and
publication fence. OpenCode result proof requires selected `OtherToolOutput`;
input-only projections cannot carry result text. The tests validate these facts
directly. Changed text, native positions, source formats, and selected ranges
cannot reuse stale proof.

Names recorded in these envelopes do not prove the identity or past contents of
a current skill file. Current-name and directory-alias matching stays inferred.
Command/path fallbacks retain inferred names; missing or conflicting fields stay
unknown. Mentions and listings do not establish document selection. Complete
means complete accepted records in the selected window. These facts do not
prove session-wide absence or equivalent-use absence.

Negative tests cover missing or conflicting result metadata, aliases, failures,
provider execution, compacted/truncated output, invalid wrappers, stale proof,
missing/reversed timestamps, and session/fence mismatches. No historical release
range is claimed for these shapes.
