/**
 * Typed IPC edge for sessions: the row and analysis payloads, the session
 * commands, and the session lifecycle bus (`session:lifecycle`,
 * `session:updated`, `session:index-changed`, the versioned live snapshot,
 * and named presence). `ipc.ts` re-exports everything here, so callers keep
 * one import path.
 */

import { invoke, isTauri } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"

import type {
  ActiveSessionsSummary,
  BillableTokens,
  SessionCostComponents,
  SessionEfficiency,
} from "./types/session"

/* -------------------------------------------------------------------------
 * Payload shapes — mirrors of `src-tauri/src/dto.rs`
 * ---------------------------------------------------------------------- */

/** One row of the activity list, before it is shaped for presentation. */
export interface ModelRunPayload {
  model: string
  thinkingMode?: string
}

export interface ActivityEntryPayload {
  agent: string
  sessionId: string
  repo: string
  timestamp: string
  isActive: boolean
  surface: string
  wslDistro: string | null
  title: string | null
  hasForkParent: boolean
  forkChildCount: number
  /** Cost of the parent transcript plus every sub-agent the session launched. */
  cost: SessionCostComponents | null
  /** Every model that contributed billable tokens. */
  models: string[]
  /** Parent model runs followed by runs used only by sub-agents. */
  modelRuns: ModelRunPayload[]
}

/** Identity of one local session, as the analysis view carries it. */
export interface SessionIdentityPayload {
  agent: string
  sessionId: string
  wslDistro: string | null
}

/** One end of a local fork relation. */
export interface SessionRelationPayload {
  identity: SessionIdentityPayload
  title: string | null
  available: boolean
}

/** Direct fork relations for one session. */
export interface SessionRelationsPayload {
  title: string | null
  parent: SessionRelationPayload | null
  children: SessionRelationPayload[]
}

/** One sub-agent an orchestrator launched. */
export interface SubagentMemberPayload {
  agent: string
  subagentId: string
  label: string
  /** The sub-agent's own priced cost, or null when it is not yet analyzed. */
  cost: SessionCostComponents | null
  /** Billable tokens that back `cost`. */
  tokens: BillableTokens | null
  /** Unix seconds of the sub-agent's first transcript event, or null when unknown. */
  startedAtEpoch: number | null
  /** Every model/thinking-mode pair the sub-agent used. */
  modelRuns: ModelRunPayload[]
}

/** The sub-agent picture for one session. */
export interface OrchestrationPayload {
  orchestrating: boolean
  orchestratorAgent: string
  orchestratorSessionId: string
  subagentCount: number
  members: SubagentMemberPayload[]
}

/** Everything the session-analysis surface renders for one session. */
export interface SessionAnalysisPayload {
  summary: ActiveSessionsSummary | null
  supportsAnalysis: boolean
  title: string | null
  wslDistro: string | null
  isActive: boolean
  /** Cost of the parent transcript plus every sub-agent it launched. */
  cost: SessionCostComponents | null
  /** Cost of the parent transcript, without any sub-agent. */
  topLevelCost: SessionCostComponents | null
  /** Cost of every sub-agent this session launched, combined. The value is
   * `null` when the session has no sub-agent, or when no sub-agent could
   * be priced. */
  subagentsCost: SessionCostComponents | null
  /** Billable tokens that back `cost`. The count sums the parent transcript
   * and every sub-agent. */
  inclusiveTokens: BillableTokens | null
  /** Billable tokens that back `subagentsCost`. The count sums every
   * sub-agent. The value is `null` when the session has no sub-agent. */
  subagentsTokens: BillableTokens | null
  /** Where the spend behind `cost` went. The same subject as `cost`. */
  efficiency: SessionEfficiency | null
  models: string[]
  /** Parent model runs followed by runs used only by sub-agents. */
  modelRuns: ModelRunPayload[]
  orchestration: OrchestrationPayload | null
  relations: SessionRelationsPayload | null
  /** The provider's own transcript, for the reveal action. */
  sourcePath: string | null
  /** Unix seconds of this session's own first transcript event, or null when
   * unknown. The sub-agent roster uses it to show each member's start as
   * elapsed time from the session start. */
  startedAtEpoch: number | null
  /** True when no published row set exists yet for this session, so every
   * other field above is a placeholder rather than a real read. The worker
   * fills the gap on its own; the view should show an indexing state, not
   * an empty-transcript state. */
  analysisPending: boolean
  /** True when the fields above come from a published fence that a fresher
   * pass is already queued or running behind, or whose transcript has since
   * moved on. The data on screen is real, just not the latest — unlike
   * `analysisPending`, which means there is nothing to show yet. */
  analysisStale: boolean
}

/* -------------------------------------------------------------------------
 * Session commands
 * ---------------------------------------------------------------------- */

/** The sessions to show in the popover, newest first. */
export async function listRecentSessions(windowDays?: number): Promise<ActivityEntryPayload[]> {
  if (!isTauri()) return []
  return invoke<ActivityEntryPayload[]>("list_recent_sessions", {
    windowDays: windowDays ?? null,
  })
}

