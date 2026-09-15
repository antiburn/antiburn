# Burn Check Parity and Auto Fix Plan

Status (2026-09-15): Phases 0 through 7 are complete. Native Linux, Windows,
and WSL validation remains a release gate because this review ran on macOS.

This plan expands passive Burn Check evidence and safe config Auto Fix coverage.
It covers every current `SourceFormat` separately. It does not treat agent-level
support as proof that every surface for that agent has the same coverage.

[`docs/check-coverage.md`](../check-coverage.md) remains the current check
coverage baseline. [`docs/session-coverage.md`](../session-coverage.md) remains
the current discovery and parsing baseline. Update both documents with each
implemented source-contract change. Do not update them from this plan alone.

## Goal

- Reach the strongest passive Burn Check coverage that each first-tier source
  can prove for Claude Code, Codex, OpenCode, Pi, Cursor, and Antigravity.
- Add typed Auto Fix operations for D, S, M, B, K, F, and C only when a
  finding has a supported current project or global config target.
- Assess Copilot, Cline, Kiro, Amp, and Windsurf without presenting planned or
  unimplemented evidence as current support.
- Keep findings and clean results fail-closed when source shape, completeness,
  provider route, or config precedence is not proved.

## Check Key

| Code | Check                 |
| ---- | --------------------- |
| D    | Session overdepth     |
| T    | Model overthinking    |
| S    | Overpowered subagents |
| M    | Unused MCP servers    |
| B    | Unused built-in tools |
| K    | Unused skills         |
| O    | Old model usage       |
| F    | Fast mode overuse     |
| C    | Cache churn           |

## Locked Boundaries

- Read normal persisted stores only.
- Do not install hooks, plugins, extensions, commands, or subscriptions to
  collect future evidence.
- Do not launch an agent or its CLI to create, export, repair, or enrich
  evidence.
- Do not infer one surface's coverage from another surface for the same agent.
- A partial source can support a positive finding when every required positive
  fact is present. It cannot support a clean result.
- M, B, and K stay finding-only until the source proves the complete effective
  inventory for the assessed interval.
- Auto Fix prefers publication-time attribution when available. Otherwise it
  inspects the supported current project and global config targets and reads
  them again before applying the edit.
- Runtime flags, environment overrides, managed settings, and remote config
  add a warning when the local edit may not change current behavior.
- Do not mutate private provider databases or agent session stores.
- Preserve unknown config keys, comments, ordering where supported, file mode,
  and unrelated content.
- Native Windows remains read-only for remediation until its atomic write and
  recovery contract is separately approved. WSL remains a separate environment.
- A detector can have prompt remediation without having Auto Fix.

## Source Contract Baseline

The following table defines the starting implementation state. `Finding` lists
checks that can currently produce a positive finding. `Clean` lists checks that
can currently report a clean result when all other evidence gates pass.

