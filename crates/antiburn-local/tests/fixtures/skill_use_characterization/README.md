# Local skill-use characterization

These fixtures are synthetic. `skill_use_characterization.rs` runs each native
shape through the existing reader, private turn store, selected-content query,
and check-owned `SkillUseSnapshot` adapter. The adapter performs no I/O. The
production Skill Opportunities worker consumes these normalized facts under the
[retained-root admission contract](../../../../../docs/session-coverage.md#retained-root-smart-check-inputs).
These tests characterize selected evidence, not model quality or full desktop
publication.

## Accepted shapes and sources

- `claude.jsonl` uses assistant `Skill` calls with `input.skill`, `id`, and native
  `uuid`. A user result joins by `tool_use_id`. The conditional reported-launch
  shape is `toolUseResult: {success: true, commandName: name}` plus the exact
  `Launching skill: name` result. This shape is reported for Claude Code 2.1.122
  in [issue 54535](https://github.com/anthropics/claude-code/issues/54535).
  Claude's producer is private. This observed shape and synthetic fixture do not
  establish a historical version range. A reported launch does not prove that
  the body loaded or the task succeeded. Forked results are not accepted.
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
  `DocumentSelected`, not human approval or task success. It hashes the path
  and retains the source position when no native record ID exists. It does not
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
publication fence. Result joins require one request and one result in the same
source, thread, and turn scope. Native adapters validate and persist generic
`recorded_skill_result` facts in private operation metadata. Selection retains
only facts bound to the selected fields, exact ranges, digests, identities, and
publication fence. OpenCode result proof requires selected `OtherToolOutput`;
input-only projections cannot carry result text. `SkillUseSnapshot` consumes
validated facts without parsing native markup or tool names. Optional native
supplements must match the selected reference, session, and fence; they cannot
repair missing or invalid persisted proof.

Names recorded in these envelopes do not prove the identity or past contents of
a current skill file. Current-name and directory-alias matching stays inferred.
Command/path fallbacks retain inferred names; missing or conflicting fields stay
unknown. Mentions and listings are not use. Complete means complete accepted
records in the selected window. No snapshot proves session-wide absence or
equivalent-use absence. Aggregates retain requests only, with inferred identity,
unknown time/order, and no absence proof.

Negative tests cover missing or conflicting result metadata, aliases, failures,
duplicate call IDs, provider execution, compacted/truncated output, source and
thread mismatches, missing/reversed timestamps, session/fence mismatches, and
bounded inputs. No historical release range is claimed for these shapes.