/** One session's analysis, sub-agent roster, and fork relations. */
export async function getSessionAnalysis(
  agent: string,
  sessionId: string,
  wslDistro?: string | null,
): Promise<SessionAnalysisPayload | null> {
  if (!isTauri()) return null
  return invoke<SessionAnalysisPayload>("get_session_analysis", {
    agent,
    sessionId,
    wslDistro: wslDistro ?? null,
  })
}

/** One sub-agent's own analysis. */
export async function getSubagentAnalysis(
  agent: string,
  parentSessionId: string,
  subagentId: string,
  wslDistro?: string | null,
): Promise<SessionAnalysisPayload | null> {
  if (!isTauri()) return null
  return invoke<SessionAnalysisPayload>("get_subagent_analysis", {
    agent,
    parentSessionId,
    subagentId,
    wslDistro: wslDistro ?? null,
  })
}

/**
 * Delete antiburn's own records for one session.
 *
 * Only antiburn's records. The agent's transcript is never touched.
 */
export async function deleteSessionData(
  agent: string,
  sessionId: string,
  wslDistro?: string | null,
): Promise<boolean> {
  if (!isTauri()) return false
  return invoke<boolean>("delete_session_data", {
    agent,
    sessionId,
    wslDistro: wslDistro ?? null,
  })
}

/* -------------------------------------------------------------------------
 * Session lifecycle bus — mirrors `src-tauri/src/session_lifecycle.rs` and
 * `src-tauri/src/session_projection.rs`
 * ---------------------------------------------------------------------- */

const noShellUnlisten: UnlistenFn = () => undefined

/**
 * Identity of one session on the lifecycle bus. Mirrors Rust `SessionRef`
 * in `src-tauri/src/session_lifecycle.rs`.
 */
export interface SessionRefPayload {
  environmentKey: string
  agent: string
  sessionId: string
}

/** Which parts of a session row changed. Mirrors Rust `UpdateFacets`. */
export interface UpdateFacetsPayload {
  metadata: boolean
  title: boolean
  analysis: boolean
  usage: boolean
  checks: boolean
  limits: boolean
}

/** Why anonymous agent activity cleared. Mirrors Rust `AnonymousClearCause`. */
export type AnonymousClearCause = "resolved" | "expired"

/**
 * The registry's exact counts after one atomic batch. Mirrors Rust
 * `Aggregate`. `working` sessions have a write inside the quiet window,
 * `total` sessions are live (working or quiet), and `anonymous` agents have
 * unindexed activity. The counts are exact regardless of the snapshot's row
 * limit, so a reader never counts a bounded row list.
 */
export interface AggregatePayload {
  working: number
  total: number
  anonymous: number
}

/**
 * The batch counts a lifecycle envelope may carry. The registry stamps them
 * on the last lifecycle event of each atomic batch; every other event omits
 * them. A reader keeps the counts of the highest-sequence stamp it has seen.
 */
export interface LifecycleAggregateCarrier {
  aggregate?: AggregatePayload
}

/**
 * One `session:lifecycle` envelope. Mirrors Rust `LifecycleEnvelope`: the
 * flattened event plus the registry sequence a reader orders deltas by,
 * plus the batch counts on the batch's last lifecycle event.
 * A reader applies only events with a sequence above its snapshot's, and
 * re-reads the snapshot on `resync`. `activity` with a null `session` is
 * anonymous agent-level activity: a watched write the store has not
 * indexed yet. Only `anonymous_cleared` ends it: the registry clears the
 * agent when a pass covers the touch or its own quiet window passes, and no
 * reader keeps a timer. Sequences are global across every session event
 * scope, so gaps between lifecycle events are normal; only `resync` means
 * loss. `resync` comes from the bridge and never carries counts.
 */
export type SessionLifecycleEventPayload = LifecycleAggregateCarrier &
  (
    | { seq: number; kind: "started"; session: SessionRefPayload; agent: string; at: number }
    | {
        seq: number
        kind: "activity"
        session: SessionRefPayload | null
        agent: string
        at: number
        /** True when the write follows quiet or idle. */
        resumed: boolean
      }
    | { seq: number; kind: "quiet"; session: SessionRefPayload; agent: string; at: number }
    | { seq: number; kind: "idle"; session: SessionRefPayload; agent: string; at: number }
    | {
        seq: number
        kind: "anonymous_cleared"
        agent: string
        at: number
        cause: AnonymousClearCause
      }
    | { seq: number; kind: "resync" }
  )

/** One `session:updated` payload. Mirrors Rust `SessionUpdatedPayload`. */
export interface SessionUpdatedPayload {
  seq: number
  session: SessionRefPayload
  facets: UpdateFacetsPayload
  entry: ActivityEntryPayload
}

/** Why a session left the store. Mirrors Rust `RemovalReason`. */
export type SessionRemovalReason = "deleted" | "purged" | "rejected" | "reconciled"

/** Why list membership changed. Mirrors Rust `IndexChangeCause`. */
export type SessionIndexChangeCause = "scan_pass" | "invalidated" | "removed" | "resync"

