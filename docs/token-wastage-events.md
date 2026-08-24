# Token-wastage events

A personal reference: eleven events that plausibly waste tokens in a coding-agent
session, what antiburn can see about each one **today**, whether the event can be
detected deterministically from a transcript, and what would have to change to stop
it happening.

This is a working note, not a specification. Where it disagrees with
[`docs/plans/local-insights-architecture.md`](plans/local-insights-architecture.md),
that plan wins — it is the ratified architecture. This file exists to record what the
code actually does right now, verified against the tree, and to be honest about the
distance between that and the plan.

**Verified against `main` at `3a666ce`.** Every claim below was checked against that
revision. Prefer symbol names over line numbers when following them — the symbols are
what this file is anchored to. Re-verify before trusting an entry after the analysis
code moves; the cache-rehydration entry went stale within two days of first being
written, which is exactly the failure mode to expect here.

## How to read the status column

Three different things get called "detection" and conflating them wastes time:

- **Computed today** — a value the engine already produces and the app can render.
- **Parsed, not judged** — the fact reaches `NormalizedEvent`, but nothing decides
  anything about it.
- **Not parsed** — the transcript carries the signal, the parser discards it.

The plan's `EvidenceValue` distinction matters throughout: `Complete(0)` means the
event did not happen; `Unsupported` and `Partial` never mean zero. Nothing in the
tree implements `EvidenceValue` yet, so every "absence" below is currently an
unqualified absence — which is the main correctness debt in this area.

## Summary

| # | Event | Status today | Deterministic? |
|---|---|---|---|
| 1 | Cache rehydration after TTL lapse | Computed today | Yes — two paths, seven thresholds |
| 2 | Oversized-context compaction | Parsed, not judged | Yes |
| 3 | Unused MCP servers | Both halves parsed, not joined | Yes |
| 4 | Unused skills | Both halves parsed, not joined | Yes |
| 5 | Unused built-in tools | Loaded set not itemized | Partial at best |
| 6 | Overpowered subagents | Parsed, not judged | Yes |
| 7 | Reasoning tier above need | Parsed, vendor-uneven | Partly |
| 8 | Fast mode as a standing default | Parsed, Claude only | Partly |
| 9 | Deprecated model still in use | Models known, no catalog | Yes, once curated |
| 10 | Redundant search and re-reading | Not parsed | Yes, needs a new field |
| 11 | Read-back after a confirmed write | Not parsed | Yes, needs a new field |
| 12 | Quota-limit rejections | Not parsed from transcripts | Yes, once error records are read |

Twelve rows for eleven events: quota pressure is listed because it belongs in any
honest wastage list, even though the private-compatibility catalog keeps it separate.

---

## 1. Cache rehydration after a TTL lapse

**What it is.** A prompt-cache entry expires. The next turn re-writes the whole
conversation to the cache instead of appending a suffix. Cache writes cost more than
cache reads, so one lapsed turn can cost more than the dozen turns before it.

**Status today: computed.** This is the one event on the list with a finished
detector, and it is the most developed code in this area.
`crates/antiburn-local/src/analysis/engine.rs` defines the rule;
`SessionMetrics::cache_rehydration_count` and `Bucket::is_cache_rehydration` carry
it, and `ActiveSessionsSummary::cache_rehydration_count` sums it across sessions.

**Deterministic detection.** Yes, subject to seven named thresholds across **two
paths**. Which path runs is decided by `NormalizedSession::cache_write_tokens_available`
— that is, by whether the provider reports cache writes at all.

*Direct path* (`is_cache_rehydration_turn`), for providers that report cache
writes. A turn counts when all of these hold:

- `context_tokens >= CACHE_REHYDRATION_MIN_CONTEXT_TOKENS` (20,000) — a full rewrite
  of a small context is not worth flagging;
- `cache_write_tokens / context_tokens >= CACHE_REHYDRATION_WRITE_RATIO` (0.5) — the
  ratio is deliberately not near 1.0, because the system prompt and tool definitions
  stay cached across sessions, so a rehydration rewrites only the conversation;
- `prev_cache_read_tokens / prev_context_tokens >= CACHE_REHYDRATION_PRIOR_READ_RATIO`
  (0.5) — the previous turn was mostly cache-served, so this is a real lapse rather
  than a session's first turn;