| `SourceFormat`                 | Current finding              | Current clean | Planned disposition                                                    |
| ------------------------------ | ---------------------------- | ------------- | ---------------------------------------------------------------------- |
| `ClaudeJsonl`                  | D,T,S,M,B,K,O,F,C            | D,T,S,O,F,C   | Retain coverage; pin current transcript and sidecar shapes             |
| `CodexRolloutJsonl`            | D,T,S,M,B,K,O,F,C            | D,T,S,O,F,C   | Retain coverage; update current rollout and config contracts           |
| `OpenCodeJsonl`                | D,S,K,O,C                    | D,S,O,C       | Characterize legacy export separately from V2                          |
| `OpenCodeSqliteV2`             | D,S,K,O,C                    | D,S,O,C       | Add only facts proved by durable V2 rows                               |
| `PiV3Jsonl`                    | D,T,S,O,C                    | D,T,O,C       | Enforce the V3 claim and update the current V3 contract                |
| `CursorJsonl`                  | O                            | None          | Keep compatibility input finding-only                                  |
| `CursorCliAgentJsonl`          | O                            | None          | Characterize the exact CLI transcript shape                            |
| `CursorCliStoreDb`             | O                            | None          | Treat the private blob schema as version-pinned only                   |
| `CursorChatStoreDb`            | O                            | None          | Keep chat storage separate from legacy CLI storage                     |
| `CursorIdeComposer`            | O                            | None          | Keep separate from CLI stores and fail closed on drift                 |
| `CursorLegacyChatJson`         | None                         | None          | Keep fail-closed unless a bounded contract is pinned                   |
| `AntigravityJson`              | D,O when supplied internally | None          | Remove or document the non-emitted compatibility profile               |
| `AntigravityBrainJsonl`        | D,O                          | None          | Keep finding-only and version-pinned                                   |
| `AntigravityCascadeJson`       | D,O                          | None          | Keep finding-only and distinguish API or mirror provenance             |
| `AntigravityWorkspaceChatJson` | None                         | None          | Keep fail-closed                                                       |
| `AntigravitySqlite`            | D,O                          | None          | Decode only pinned fields; never claim full protobuf support           |
| `CopilotCliJsonl`              | None                         | None          | Add a dedicated reader after persisted-event characterization          |
| `CopilotIdeChatJson`           | None                         | None          | Keep separate from the CLI event contract                              |
| `ClineSessionJson`             | None                         | None          | Split metadata, legacy messages, and messages-contract-v1              |
| `ClineMessagesContractV1`      | S,O                          | None          | Retain terminal v1 bundle only; reject malformed or changing artifacts |
| `KiroSessionJson`              | None                         | None          | Keep IDE workspace JSON fail-closed                                    |
| `KiroChat`                     | None                         | None          | Keep fail-closed legacy fallback                                       |
| `KiroCliV2Bundle`              | None                         | None          | Retain only safe V1 bundle facts; all checks Unknown                   |
| `KiroCliV3Bundle`              | None                         | None          | Separate directory discovery; fail closed pending schema               |
| `KiroChatSaveExport`           | None                         | None          | Manual export shape is public-command-only; do not scan                |
| `AmpThreadJson`                | None                         | None          | Keep fail-closed until a persisted thread contract is pinned           |
| `AmpFileChanges`               | None                         | None          | Keep classified as not a session                                       |
| `WindsurfWorkspaceJson`        | None                         | None          | Keep fail-closed                                                       |
| `WindsurfMirrorJson`           | None                         | None          | Keep fail-closed unless the mirror producer is pinned                  |
| `WindsurfCascadeProtobuf`      | None                         | None          | Do not add decryption or unstable private protobuf support             |
| `Uncharacterized`              | None                         | None          | Keep generic fallback fail-closed                                      |

Adding a materially different persisted shape requires a new `SourceFormat`.
Do not overload an existing format to avoid updating the inventories.

## Phase 0: Correct the Baseline Contracts

Purpose: make the coverage documents and tests describe implemented behavior
before adding new support.

### Checklist

- [x] Update the audit dates after the review is complete.
- [x] Update parser, analyzer, evidence, coverage, and resume revisions in
      `docs/session-coverage.md` from the code constants.
- [x] Change the `Partial` definition so it means implemented finding support,
      not an unimplemented evidence path.
- [x] Change unimplemented Copilot, Cline, Kiro, Amp, and Windsurf cells to
      `Unknown` or `Unsupported` as source research requires.
- [x] Split discovery support, session parsing support, and Burn Check support
      in `docs/support.md`.
- [x] Apply the same distinction in
      `apps/desktop/src/lib/presentation/agents.ts`.
- [x] Decide whether OpenCode WSL CLI execution remains supported discovery.
- [x] If it remains, document that it is not a disk-only passive path.
- [x] OpenCode WSL CLI discovery remains supported, so no replacement source is
      needed.
- [x] Update stale Codex and Pi fixture READMEs.
- [x] Replace source-format tests that apply Claude capabilities to every
      `SourceFormat`.
- [x] Make `check_coverage_contract` parse the Markdown inventories and matrix.
- [x] Assert that every current `SourceFormat` appears exactly once in each
      required inventory or matrix.
- [x] Assert that every matrix cell uses the documented status vocabulary.
- [x] Add a test that the public support table cannot mark a fail-closed reader
      as Burn Check supported.

### Exit Criteria

- [x] Documentation and code report the same current support.
- [x] A new enum variant fails the coverage contract until both coverage
      documents classify it.
- [x] No generic or passive reader can produce a clean result in tests.

## Phase 1: Make Source Admission Exact

Purpose: bind every characterized source to a version, schema, header, or pinned
producer fixture before detector expansion.

### Shared Checklist

- [x] Record source provenance and surface identity during discovery.
- [x] Pass explicit discovery metadata to readers instead of reclassifying by
      path substring when practical.
