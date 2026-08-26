// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Public presentation types for local session analysis.
 *
 * Every type here mirrors the JSON shape a value from the `antiburn-local`
 * engine crate takes when it crosses the IPC boundary. The engine serializes
 * with `#[serde(rename_all = "camelCase")]`, so a field here is the camelCase
 * spelling of the Rust field it mirrors, and each block records the engine
 * type it comes from.
 *
 * These are *presentation* types, deliberately declared in the renderer rather
 * than generated: the charts and cards need a stable shape to render, not a
 * transport contract. Nothing here carries an account, transfer, or remote
 * enrichment field — the engine produces none, and the public surface must not
 * invent one.
 */

/* -------------------------------------------------------------------------
 * Timeline metrics — mirrors `analysis::engine::{ToolMix, Bucket}`.
 * ---------------------------------------------------------------------- */

/** Tool-call counts by category. Mirrors engine `ToolMix`. */
interface ToolMix {
  edit: number
  read: number
  search: number
  test: number
  bash: number
  other: number
}

/** One point on the shared 0→100% session-progress grid. Mirrors engine `Bucket`. */
export interface SessionBucket {
  /** The bucket contains parent effective input tokens. */
  tokensIn: number
  /** The bucket contains parent generated output tokens. */
  tokensOut: number
  /** The bucket combines effective input and generated output from sub-agents. */
  subagentTokens: number
  contextTokens: number
  /** True when a real compaction boundary landed in this bucket. */
  isCompactionBoundary: boolean
  /** The bucket sums parent cache-read tokens, which are already part of `contextTokens`. */
  cacheReadTokens: number
  /** The bucket sums parent cache-write tokens, which are part of `tokensIn`. */
  cacheWriteTokens: number
  /** True when a turn in this bucket is a cache rehydration: the cache TTL lapsed and re-wrote. */
  isCacheRehydration: boolean
  /** True when a turn in this bucket is a cache routing miss: a fast re-send too soon for a TTL lapse. */
  isCacheRoutingMiss: boolean
  /** Wall-clock seconds since the prior parent turn, when this bucket contains a timed turn. */
  secsSincePriorTurn: number | null
  /** Count of `Task` tool calls in this bucket: how many sub-agents launched at this point. */
  subagentLaunches: number
  /** Count of user prompts in this bucket. */
  userPrompts: number
  /** The name of the last parent tool call in this bucket, when any. */
  lastTool: string | null
  /** The model that produced the last parent event in this bucket, when known. */
  model: string | null
  /** The thinking-effort mode of the last parent event in this bucket, when known. */
  thinkingMode: string | null
  /** The response speed of the last parent event in this bucket, when known. */
  speed: string | null
  /** True when any parent event in this bucket carries a thinking block. */
  hasThinking: boolean
  /** Whether the compaction in this bucket was manual or automatic, when known. */
  compactionTrigger: "manual" | "auto" | null
  /** The context token count right before the compaction in this bucket, when known. */
  compactionPreTokens: number | null
  /** The context token count right after the compaction in this bucket, when known. */
  compactionPostTokens: number | null
}

/* -------------------------------------------------------------------------
 * Initial context — mirrors `analysis::initial_context::{
 * InitialContextTokenSource, InitialContextSourceCount, InitialContextBreakdown}`.
 * ---------------------------------------------------------------------- */

/** Source dimension for tokens loaded before the agent's first response. */
type InitialContextSource =
  | "agent_instructions"
  | "system_instructions"
  | "skill_instructions"
  | "mcp_instructions"
  | "unattributed"

/** Where a skill or MCP server installs from. Mirrors `initial_context::SourceOrigin`. */
export type SourceOrigin = "bundled" | "plugin" | "user" | "project" | "unknown"

/** One source/source-name token count in the initial-context breakdown. */
interface InitialContextSourceCount {
  source: InitialContextSource
  /** Skill name, MCP server name, or instruction file — `null` when unnamed. */
  sourceName: string | null
  tokenCount: number
  /** How many times the session used this source after it loaded. 0 for non-skill/MCP sources. */
  useCount?: number
  /** Where the skill or MCP server is installed. Mirrors `initial_context::SourceOrigin`. */
  origin?: SourceOrigin
}

/**
 * Where a session's *initial* context window went, by source. Absent on a
 * session means the breakdown is unavailable for that agent (not zero).
 */
export interface InitialContextBreakdown {
  /** `tracked` = fully attributed; `trackedPartial` = some `unattributed`. */
  trackingStatus: "tracked" | "trackedPartial"
  totalTokens: number | null
  sources: InitialContextSourceCount[]
}

/* -------------------------------------------------------------------------
 * Cost — mirrors `analysis::engine::{SessionCost, ModelTokens}`.
 * ---------------------------------------------------------------------- */

/** On-device cost estimate for a session, split by billable component. */
export interface SessionCostComponents {
  totalUsd: number
  inputUsd: number
  outputUsd: number
  cacheReadUsd: number
  cacheWriteUsd: number
}