- and the turn is **not** the first after a compaction boundary, since compaction
  always rewrites the (smaller) context and that is not a TTL lapse.

*Inferred path* (`inferred_cache_rehydration_turn`), for providers that report no
cache writes — Codex is the case this was built for. With no write signal, the rule
looks for a cache-read collapse followed by a large replay, then waits for the next
turn to confirm the cache rebuilt:

- the previous turn was mostly cache-served (same `PRIOR_READ_RATIO`), and this turn
  reads at most `CACHE_REHYDRATION_MISS_READ_RATIO` (0.2) from the old cache;
- the context did not shrink: `CACHE_REHYDRATION_CONTEXT_RETENTION_RATIO` (0.8);
- fresh input minus real context growth — the *replayed* portion — reaches
  `CACHE_REHYDRATION_REPLAY_RATIO` (0.5) of the context;
- the next turn reads back at least `CACHE_REHYDRATION_RECOVERY_READ_RATIO` (0.5)
  from the rebuilt cache and itself retains the context.

Both paths are parent-only: `EventSource::Subagent` events are skipped, because a
subagent has its own context window and mixing its turns into the parent's cache
arithmetic means nothing. Both also guard on the model: `same_known_model` stops a
mid-session model switch from being read as a TTL lapse, since a switch invalidates
the cache for a different reason.

The inferred path is the more interesting piece of engineering and the more fragile
one. It is a three-turn window with five ratios, inferring an event the transcript
never records. It should be the first thing re-examined if rehydration counts ever
look wrong on a provider that reports no cache writes.

**What is genuinely a threshold, not a fact.** All seven ratios are policy. Under the
plan's "findings are policy; evidence is fact" rule they should live in a detector
over persisted evidence, not baked into the metrics pass — changing 0.5 to 0.6 today
requires reparsing every transcript.

**How to avoid it.**

- *User:* finish or hand off a task before walking away from a long context. Coming
  back after lunch to a 150k-token session is the classic case.
- *User:* `/clear` before a break costs nothing; resuming a stale context costs the
  whole window.
- *Agent:* avoid mid-session model switches, which invalidate the cache for the same
  reason a TTL lapse does. The detector knows this and excludes them, so a switch
  costs real tokens without ever showing up as a rehydration finding.
- *Codebase:* the detector works. The open question is confidence in the inferred
  path, which has no ground truth to check itself against.

---

## 2. Oversized-context compaction

**What it is.** A request's context crosses the autocompact cap. The window is
summarized, and the summary is then re-sent on every subsequent turn. The compaction
itself costs a full read of the pre-compaction context, and the sessions that compact
repeatedly pay it repeatedly.

**Status today: parsed, not judged.** The parser is complete.
`crates/antiburn-local/src/analysis/vendors/jsonl.rs` reads Claude's
`system`/`compact_boundary` record and its `compactMetadata`, populating
`NormalizedEvent::is_compaction_boundary`, `compaction_trigger`
(`CompactionTrigger::{Manual, Auto}`), `compaction_pre_tokens`, and
`compaction_post_tokens`. Codex's `event_msg`/`context_compacted` sets the boundary
but names no trigger. The engine surfaces `SessionMetrics::compaction_count` and
per-bucket fields. Nothing decides that a session compacted *too much* or *too late*.

**Deterministic detection.** Yes. Every input is already on the event:

- cost of one compaction ≈ `compaction_pre_tokens` read at input rates;
- `compaction_trigger == Auto` distinguishes "the agent ran out of room" from "the
  user chose a boundary" — the first is the wasteful one;
- repeated auto-compactions in one session are countable directly;
- depth is `peak_context_tokens` against `context_window`, guarded by
  `context_available`.

One real caveat: `context_available` is false for unrecognized Claude model ids, and
`CONTEXT_WINDOW_TIERS` snaps a nominal window up when the observed peak tops it. So
occupancy must be presented as unavailable rather than as a wrong percentage — this is
exactly the `Unsupported`-is-not-zero rule.

**How to avoid it.**

- *User:* compact at task boundaries deliberately rather than letting auto-compaction
  pick the moment. A manual compaction between tasks discards what is genuinely dead;
  an automatic one mid-task summarizes work you still need and you pay to rebuild it.