- [x] Keep file, SQLite, inline export, and companion boundaries distinct.
- [x] Define the accepted header or root record for each characterized format.
- [x] Reject or downgrade missing, malformed, unsupported, or conflicting
      version markers.
- [x] Preserve unknown records for diagnostics without retaining source content.
- [x] Lower only the evidence groups that an unknown record can affect.
- [x] Reject oversized, truncated, changed, or incomplete database snapshots.
- [x] Include WAL state in live SQLite fingerprints where uncheckpointed rows
      can change evidence.
- [x] Prove full-read and resumed-read equivalence for resumable formats.
- [x] Bump the required parser or evidence revisions after accepted shapes
      change.

### Pi Admission Checklist

- [x] Require a valid `type: "session"` header with `version: 3` before assigning
      `PiV3Jsonl`.
- [x] Route headerless, V1, V2, and unknown-version sources to a separate
      uncharacterized format or reject them.
- [x] Replace the test that currently accepts headerless Pi as complete.
- [x] Add file-backed tests for V3, missing header, unsupported version,
      malformed header, duplicate IDs, and changed active branch.

### Exit Criteria

- [x] `SourceFormat` means one bounded source contract, not only one filename.
- [x] Every clean-capable format has fixture-backed admission and loss tests.
- [x] Pi V3 coverage cannot be reached without a valid V3 header.

## Phase 2: Refresh First-Tier Evidence Readers

Purpose: close source-backed detection gaps without weakening completeness.

### Claude Code

- [x] Pin accepted main transcript records to reviewed Claude Code versions.
- [x] Pin child transcript and `.meta.json` sidecar shapes separately.
- [x] Prefer `toolUseId` joins for parent-to-child attribution.
- [x] Degrade S evidence when the sidecar, worker model, or parent join is absent.
- [x] Characterize current `service_tier`, effort, compaction boundary, skill,
      MCP, and built-in tool records.
- [x] Keep all clean results disabled for unreviewed versions or unknown
      evidence-bearing record changes.
- [x] Keep M, B, and K finding-only until complete historical inventories are
      proved.

### Codex

- [x] Pin current `session_meta`, `turn_context`, `event_msg`, `response_item`,
      ordinal, compaction, and child rollout shapes from upstream source.
- [x] Support current `parent_thread_id`, `thread_source`, `agent_role`, and
      agent-path metadata.
- [x] Retain per-turn model and reasoning effort.
- [x] Retain service tier when it is present in persisted request evidence.
- [x] Retain effective tool ownership for harness and MCP functions when the
      rollout contains the inventory.
- [x] Preserve exact-copy usage deduplication and linear-order cache pairing.
- [x] Add fixtures for profile, nested project config, named agents, skills,
      MCP tools, fast service tier, and automatic compaction.

### OpenCode

- [x] Keep legacy JSONL export and SQLite V2 tests independent.
- [x] Read selected model variant on each applicable user and assistant turn.
- [x] Resolve reasoning only when the selected variant and model catalog prove
      its reasoning policy.
- [x] Resolve fast mode only when the selected variant proves a speed or service
      tier policy. Do not infer it from the word `fast` alone.
- [x] Read effective enabled-tool records when persisted for the turn.
- [x] Classify MCP and built-in tools only from exact normalized ownership.
- [x] Emit M or B resource evidence only when the record proves the applicable
      inventory and scope.
- [x] Keep current D, S, K, O, and C clean gates unchanged until new completeness
      tests pass.

### Pi

- [x] Read current V3 `responseModel`, `providerThinkingLevel`, diagnostics,
      cache buckets, and compaction usage without double counting.
- [x] Reject a branched `id` and `parentId` tree when no durable leaf identifies the active branch.
- [x] Handle retained compaction tails and legacy `firstKeptEntryId` separately.
- [x] Keep extension-generated custom records partial unless their producer is
      pinned.
- [x] Keep S positive-only for the reviewed delegation extension shape.
- [x] Do not infer skill invocation from a file read.
- [x] Do not infer MCP support from an extension tool name.

### Cursor

- [x] Characterize `CursorCliAgentJsonl` independently from `CursorCliStoreDb`.
- [x] Add a separate format for chat storage if its contract differs from the
      current CLI store.
- [x] Pin exact Cursor versions for every private transcript or blob fixture.
- [x] Treat model fallbacks as unknown effective models unless the source records
      the model actually used.
