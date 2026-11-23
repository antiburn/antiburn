/**
 * This module defines session IPC payloads, commands, and lifecycle subscriptions.
 * `ipc.ts` re-exports them to preserve the shared import path.
 */

import { invoke, isTauri } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "./tauriEvents"

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
interface ModelRunPayload {
  model: string
  nativeVariant?: string
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
  remoteHostId?: string | null
  title: string | null
  hasForkParent: boolean
  forkChildCount: number
  /** Cost of the parent transcript plus every sub-agent the session launched. */
  cost: SessionCostComponents | null
  /** Input, cache-creation, output, and cache-read tokens, summed across
   *  every model. The count covers every sub-agent the session launched. */
  totalTokens: number
  /** Every model that contributed billable tokens. */
  models: string[]
  /** Parent model runs followed by runs used only by sub-agents. */
  modelRuns: ModelRunPayload[]
}

/** Identity of one local session, as the analysis view carries it. */
interface SessionIdentityPayload {
  agent: string
  sessionId: string
  wslDistro: string | null
  remoteHostId?: string | null
}

/** One end of a local fork relation. */
interface SessionRelationPayload {
  identity: SessionIdentityPayload
  title: string | null
  available: boolean
}

/** Direct fork relations for one session. */
interface SessionRelationsPayload {
  title: string | null
  parent: SessionRelationPayload | null
  children: SessionRelationPayload[]
}