- *User:* lower the autocompact threshold so compaction happens on a smaller context.
- *Agent:* push open-ended exploration into a subagent, whose context dies with it,
  instead of accumulating it in the main window.
- *Codebase:* nothing new to parse. The gap is a detector that compares depth to the
  cap and counts auto-compactions per session.

---

## 3. Unused MCP servers

**What it is.** An MCP server's tool definitions load into the system prompt of every
request in a session. If no tool from that server is ever called, that is a fixed
per-request tax for nothing, multiplied by turn count.

**Status today: both halves parsed, never joined.** This is the cheapest real win on
the list.

- *Loaded side:* `crates/antiburn-local/src/analysis/initial_context.rs` reads Claude's
  `mcp_instructions_delta` attachment, pulling `addedNames` and `addedBlocks` into
  `InitialContextTokenSource::McpInstructions` rows that carry both the server name and
  an estimated token count.
- *Invoked side:* `ToolCall::name` keeps the raw vendor tool name.
  `ToolCategory::from_tool_name` is substring-based and case-insensitive, so an
  `mcp__github__list_issues` call is bucketed as `Other` but its full name survives.

Nothing joins the two.

**Deterministic detection.** Yes. Take the set of names from the
`mcp_instructions_delta` rows; take the set of servers implied by the `mcp__<server>__`
prefix of every `ToolCall::name`; report the difference, priced by the estimated token
count already attached to each loaded row, multiplied by request count.

The honest qualification: `initial_context` is best-effort and returns `None`
("unavailable") for any agent other than Claude and Codex, and the token counts come
from `estimate_tokens`, not from the provider. So a finding is a good estimate, and
absence of the attachment must read as `Unsupported`, not as "no MCP servers".

**How to avoid it.**

- *User:* remove the server from the MCP config, or scope it to the projects that use
  it rather than enabling it globally.
- *User:* prefer deferred/on-demand tool loading where the harness offers it, so
  definitions cost nothing until searched for.
- *Codebase:* the join described above, plus per-server token attribution in the
  report.

---

## 4. Unused skills

**What it is.** Skill descriptions load into every request's context so the model can
decide whether to invoke one. A skill that is never invoked is paying rent on that
description for the life of the session. A skill whose *body* loaded and then went
unused is far worse.

**Status today: both halves parsed, never joined.** Same shape as MCP, and equally
close to done.

- *Loaded side:* `parse_initial_context` walks Claude's `skill_listing` attachment via
  `parse_named_markdown_bullets`, producing one `SkillInstructions` row per skill with
  its name and estimated tokens. A fully loaded skill body is caught separately by the
  `"Base directory for this skill:"` marker in a meta user record, named by
  `parse_claude_loaded_skill_name`.
- *Invoked side:* `crate::model::skill::SkillUse` records one entry per `Skill` tool
  call, with `name`, `progress`, `tokens_out`, `context_tokens`, and a `description`
  grafted from the listing by `parse_skill_descriptions`.

**Deterministic detection.** Yes — set difference between listing names and `SkillUse`
names, priced by the listing rows' token counts. Grouping by origin (installed,
project, plugin, bundled) as the plan's catalog wants is *not* available: the listing
gives a name and a description, not a provenance. That grouping needs either a new
parse or a filesystem lookup, and a filesystem lookup would be reporting on the
machine's current state rather than on what the session actually loaded.

**How to avoid it.**

- *User:* uninstall or disable skills that never fire. A plugin that ships twelve
  skills for one you use costs eleven descriptions per request.
- *User:* narrow a skill's `description` so it stops loading speculatively — though note
  that the description is what loads, so a shorter description is itself the saving.
- *Codebase:* the set difference, plus a decision about whether origin grouping is
  worth a new parse.

---

## 5. Unused built-in tools

**What it is.** The harness's own tool definitions occupy context whether or not the
session uses them.

**Status today: the loaded set is not itemized.** This is the weakest entry on the
list and it is worth being blunt about why. `parse_claude` handles
`deferred_tools_delta` by serializing the whole attachment to a string and folding its
`estimate_tokens` into a single `SystemInstructions` row. There is no per-tool name and
no per-tool token count. The invoked side is fine — `ToolCall::name` and the `ToolMix`
counters are complete — but you cannot compute a difference against a set you never
enumerated.