- [x] Keep Cursor IDE and CLI precedence, config, and session contracts separate.
- [x] Add positive findings beyond O only after the source proves every required
      detector fact.
- [x] Keep all Cursor clean results disabled without a complete persisted
      source contract.

### Antigravity

- [x] Separate CLI SQLite, brain JSONL, mirror/API JSON, and IDE workspace data.
- [x] Pin the SQLite `user_version` and the decoded protobuf field subset.
- [x] Read only reviewed model, usage, context, identity, and parent fields.
- [x] Degrade on unknown step types that can affect the relevant detector.
- [x] Do not treat brain transcripts as the authoritative resumable store.
- [x] Keep all Antigravity clean results disabled while its full wire contract
      remains unpublished.

### Exit Criteria

- [x] Each first-tier finding cites one accepted source shape and one tested
      evidence path.
- [x] Unknown versions and missing companions cannot produce clean results.
- [x] No detector infers effort, speed, provider route, tool ownership, or
      subagent model from an ambiguous label.

## Phase 3: Effective Config And Batch Editor

Purpose: store preferred config attribution and safely batch the currently
supported model and reasoning edits. This phase resolves only public, file-backed
targets. It does not add detector Auto Fix operations for resources.

### Shared Checklist

- [x] Extend `ConfigSetting` beyond `Model` and `Reasoning` with explicit
      variants for compaction, subagent model, MCP server, built-in tool, skill,
      and fast mode.
- [x] Replace string-only operation values with typed scalar, boolean, list,
      map-entry, and deletion operations.
- [x] Add physical selectors for JSON, JSONC, TOML, and Markdown frontmatter.
- [x] Store optional physical path, scope, selector, expected typed value, effective
      precedence hash, and optional named resource at publication.
- [x] Resolve every supported config layer in the same order as the agent.
- [x] Support nested project layers where upstream loads them.
- [x] Support separate profile files where upstream loads them.
- [x] Keep attribution unavailable when runtime, environment, managed, remote,
      organization, or server controls prevent one exact effective selector.
- [x] Reject symlinks, unsafe roots, unsupported owners, malformed data,
      duplicate selectors, and ambiguous aliases.
- [x] Replace wildcard setting matches in vendor policies with exhaustive
      setting matches.
- [x] Add schema migration fields for the new attribution data.
- [x] Keep old publications unavailable for new attribution-dependent Auto Fix
      types until reanalysis creates exact attribution.
- [x] Prepare one batch with every existing active project and global layer that
      has a supported model or reasoning setting.
- [x] Create a missing global config only when the vendor has a valid standalone
      entry. Never create a project config file.
- [x] Allow supported local edits despite a runtime, environment, managed,
      remote, organization, or server override. Return a behavior warning.
- [x] Stage every batch file, recheck all originals, roll back completed
      replacements after a later failure, and enter recovery for uncertain state.

### Agent Precedence Checklist

- [x] Claude: inspect managed, CLI, local, project, user, environment, model-specific
      effort, MCP files, skill overrides, and named agent frontmatter.
- [x] Codex: inspect user config, selected profile file, every trusted nested project
      config, named agent files, and project-key restrictions.
- [x] OpenCode: inspect managed, inline, `.opencode` resources, nested project JSON or
      JSONC, custom config, global config, and remote defaults.
- [x] Pi: inspect project and global recursive merge, model-specific maps, trust, and
      `PI_AGENT_DIR` or session-directory overrides.
- [x] Cursor: resolve only the public CLI model files: `~/.cursor/cli-config.json`
      and `<project>/.cursor/cli.json`. Register public MCP paths for Phase 4:
      `~/.cursor/mcp.json` and `<project>/.cursor/mcp.json`. Do not treat IDE
      settings, permissions, agents, or skills as CLI settings.
- [x] Antigravity: resolve only the public CLI model file
      `~/.gemini/antigravity-cli/settings.json`. Register public MCP paths for
      Phase 4: `~/.gemini/config/mcp_config.json` and
      `<project>/.agents/mcp_config.json`.
- [x] Document generic agent skill paths, `<project>/.agents/skills` and
      `~/.agents/skills`, as inventory paths only. They are not edited in Phase 3.

### Exit Criteria

- [x] Attribution is preferred metadata and does not block an editor-supported
      operation.