/** One sub-agent an orchestrator launched. */
interface SubagentMemberPayload {
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
interface OrchestrationPayload {
  orchestrating: boolean
  orchestratorAgent: string
  orchestratorSessionId: string
  subagentCount: number
  members: SubagentMemberPayload[]
}

/**
 * One tool or skill a session used, with how many times it used it. A
 * `skill` entry counts skill uses, not calls of a tool with that name.
 */
export interface CalledToolPayload {
  name: string
  kind: "tool" | "skill"
  calls: number
}

/** Everything the session-analysis surface renders for one session. */
export interface SessionAnalysisPayload {
  summary: ActiveSessionsSummary | null
  supportsAnalysis: boolean
  title: string | null
  wslDistro: string | null
  remoteHostId?: string | null
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
  /** The stored absolute working directory. */
  projectPath: string | null
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
  /** The tools this session called, most-called first, bounded by the
   * backend. Null when the source records no tool evidence. The Tools tab
   * falls back to this list for an agent that records no startup context. */
  calledTools: CalledToolPayload[] | null
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

export interface SessionActivityBatch {
  entries: ActivityEntryPayload[]
  exhausted: boolean
  agents?: string[] | null
  /** Indexed sessions in the selected date range. Supplied by the head reads. */
  total?: number | null
  reset?: boolean
  /** The native stream already completed this refresh sequence. */
  replayedRefresh?: boolean
  /** Next native read after a reset discards an unacknowledged append. */
  nextReadSequence?: number
}

/** One demand-driven native stream. Call one operation at a time. */
export interface SessionActivityStream {
  read(): Promise<SessionActivityBatch>
  refresh(reconcile?: boolean): Promise<SessionActivityBatch>
  close(): void
}

/** A session stream command has no reply after its allowed attempts. */
export class SessionActivityTimeout extends Error {}

/** Create a stream that opens on first demand. The shell owns the scan position. */
export function openSessionActivityStream(windowDays: number): SessionActivityStream {
  const id = globalThis.crypto.getRandomValues(new Uint32Array(4)).join("-")
  let opened = false
  let closed = false
  let readSequence = 0
  let refreshSequence = 0
  let cancelRead: (() => void) | undefined

  function close() {
    if (closed) return
    closed = true
    cancelRead?.()
    if (isTauri())
      void invoke("session_activity_stream", { request: { kind: "close", id } }).catch(() => {})
  }

  async function request(
    operation: { kind: "read" } | { kind: "refresh"; reconcile: boolean },
  ): Promise<SessionActivityBatch> {
    if (closed || cancelRead) throw new Error("Session activity stream is unavailable")
    if (!isTauri()) return { entries: [], exhausted: true, agents: [] }

    const continuation = opened && operation.kind === "read"
    const nativeRequest = !opened
      ? { kind: "open", id, windowDays }
      : continuation
        ? { kind: "read", id, sequence: readSequence }
        : { ...operation, id, sequence: refreshSequence }
    const interruption = new Promise<never>((_, reject) => {
      cancelRead = () => reject(new Error("Session activity stream is closed"))
    })
    async function readAttempt() {
      if (closed) throw new Error("Session activity stream is closed")
      let timer: ReturnType<typeof setTimeout> | undefined
      try {
        return await Promise.race([
          invoke<SessionActivityBatch>("session_activity_stream", { request: nativeRequest }),
          interruption,
          new Promise<never>((_, reject) => {
            timer = setTimeout(
              () => reject(new SessionActivityTimeout("Session activity read timed out")),
              15_000,
            )
          }),
        ])
      } finally {
        clearTimeout(timer)
      }
    }
    try {
      const batch = await readAttempt().catch(async (error: unknown) => {
        if (closed || !continuation) throw error
        // Reuse the sequence so a retry cannot advance the cursor twice.
        let retryTimer: ReturnType<typeof setTimeout> | undefined
        try {
          await Promise.race([
            new Promise<void>((resolve) => {
              retryTimer = setTimeout(resolve, 250)
            }),
            interruption,
          ])
        } finally {
          clearTimeout(retryTimer)
        }
        return readAttempt()
      })
      opened = true
      if (continuation) readSequence += 1
      if (nativeRequest.kind === "refresh") {
        refreshSequence += 1
        if (batch.nextReadSequence !== undefined) readSequence = batch.nextReadSequence
      }
      return batch
    } catch (error) {
      // An open has no established position to preserve.
      if (nativeRequest.kind === "open" && error instanceof SessionActivityTimeout) close()
      throw error
    } finally {
      cancelRead = undefined
    }
  }

  return {
    read: () => request({ kind: "read" }),
    refresh: (reconcile = false) => request({ kind: "refresh", reconcile }),
    close,
  }
}

/** Local and WSL rows for Overview, filtered before the backend row limit. */
export async function listOverviewSessions(
  windowDays?: number,
): Promise<ActivityEntryPayload[]> {
  if (!isTauri()) return []
  return invoke<ActivityEntryPayload[]>("list_recent_sessions", {
    windowDays: windowDays ?? null,
    localOnly: true,
  })
}

/** One session's analysis, sub-agent roster, and fork relations. */
export async function getSessionAnalysis(
  agent: string,
  sessionId: string,
  wslDistro?: string | null,
  remoteHostId?: string | null,
): Promise<SessionAnalysisPayload | null> {
  if (!isTauri()) return null
  return invoke<SessionAnalysisPayload>("get_session_analysis", {
    agent,
    sessionId,
    wslDistro: wslDistro ?? null,
    remoteHostId: remoteHostId ?? null,
  })
}

/** One sub-agent's own analysis. */
export async function getSubagentAnalysis(
  agent: string,
  parentSessionId: string,
  subagentId: string,
  wslDistro?: string | null,
  remoteHostId?: string | null,
): Promise<SessionAnalysisPayload | null> {
  if (!isTauri()) return null
  return invoke<SessionAnalysisPayload>("get_subagent_analysis", {
    agent,
    parentSessionId,
    subagentId,
    wslDistro: wslDistro ?? null,
    remoteHostId: remoteHostId ?? null,
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
  remoteHostId?: string | null,
): Promise<boolean> {
  if (!isTauri()) return false
  return invoke<boolean>("delete_session_data", {
    agent,
    sessionId,
    wslDistro: wslDistro ?? null,
    remoteHostId: remoteHostId ?? null,
  })
}

/* -------------------------------------------------------------------------
 * Session lifecycle bus — mirrors `src-tauri/src/session_lifecycle.rs` and
 * `src-tauri/src/session_projection.rs`
 * ---------------------------------------------------------------------- */

const noShellUnlisten: UnlistenFn = () => undefined

/** This identity matches Rust `SessionRef` on the lifecycle bus. */
export interface SessionRefPayload {
  environmentKey: string
  agent: string
  sessionId: string
  remoteHostId?: string | null
}

/** These facets match Rust `UpdateFacets` and name changed row data. */
export interface UpdateFacetsPayload {
  metadata: boolean
  title: boolean
  analysis: boolean
  usage: boolean
  checks: boolean
  limits: boolean
}

/**
 * This cause matches Rust `AnonymousClearCause` and identifies why anonymous activity
 * ends.
 */
type AnonymousClearCause = "resolved" | "expired"

/** The newest published modeled turn supplies execution metadata independently of the harness. */
interface ExecutionMetadataPayload {
  model: string
  /** This field preserves the same turn's provider without alias rewriting. */
  recordedProvider: string | null
  /** Only recorded route normalization supplies this field; missing and custom routes remain null. */
  providerRoute: string | null
  /** A recognized model family identifies the vendor, not the billing route. */
  modelVendor: string | null
}

/** These exact harness counts cover all working identities, independently of snapshot row limits. */
export interface SweepCountsPayload {
  agent: string
  working: number
  anonymous: number
  modelPendingWorking: number
  modelFailedWorking: number
  modelNoneWorking: number
  models: (ExecutionMetadataPayload & { working: number })[]
}

export interface AggregatePayload {
  working: number
  total: number
  anonymous: number
  sweep: SweepCountsPayload[]
}

/**
 * The last lifecycle event of an atomic batch carries exact counts. Readers retain the
 * counts with the highest sequence.
 */
interface LifecycleAggregateCarrier {
  aggregate?: AggregatePayload
}

/**
 * This envelope matches Rust `LifecycleEnvelope`. Readers apply deltas above their base
 * sequence. Sequence gaps are normal across the three session scopes. Only the bridge
 * requests recovery through `resync`. Recovery markers carry no counts. A null activity
 * session identifies anonymous activity. Only registry covers and deadlines clear that
 * state.
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
        /** The write resumes activity after quiet or idle. */
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
    | { seq: number; kind: "sweep_changed" }
  )

/** This row projection matches Rust `SessionUpdatedPayload`. */
export interface SessionUpdatedPayload {
  seq: number
  session: SessionRefPayload
  facets: UpdateFacetsPayload
  entry: ActivityEntryPayload
}

/** This removal reason matches Rust `RemovalReason`. */
type SessionRemovalReason = "deleted" | "purged" | "rejected" | "reconciled"

/** This index change cause matches Rust `IndexChangeCause`. */
type SessionIndexChangeCause = "scan_pass" | "invalidated" | "removed" | "resync"

/** This index notification matches Rust `IndexChangedPayload`. */
export interface SessionIndexChangedPayload {
  seq: number
  cause: SessionIndexChangeCause
  session?: SessionRefPayload
  removal?: SessionRemovalReason
}

/** This snapshot row matches Rust `LiveSession`. */
export interface LiveSessionPayload {
  execution?: ExecutionMetadataPayload | null
  session: SessionRefPayload
  agent: string
  /** The timestamp records session activity in Unix seconds. */
  lastActivityAt: number
  /** The registry sets this flag when the session becomes quiet. */
  quiet: boolean
}

/** This anonymous agent state matches Rust `LiveAnonymous`. */
export interface LiveAnonymousPayload {
  agent: string
  /** The timestamp records anonymous activity in Unix seconds. */
  lastActivityAt: number
}

/**
 * This snapshot matches Rust `LiveSnapshot` at `seq`. Working and total counts remain
 * exact. A truncated row list leaves omitted identities unknown, not absent. The
 * complete anonymous list replaces prior anonymous state.
 */
export interface LiveSnapshotPayload {
  seq: number
  working: number
  total: number
  sessions: LiveSessionPayload[]
  anonymous: LiveAnonymousPayload[]
  sweep: SweepCountsPayload[]
}

/**
 * This named presence result matches Rust `LivePresence`. One registry lock protects
 * both lists at the same sequence. Each requested identity appears in exactly one list.
 */
export interface LivePresencePayload {
  seq: number
  present: LiveSessionPayload[]
  absent: SessionRefPayload[]
}

/**
 * Each presence request names at most this many identities. The limit matches Rust
 * `MAX_ACTIVITY_ROWS`. Larger interest unions require sequential bounded requests.
 */
export const LIVE_PRESENCE_REQUEST_LIMIT = 500

/**
 * Read bounded recent rows with the canonical registry sequence. Subscribe to lifecycle
 * events before requesting this snapshot.
 */
export async function getLiveSessions(limit?: number): Promise<LiveSnapshotPayload | null> {
  if (!isTauri()) return null
  return invoke<LiveSnapshotPayload>("get_live_sessions", { limit: limit ?? null })
}

/**
 * Read named presence for identities the bounded snapshot omits. The shell rejects
 * requests above {@link LIVE_PRESENCE_REQUEST_LIMIT}.
 */
export async function getLiveSessionsFor(
  sessions: SessionRefPayload[],
): Promise<LivePresencePayload | null> {
  if (!isTauri()) return null
  return invoke<LivePresencePayload>("get_live_sessions_for", { sessions })
}

/**
 * Only the projection bridge emits this lifecycle scope. Its name matches Rust
 * `SESSION_LIFECYCLE_EVENT`.
 */
const SESSION_LIFECYCLE_EVENT = "session:lifecycle"

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
 * Only the projection bridge emits enriched rows on this scope. Its name matches Rust
 * `SESSION_UPDATED_EVENT`.
 */
const SESSION_UPDATED_EVENT = "session:updated"

/** Subscribe to enriched row projections. The result unsubscribes. */
export async function onSessionUpdated(
  handler: (update: SessionUpdatedPayload) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return noShellUnlisten
  return listen<SessionUpdatedPayload>(SESSION_UPDATED_EVENT, (event) => handler(event.payload))
}

/**
 * Only the projection bridge emits index changes on this scope. Its name matches Rust
 * `SESSION_INDEX_CHANGED_EVENT`.
 */
const SESSION_INDEX_CHANGED_EVENT = "session:index-changed"

/** Subscribe to list-membership changes. The result unsubscribes. */
export async function onSessionIndexChanged(
  handler: (change: SessionIndexChangedPayload) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return noShellUnlisten
  return listen<SessionIndexChangedPayload>(SESSION_INDEX_CHANGED_EVENT, (event) =>
    handler(event.payload),
  )
}