**Deterministic detection.** Partial at best, today. The `deferred_tools_delta`
attachment does carry tool names before it is flattened, so itemizing it is a small
parser change. But the *non-deferred* built-in tool definitions never appear in the
transcript at all — they live in a system prompt the transcript does not record. So
even after that change, the honest status for the always-loaded core tools is
`Unsupported`, not `Complete(0)`.

**How to avoid it.**

- *User:* disable a tool only where the tool, the capability lost, and a safe disable
  mechanism are all known. Otherwise this finding stays audit-only — the plan says so
  explicitly and it is right.
- *Codebase:* itemize `deferred_tools_delta` instead of flattening it; mark the
  core-tool half `Unsupported` rather than inferring it.

---

## 6. Overpowered subagents

**What it is.** A premium main-loop model spawns subagents that silently inherit the
same premium tier. Fan out ten agents and you are running ten premium contexts for
work that a cheaper tier would finish.

**Status today: parsed, not judged.** Every input exists.

- `crates/antiburn-local/src/analysis/merge.rs` merges each subagent transcript into
  the parent with `merge_subagent_events`, tagging every event
  `EventSource::{Parent, Subagent}`.
- `NormalizedEvent::model` carries the per-turn model, so parent and subagent models
  are directly comparable within one merged session.
- `Bucket::subagent_launches` counts `Task` tool calls on parent turns.
- `SessionMetrics::model_breakdown` retains billable tokens per normalized model key,
  and `model_runs` lists distinct model/thinking-mode pairs.

The product rule already in place — a subagent is an implementation detail of its
parent, so its spend counts toward the parent — is exactly the right frame for this
finding.

**Deterministic detection.** Yes. For each merged session, compare the model of
`Subagent` events against the parent's model and against a premium-tier list, and
price the subagent share from `model_breakdown`. The only judgement call is which
models count as premium, which is catalog policy and should live in the detector, not
in evidence.

**How to avoid it.**

- *User:* set a cheaper default subagent model, or a per-agent model in the agent
  definition. Keep premium subagents where they are deliberately justified.
- *Agent:* do not fan out ten premium agents for a search that one cheap agent could
  do; match the tier to the subtask.
- *Codebase:* the comparison above. No new parsing.

---

## 7. Reasoning tier above what the task needs

**What it is.** Thinking or effort left at a high tier for mechanical work. Reasoning
tokens are output tokens, at output rates.

**Status today: parsed, vendor-uneven.** `NormalizedEvent::thinking_mode` is populated
from `effort` or `reasoning_effort`, at the record's top level or inside `message`.
Codex is the strong case: `crates/antiburn-local/src/analysis/vendors/codex.rs` tracks
`current_thinking_mode` statefully across the rollout, so every event carries the mode
in force. `NormalizedEvent::has_thinking` is set from a `thinking` content block
(Claude) or a `reasoning` response item (Codex).

**Deterministic detection.** Partly, and the split matters. Where a transcript records
an explicit `effort`/`reasoning_effort`, the tier is a fact and a threshold comparison
is deterministic. Where it does not, `has_thinking` tells you only that the model
thought, not how hard — that is a presence signal, not a tier. The plan is explicit
that prompt keywords must **not** be inferred as reasoning-tier evidence, and that is
the correct line: inferring "ultrathink" from prompt text would manufacture evidence.
So this detector is `Complete` for Codex, and `Partial` or `Unsupported` for a Claude
session whose records carry no effort field.

**How to avoid it.**

- *User:* lower the tier with the provider's own effort control for mechanical work.
- *Agent:* reserve high effort for the genuinely hard step, not the whole session.
- *Codebase:* a threshold detector over `thinking_mode`, with an explicit
  `Unsupported` for sessions that carry no tier field — not a fallback to `has_thinking`.

---

## 8. Fast mode as a standing default

**What it is.** Fast-tier serving applied to work where latency does not matter, such
as delegated or batch work, at fast-tier prices.