- [x] A prepared operation contains all changed existing active layers and an
      allowed new global layer.
- [x] Changed batch input rejects apply before replacement. A partial batch rolls
      back or enters durable recovery.

## Phase 4: Implement Safe Auto Fix Controls

Purpose: add reversible, reviewed edits where detector evidence identifies a
supported setting and the current config targets can be edited safely.

### D: Session Overdepth

- [x] Offer a fix only when the finding maps to a persisted automatic compaction
      control that is disabled or has a proved excessive threshold.
- [x] Claude: support `autoCompactEnabled` and `autoCompactWindow`.
- [x] Codex: support `model_auto_compact_token_limit`.
- [x] OpenCode: support the applicable V1 or V2 `compaction` fields without
      mixing their schemas.
- [x] Pi: support `compaction.enabled`, `reserveTokens`, `keepRecentTokens`, and
      exact supported model overrides.
- [x] Do not offer D Auto Fix when ordinary session growth, a runtime override,
      or fixed instructions caused the finding.

### S: Overpowered Subagents

- [x] Require one named worker and one supported worker-model target.
- [x] Claude: edit the exact named subagent frontmatter model.
- [x] Codex: edit the exact named agent TOML model or its selected config file.
- [x] OpenCode: edit the exact named agent model or variant.
- [x] Defer Pi extension workers until the extension defines a normal persisted
      model selector with exact invocation attribution.
- [x] Defer Cursor and Antigravity until their findings prove the effective
      worker model and config target.

### M: Unused MCP Servers

- [x] Require one named server, a supported project or global config target,
      an eligible observation interval, and complete calls for that interval.
- [x] Claude: add the narrowest effective deny or disable entry supported by the
      server's source scope.
- [x] Codex: set `mcp_servers.<name>.enabled = false`.
- [x] OpenCode: set the exact MCP server `enabled` field to `false` after M
      evidence support ships.
- [x] Antigravity: set the exact `disabled` field only after source and
      precedence support ship.
- [x] Do not remove credentials, server definitions, or unrelated tools.
- [x] Do not edit Cursor's private toggle store or invoke `agent mcp disable`.

### B: Unused Built-in Tools

- [x] Require exact tool identity and a supported disable control.
- [x] Claude: append one exact permission deny rule when it does not broaden an
      existing deny.
- [x] Codex: the documented app-tool controls do not identify one built-in tool,
      so B Auto Fix remains explicitly unavailable.
- [x] OpenCode: append one exact V2 permission deny rule after inventory support
      ships.
- [x] Pi: remove one exact tool from `defaultTools` after inventory support
      ships.
- [x] Do not disable a tool through a broad wildcard or a general permission
      posture change.

### K: Unused Skills

- [x] Require exact skill identity, source path, winning definition, and
      invocation coverage.
- [x] Claude: set the exact `skillOverrides` entry to `off`.
- [x] Codex: set the exact `skills.config` entry to `enabled = false`.
- [x] OpenCode: append one exact skill permission deny rule.
- [x] Defer Pi path exclusions until array precedence and the winning skill path
      are proved.
- [x] Do not delete skill files or edit `SKILL.md` content.

### F: Fast Mode Overuse

- [x] Require explicit persisted fast-tier evidence and a supported config
      target.
- [x] Claude: set `fastMode` to `false` or remove the winning true value when
      removal has the same documented effect.
- [x] Codex: replace or remove `service_tier = "fast"` according to the winning
      layer and reviewed standard-tier semantics.
- [x] Do not infer fast mode from model names, variant names, UI labels, or
      response latency.

### C: Cache Churn

- [x] Keep Auto Fix unavailable until one supported cache control is proved
      useful for the finding.
- [x] Do not present compaction, model replacement, or cache notices as a cache
      fix without a reviewed cache-control contract.
- [x] Keep prompt remediation available where the finding is valid.
- [x] Record any future provider-specific cache control as a separate reviewed
      operation rather than a generic cache toggle.

### Operation Safety Checklist

- [x] Prepare returns the exact old value, new value, selector, scope, file, and
      expected side effect.
- [x] Apply repeats resolution and conflict checks.
- [x] Write through a same-directory temporary file and atomic replacement.
- [x] Preserve permissions and unrelated bytes where the format allows it.
- [x] Perform semantic readback through the same vendor resolver.
- [x] Enter durable recovery when replacement success is uncertain.
- [x] Enroll a detector-specific passive verification watch after success.
- [x] Never claim savings before later passive evidence verifies the change.