/** One `session:index-changed` payload. Mirrors Rust `IndexChangedPayload`. */
export interface SessionIndexChangedPayload {
  seq: number
  cause: SessionIndexChangeCause
  session?: SessionRefPayload
  removal?: SessionRemovalReason
}

/** One live session in the registry snapshot. Mirrors Rust `LiveSession`. */
export interface LiveSessionPayload {
  session: SessionRefPayload
  agent: string
  /** Unix seconds of the session's last observed write. */
  lastActivityAt: number
  /** True when the registry has published `quiet` for that write. */
  quiet: boolean
}

/**
 * One agent with anonymous activity inside the registry's quiet window.
 * Mirrors Rust `LiveAnonymous`.
 */
export interface LiveAnonymousPayload {
  agent: string
  /** Unix seconds of the agent's last anonymous write. */
  lastActivityAt: number
}

/**
 * The versioned live-session snapshot. Mirrors Rust `LiveSnapshot`. `seq`
 * is the sequence of the last event whose effect the snapshot includes: a
 * subscriber applies only lifecycle deltas with a higher sequence.
 * `working` and `total` are exact; `sessions` holds at most the requested
 * limit of the most recent live rows, so `sessions.length < total` means
 * the rows are truncated and an omitted identity is unknown, not absent.
 * `anonymous` is complete, so a snapshot replaces the tracker's anonymous
 * state as well as its sessions.
 */
export interface LiveSnapshotPayload {
  seq: number
  working: number
  total: number
  sessions: LiveSessionPayload[]
  anonymous: LiveAnonymousPayload[]
}

/**
 * The registry's answer for named identities at one sequence. Mirrors Rust
 * `LivePresence`. Every requested identity is in exactly one list; both are
 * read under one registry lock, so they agree with `seq`.
 */
export interface LivePresencePayload {
  seq: number
  present: LiveSessionPayload[]
  absent: SessionRefPayload[]
}

/**
 * How many identities one `get_live_sessions_for` call may name. Mirrors
 * `MAX_ACTIVITY_ROWS` in `src-tauri/src/commands.rs`, the list's own row
 * bound. A larger interest set is read in bounded chunks, one at a time.
 */
export const LIVE_PRESENCE_REQUEST_LIMIT = 500

/**
 * The most recent live sessions from the lifecycle registry, bounded to
 * `limit`, with the registry sequence. A reader subscribes to
 * `session:lifecycle` first, takes this snapshot, then applies only
 * deltas with a higher sequence.
 */
export async function getLiveSessions(limit?: number): Promise<LiveSnapshotPayload | null> {
  if (!isTauri()) return null
  return invoke<LiveSnapshotPayload>("get_live_sessions", { limit: limit ?? null })
}

/**
 * The registry's state for the named identities at one sequence. A list
 * whose rows fall outside the bounded snapshot asks for them here. At most
 * {@link LIVE_PRESENCE_REQUEST_LIMIT} identities per call; the shell rejects
 * more.
 */
export async function getLiveSessionsFor(
  sessions: SessionRefPayload[],
): Promise<LivePresencePayload | null> {
  if (!isTauri()) return null
  return invoke<LivePresencePayload>("get_live_sessions_for", { sessions })
}

/**
 * Event the projection bridge emits for every transition on the session
 * lifecycle bus. Mirrors `SESSION_LIFECYCLE_EVENT` in
 * `src-tauri/src/commands.rs`. Only the bridge emits it.
 */
export const SESSION_LIFECYCLE_EVENT = "session:lifecycle"

/** Subscribe to lifecycle transitions and resync metadata. */
export async function onSessionLifecycleEvent(
  handler: (event: SessionLifecycleEventPayload) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return noShellUnlisten
  return listen<SessionLifecycleEventPayload>(SESSION_LIFECYCLE_EVENT, (event) =>
    handler(event.payload),
  )
}

/**
 * Event the projection bridge emits with one enriched row per coalesced
 * registry update. Mirrors `SESSION_UPDATED_EVENT` in
 * `src-tauri/src/commands.rs`. Replaces the removed `sessions:entry-changed`.
 */
export const SESSION_UPDATED_EVENT = "session:updated"

/** Subscribe to enriched row projections. The result unsubscribes. */
export async function onSessionUpdated(
  handler: (update: SessionUpdatedPayload) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return noShellUnlisten
  return listen<SessionUpdatedPayload>(SESSION_UPDATED_EVENT, (event) =>
    handler(event.payload),
  )
}

/**
 * Event the projection bridge emits when list membership changes: a
 * removal, a scan pass, a broad invalidation, or a resync. Mirrors
 * `SESSION_INDEX_CHANGED_EVENT` in `src-tauri/src/commands.rs`. Replaces
 * the removed `sessions:invalidated`.
 */
export const SESSION_INDEX_CHANGED_EVENT = "session:index-changed"

/** Subscribe to list-membership changes. The result unsubscribes. */
export async function onSessionIndexChanged(
  handler: (change: SessionIndexChangedPayload) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return noShellUnlisten
  return listen<SessionIndexChangedPayload>(SESSION_INDEX_CHANGED_EVENT, (event) =>
    handler(event.payload),
  )
}