**Status today: parsed, Claude only.** `NormalizedEvent::speed` is read from
`message.usage.speed`, top-level `usage.speed`, or a bare `speed` field, and reaches
`Bucket::speed` on parent turns. Codex rollouts carry no equivalent, and the codex
adapter says so in a comment rather than guessing. Pricing has fast-tier entries for
other vendors (`GPT_55_FAST_MODEL_KEY`, `composer-2.5-fast` in
`crates/antiburn-local/src/pricing/table.rs`), but those are model keys, not a
per-turn speed signal.

**Deterministic detection.** Partly. For Claude, `speed == "fast"` on a turn is a
fact, and correlating it with delegated work (`EventSource::Subagent`) or with a whole
session's worth of turns is straightforward. For every other vendor the answer is
`Unsupported`. Treating a missing `speed` as "standard" would be exactly the false
absence the plan forbids.

**How to avoid it.**

- *User:* use fast mode deliberately for interactive, latency-sensitive work; turn it
  off for delegation-heavy or background work.
- *Codebase:* a share-of-turns detector over `speed`, gated to vendors that report it.

---

## 9. Deprecated model still in use

**What it is.** Paying an older model's rate after a cheaper, better replacement
shipped.

**Status today: models are known, no deprecation catalog exists.** `SessionMetrics`
carries `model`, `model_runs`, and `model_breakdown`; `crates/antiburn-local/src/pricing/`
prices them. A grep for deprecation across the crate finds nothing: there is no
curated map from a retired model to its replacement, and no notion of when a
replacement became available.

**Deterministic detection.** Yes, once the catalog exists — and the catalog is the
whole job. Detection is a lookup of each key in `model_breakdown` against a curated
list, with the finding gated on the session's timestamp being after the replacement's
availability date. Pricing the finding is a second lookup at the replacement's rate.
Critically, this catalog belongs to the detector, not to evidence: the plan's FR-11
says a catalog change must **not** requeue any session, and that only holds if the
catalog is read at report time.

**How to avoid it.**

- *User:* update the default model in the agent's config. Most instances of this are a
  stale config file, not a decision.
- *Codebase:* a curated, dated model-replacement catalog, read at report time.

---

## 10. Redundant search and re-reading

**What it is.** The agent greps for the same thing twice, re-reads a file it already
has in context, or reads a whole file when it needed thirty lines. Every repeat pays
full input cost for content already in the window, and it inflates the context, which
in turn drives events 1 and 2.

**Status today: not parsed.** The counters exist but the identity does not.
`ToolMix` counts `read`, `search`, `edit`, `test`, `bash`, `other`, and
`SessionMetrics::grep_count` counts search-tool calls via `ToolCategory::is_grep`. So
antiburn can already say *how much* searching happened — it is surfaced as a "search
intensity" signal. It cannot say whether any of it was **repeated**, because
`ToolCall` keeps only `name`, `category`, and `detail`, and `detail` is populated for
skill calls only. `tool_call_from_input` receives the full input `Value` and extracts
just the shell command and the skill name; everything else — `file_path`, `pattern`,
`offset`, `limit` — is dropped on the floor.

Tool *results* are worse: `jsonl.rs` matches `"tool_result" | "toolResult" |
"function_call_output"` and does nothing with them, so no result payload or size is
retained anywhere.

**Deterministic detection.** Yes, and cheaply, but it needs one new field. The
predicate is a plain duplicate count over a normalized target key per session:

- repeated `Read` of the same `file_path` with overlapping ranges;
- repeated `Grep` with an identical pattern and path;
- a whole-file `Read` (no `offset`/`limit`) of a file above some line count.

None of this requires a second pass — it is counters over records the stream already
decodes, which is the FR-5 constraint that keeps evidence free. What it requires is
that `ToolCall` carry a stable, **non-identifying** target key rather than the raw
path. That constraint is not optional: `docs/oss/` and `tests/boundary.rs` enforce the
engine's local boundary, and the plan forbids persisting raw transcript content.
A salted hash of the normalized path is enough to count duplicates without storing the
path, and it keeps the evidence rule-neutral.

Pricing a finding is the harder half: without result sizes you know a read repeated,
not what it cost. Either retain a coarse result-size bucket, or attribute the cost of
the turn the repeat landed in.

**How to avoid it.**