### Exit Criteria

- [x] Every available Auto Fix cell has precedence, mutation, readback,
      conflict, recovery, and verification tests.
- [x] Every unsupported cell has a test that explains its blocker.
- [x] C remains unavailable unless its causal-control gate is met.

## Phase 5: Generalize the Review UI

Purpose: present new typed operations without weakening the existing two-step
review contract.

### Checklist

- [x] Extend Rust and TypeScript review DTOs beyond `model | reasoning`.
- [x] Use one exhaustive operation-kind vocabulary in Rust and TypeScript.
- [x] Show the agent, check, scope, path label, selector label, expected value,
      proposed value, effect, and important side effect.
- [x] Show a named server, tool, skill, or worker only when the backend supplies
      a bounded reviewed display value.
- [x] Keep prepare and apply as separate user actions.
- [x] Keep stale action and stale prepared-operation handling.
- [x] Keep prompt remediation available when Auto Fix is unavailable.
- [x] Add analytics for review, confirmation, result category, and later passive
      outcome under the existing privacy contract.
- [x] Do not send paths, resource names, config values, session IDs, model IDs,
      exact tokens, or exact costs in analytics.
- [x] Update `docs/analytics-measurement.md` if event definitions change.
- [x] Add keyboard, screen-reader, reduced-motion, narrow-window, light-theme,
      and dark-theme tests or review cases.

### Exit Criteria

- [x] The frontend derives Fix availability only from the backend result.
- [x] Unsupported operation kinds fail type checks instead of falling back to a
      model or reasoning presentation.
- [x] The user can identify the exact effect before confirming an edit.

## Phase 6: Assess Selected Second-Tier Agents

Purpose: replace generic parsing claims with dedicated bounded decisions for
Copilot, Cline, and Kiro. Amp and Windsurf are deferred outside this delivery.
This phase does not block first-tier improvements.

### Copilot

- [x] Characterize current `session-state` files and `events.jsonl` against the
      public Copilot session-event schema.
- [x] Determine which public SDK events are persisted by Copilot CLI rather than
      only streamed.
- [x] Confirm that the accepted v1 CLI contract has no `session-store.db`,
      `data.db`, or other database session source to admit.
- [x] Keep database WAL handling out of the v1 CLI contract because the accepted
      source is the append-only `events.jsonl` file.
- [x] Evaluate D, T, S, M, B, K, O, and C. The persisted v1 contract supports
      only S/O; no inventory, request-depth, or cache-churn claim is made.
- [x] Keep F unsupported unless a persisted speed or service-tier signal exists.
- [x] Keep Copilot IDE chat separate from CLI session events.

### Cline

- [x] Add a new format for `messages-contract-v1` instead of broadening
      `ClineSessionJson`.
- [x] Treat `messages.json` as the canonical replay artifact and hooks as
      optional auxiliary evidence.
- [x] Keep legacy `api_conversation_history.json`, `ui_messages.json`, and task
      metadata in separate source contracts.
- [x] Evaluate model, usage, context, tools, MCP, skills, and subagents only from
      fields guaranteed by the accepted contract.
- [x] Add migration-era fixtures where SDK and legacy records coexist.

### Kiro

- [x] Separate V2 and V3 session formats.
- [x] Pin the V2 local file-bundle shape with synthetic fixtures.
- [x] Treat manual `/chat save` exports as a separate format from automatic
      local sessions. The command is public but its JSON shape is not, so it is unsupported.
- [x] Retain only safe V2 model, token, tool, and child-parent facts. Do not
      enable findings for parent links, MCP, compaction, or completion.
- [x] Keep all Kiro clean results disabled while the producer schemas remain unpublished.

### Deferred: Amp

Amp is outside this delivery. A later plan must characterize `threads/*.json`,
keep `AmpFileChanges` out of session analysis, and avoid `--stream-json`.

### Deferred: Windsurf

Windsurf is outside this delivery. A later plan must keep workspace JSON,
mirrors, and Cascade protobuf separate and must not use decryption keys or
binary extraction without an approved format policy.

### Exit Criteria

- [x] Each in-scope second-tier surface is dedicated and fixture-backed or
      clearly fail-closed.
- [x] Public support text does not describe in-scope discovery as usable analysis.
- [x] No in-scope source requires an optional command, export, hook, or
      subscription.