/**
 * Billable token counts, summed across one or more models.
 *
 * A single session already carries these counts as separate fields. This
 * type names a subject that spans more than one transcript, such as every
 * sub-agent combined.
 */
export interface BillableTokens {
  inputTokens: number
  outputTokens: number
  cacheReadTokens: number
  cacheCreationTokens: number
}

/**
 * Where a subject's spend went. Mirrors Rust `EfficiencyTotals`.
 *
 * Every field sums over priced turns only. A turn whose model has no price
 * counts in `unpricedTurns` and in nothing else, so any ratio of two fields
 * describes the same set of turns.
 */
export interface SessionEfficiency {
  totalUsd: number
  /** Output plus the fresh input that grew the context. */
  newWorkUsd: number
  /** Cache reads. */
  carryUsd: number
  /** Fresh input beyond the context growth. */
  rewriteUsd: number
  growthTokens: number
  outputTokens: number
  pricedTurns: number
  unpricedTurns: number
}

/** Billable token counts retained per normalized model key. */
interface ModelTokens {
  inputTokens?: number
  outputTokens?: number
  cacheReadTokens?: number
  cacheCreationTokens?: number
}

/* -------------------------------------------------------------------------
 * Metrics — mirrors `analysis::engine::{SessionMetrics, ActiveSessionsSummary}`.
 * ---------------------------------------------------------------------- */

/** Metrics for a single analyzed session. Mirrors engine `SessionMetrics`. */
export interface SessionMetrics {
  agent: string
  sessionId: string
  durationSecs: number
  /** Wall-clock span minus idle gaps; time the session was genuinely active. */
  activeSecs: number
  eventCount: number
  tokensIn: number
  tokensOut: number
  peakContextTokens: number
  /** Compaction boundaries in the parent transcript. */
  compactionCount?: number
  /** Turns the engine flagged as a cache rehydration. */
  cacheRehydrationCount?: number
  /** Turns the engine flagged as a cache routing miss. Not aggregated onto `ActiveSessionsSummary`. */
  cacheRoutingMissCount?: number
  /** False when the model's context window is unknown. */
  contextAvailable?: boolean
  contextWindow: number
  toolMix: ToolMix
  grepCount: number
  buckets: SessionBucket[]
  initialContext?: InitialContextBreakdown | null
  model?: string | null
  billableInputTokens?: number
  billableOutputTokens?: number
  billableCacheReadTokens?: number
  billableCacheCreationTokens?: number
  modelBreakdown?: Record<string, ModelTokens>
  cost?: SessionCostComponents | null
  efficiency?: SessionEfficiency
}

/**
 * Averaged view across the analyzed sessions. A single-session view uses
 * `sessionCount: 1`.
 * Mirrors engine `ActiveSessionsSummary`.
 */
export interface ActiveSessionsSummary {
  sessionCount: number
  avgDurationSecs: number
  avgActiveSecs: number
  toolMix: ToolMix
  grepTotal: number
  tokensInTotal: number
  tokensOutTotal: number
  peakContextTokens: number
  /** Compactions summed over the included sessions. */
  compactionCount?: number
  /** Cache rehydrations summed over the included sessions. */
  cacheRehydrationCount?: number
  /** Whether at least one included session has a known context window. */
  contextAvailable?: boolean
  contextWindow: number
  costTotalUsd?: number | null
  buckets: SessionBucket[]
  sessions: SessionMetrics[]
}

/* -------------------------------------------------------------------------
 * Orchestration — the local parent/child relationships the engine's sub-agent
 * discovery reports for one session.
 * ---------------------------------------------------------------------- */

/** One sub-agent launched by an orchestrator — one roster row. */
export interface SubagentMember {
  /** Orchestrator app slug (for example `claude-code`). */
  agent: string
  /** The sub-agent's transcript id, used to open its own analysis. */
  subagentId: string
  /** First task prompt (truncated), or a persona/slug fallback. */
  label: string
}

/** Sub-agent picture for one orchestrator session. */
export interface LocalOrchestrationStatus {
  /** True when the session launched two or more sub-agents. */
  orchestrating: boolean
  /** App slug of the orchestrator session. */
  orchestratorAgent: string
  /** The orchestrator's own session id. */
  orchestratorSessionId: string
  subagentCount: number
  members: SubagentMember[]
}

/* -------------------------------------------------------------------------
 * Fork relations — local-transcript parent/child links between sessions.
 * ---------------------------------------------------------------------- */

/** Identity of a local session on this machine. */
export interface LocalSessionIdentity {
  agent: string
  sessionId: string
  wslDistro?: string | null
}

/** One end of a local fork relation. */
export interface LocalSessionRelation {
  identity: LocalSessionIdentity
  title?: string | null
  /** False when the related session's transcript is no longer on this machine. */
  available: boolean
}

/** Direct fork relations for one session, resolved from local transcripts. */
export interface LocalSessionRelations {
  /** Title the relation graph resolved for the session itself, when known. */
  title?: string | null
  parent: LocalSessionRelation | null
  children: LocalSessionRelation[]
}