- *Agent:* read a range, not a file, once the relevant region is known. Search before
  reading, then read narrowly.
- *Agent:* keep a note of what has already been read rather than re-fetching it; the
  content is still in the context window.
- *User:* point the agent at the file. Most redundant searching is the agent
  reconstructing knowledge the prompt could have supplied in one line.
- *User:* a `CLAUDE.md`/`AGENTS.md` that names the repository layout removes a whole
  class of exploratory searching from every session.
- *Codebase:* add a hashed target key to `ToolCall`, populate it in
  `tool_call_from_input`, and count duplicates in the metrics pass.

---

## 11. Read-back after a confirmed write

**What it is.** The agent edits a file, then reads it back to check the edit landed —
when the edit tool already confirmed it. A pure re-read of content the agent just
authored and still holds in context.

**Status today: not parsed.** Same root cause as event 10: the ordering is available
(events are in transcript order, with `ToolCategory::Edit` and `ToolCategory::Read`
already distinguished), but the *target* is not, so "read the file I just wrote"
cannot be told from "read a different file".

**Deterministic detection.** Yes, once event 10's target key exists. The predicate is
narrow enough to be near-zero false positives: a `Read` whose target key matches an
`Edit` target key from a preceding turn in the same session, with no intervening
external event that could have changed the file. Tightening it to "the immediately
following turn" makes it stricter and cheaper still.

The one honest caveat: a read-back is legitimate after an edit the tool reported as
partially applied, or after an external process touched the file. The detector should
count occurrences and let the reader judge, not assert waste.

**How to avoid it.**

- *Agent:* trust the edit tool's confirmation. Claude Code's own harness guidance
  already says not to re-read a file to verify an edit, because the tool would have
  errored if the change failed — this event is a measurable check on whether that
  instruction is being followed.
- *Codebase:* the same `ToolCall` target key as event 10, plus an adjacency check. One
  field unlocks both detectors, which is the argument for doing it.

---

## 12. Quota-limit rejections

**What it is.** A request that consumes input tokens and comes back as a limit error,
then gets retried. The rejected attempt is paid for and produces nothing.

**Status today: not parsed from transcripts.** A grep for `rate_limit`, `quota`,
`usage_limit`, and `429` across `crates/antiburn-local/src` returns nothing outside
tests. What *does* exist is a different subsystem at a different altitude: the desktop
app reads a provider's own current usage figures with the reader's credentials, and
`apps/desktop/src-tauri/src/dto.rs` models `LiveUsageWindow`, `LiveUsageForecast`, and
`LiveUsageSupport` (five-hour, weekly, per-model windows), with
`apps/desktop/src-tauri/src/usage_alerts.rs` raising milestones and spend anomalies.
That is account-level state, live. It says nothing about which session hit a limit.

**Deterministic detection.** Yes, for the transcript half, once error records are
read. The signal is an error record attributable to a session, carrying a limit kind
and sometimes a reset time or a utilization figure. Reporting limit kind, hit count,
affected sessions and models, and observed times is all fact.

Two things make this awkward and both are real rather than incidental. First,
provider limits are not one thing: rolling five-hour, weekly, per-model allocation,
weighted rather than raw usage, or a bare rate-limit error with no numeric quota
exposed. Second, when the transcript carries no quota evidence the correct answer is
"not assessed" — an absence of limit errors in a transcript that never records them is
not a clean bill.

**How to avoid it.**

- *User:* spread heavy work rather than burning a five-hour window in one sitting;
  the live usage windows in the app are the place to see this coming.
- *Agent:* back off rather than retrying immediately into the same limit.
- *Codebase:* parse limit errors into evidence, keeping the section `Unsupported`
  wherever the transcript exposes nothing.

---

## What one change would unlock the most

Two entries, 10 and 11, are blocked on a single missing field: a stable, non-identifying
target key on `ToolCall`. The input is already in hand at
`vendors::jsonl::tool_call_from_input` and is discarded there. Adding a hashed key
costs one field and one hash per tool call, needs no second pass over the transcript,
and satisfies both the local boundary and the rule-neutral-evidence rule.

Two more, 3 and 4, are blocked on nothing at all: the loaded set and the invoked set
are both parsed today, and no code takes the difference between them.