## Phase 7a: Native Clipboard Prompt Copy

Purpose: keep the first Copy fix prompt action inside a native clipboard write
on macOS, Windows, and Linux.

- [x] Add the Tauri clipboard manager and initialize it in the desktop shell.
- [x] Grant only `clipboard-manager:allow-write-text` to the main window.
- [x] Route prompt copies through one native write boundary.
- [x] Keep prompt preparation and clipboard failures distinct and retryable.
- [x] Add first-click and native-write retry tests.
- [x] Do not add clipboard read permission or tell readers to change system settings.

## Phase 7: Verification and Release Review

### Engine Checklist

- [x] Add characterization fixtures for every new accepted shape.
- [x] Add malformed, oversized, truncated, duplicate, reordered, unknown,
      changed-source, missing-companion, and unsupported-version cases.
- [x] Add finding and clean tests per `SourceFormat` and detector.
- [x] Add provider-route and model-policy tests for T, O, F, and C.
- [x] Add active-branch, compaction, cache-link, and child-ownership tests.
- [x] Run `cargo fmt --check` in `crates/antiburn-local`.
- [x] Run `cargo clippy --all-targets --locked -- -D warnings` there.
- [x] Run `cargo test --locked --test check_coverage_contract` there.
- [x] Run `cargo nextest run --locked --lib --tests` there.

### Desktop Rust Checklist

- [x] Add editor support-matrix tests for every agent, setting, scope, source,
      and platform cell.
- [x] Add precedence and target-attribution tests for every supported control.
- [x] Add prepare, apply, changed-target, semantic readback, rollback, uncertain
      replacement, recovery, and passive verification tests.
- [x] Add migration, retention, clear, delete, and privacy tests for new stored
      attribution.
- [x] Run `cargo fmt --check` in `apps/desktop/src-tauri`.
- [x] Run `cargo clippy --all-targets --locked -- -D warnings` there.
- [x] Run `cargo test --locked` there.

### Frontend Checklist

- [x] Add DTO boundary tests for every operation kind.
- [x] Add unavailable, review, confirm, success, conflict, recovery, recurrence,
      and later-verification states.
- [x] Run `pnpm --filter @antiburn/desktop format`.
- [x] Run `pnpm --filter @antiburn/desktop lint`.
- [x] Run `pnpm --filter @antiburn/desktop type-check`.
- [x] Run `pnpm --filter @antiburn/desktop knip`.
- [x] Run `pnpm --filter @antiburn/desktop test`.
- [x] Run `pnpm --filter @antiburn/desktop build`.

### Contract and Quality Checklist

- [x] Update `docs/check-coverage.md` with implemented finding, clean, prompt,
      Auto Fix, verification, and savings support.
- [x] Update `docs/session-coverage.md` with discovery, shape, framing,
      companion, route, version, and completeness changes.
- [x] Update `docs/support.md` and fixture READMEs.
- [x] Run the repository coverage and design-drift checks that apply.
- [x] Run `aislop scan --changes` after each coherent code phase.
- [x] Run `pnpm run slop:all` before push.
- [x] Run `pnpm run secrets` before push.
- [x] Perform native macOS read and write tests.
- [x] Record the native Linux read/write release gate. The cross-platform editor
      matrix passed on macOS; a Linux host must run the native read/write suite.
- [x] Record the native Windows read-only release gate. The target is installed,
      but macOS lacks the MSVC C toolchain required for a cross-check; a Windows
      host must run the read-only suite.
- [x] Record the WSL discovery and no-write release gate. The native-environment
      matrix passed on macOS; a WSL host must run discovery and assert no write.

Native macOS tests ran on 2026-09-15. Linux, Windows, and WSL were not executed
in this macOS review. The release gates above require native execution before a
release. The Windows `x86_64-pc-windows-msvc` target was available, but its
cross-check stopped before compilation because the macOS host has no MSVC C
headers or linker.

### Final Exit Criteria

- [x] Every advertised finding has detector-grade passive evidence.
- [x] Every advertised clean result has complete source and detector evidence.
- [x] Every advertised Auto Fix has a supported current target, safe mutation,
      readback, recovery, and later passive verification.
- [x] Every unsupported surface states whether the blocker is the source,
      missing characterization, missing implementation, or unsafe mutation.
- [x] The coverage documents describe the released code, not future phases.
