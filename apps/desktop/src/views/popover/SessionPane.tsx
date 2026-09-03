import { confirm } from "@tauri-apps/plugin-dialog"
import { useCallback } from "react"

import { SessionDetailPresentation } from "../../components/session/SessionDetailPresentation"
import type { TokensCostSplit } from "../../components/session/tokensCard"
import { renderAgentIcon } from "../../lib/agentIcon"
import {
  deleteSessionData,
  revealSource,
  withPopoverHold,
  type SessionAnalysisPayload,
} from "../../lib/ipc"
import { agentSupportsAnalysis } from "../../lib/presentation/agents"
import {
  inclusiveCostSubject,
  subagentsCostSubject,
  topLevelCostSubject,
  type LocalSessionCost,
} from "../../lib/presentation/sessionCosts"
import { sessionHygieneFor, useSessionHygiene } from "../../lib/useSessionHygiene"
import type {
  BillableTokens,
  LocalSessionRelation,
  LocalSessionRelations,
} from "../../lib/types/session"

/**
 * One session's analysis, loaded and wired to the actions a reader can take.
 *
 * The presentation component owns every pixel; this owns the data it renders,
 * the navigation between sessions, and the destructive delete action, spelled
 * out below because it is easy to get subtly wrong.
 */

/** Which session the pane is showing. */
export interface SessionSubject {
  agent: string
  sessionId: string
  repo?: string | undefined
  timestamp?: string | undefined
  wslDistro?: string | null | undefined
  title?: string | undefined
  /** Present when the subject is a sub-agent rather than a session a reader drove. */
  subagent?: {
    parentSessionId: string
    subagentId: string
    parentTitle?: string
  }
}

export interface SessionPaneProps {
  subject: SessionSubject
  /** The subject's loaded analysis, or null while loading or on failure. */
  payload: SessionAnalysisPayload | null
  /** Whether `payload` belongs to a load still in flight for this subject. */
  loading: boolean
  /** Whether a re-load is in flight while `payload` is still on screen. */
  refreshing: boolean
  /** Whether the load for this subject failed. */
  error: boolean
  onBack: () => void
  /** Newer adjacent session; omitted when there is none. */
  onPrev?: (() => void) | undefined
  /** Older adjacent session; omitted when there is none. */
  onNext?: (() => void) | undefined
  /** Navigate to another session (a fork, a sub-agent, an orchestrator). */
  onOpenSession: (subject: SessionSubject) => void
  /** The session's local records were deleted, so it can no longer be shown. */
  onDeleted: () => void
}

/** All-zero token counts. Use this value when a subject has no billable-token summary. */
const ZERO_TOKENS: BillableTokens = {
  inputTokens: 0,
  outputTokens: 0,
  cacheReadTokens: 0,
  cacheCreationTokens: 0,
}

/** One priced result for a cost subject. The function returns null when nothing priced the subject. */
function localCost(
  subject: LocalSessionCost["subject"],
  cost: SessionAnalysisPayload["cost"],
  tokens: BillableTokens,
  model: string | null,
  isActive: boolean,
): LocalSessionCost | null {
  if (!cost) return null
  return {
    subject,
    ...tokens,
    totalTokens:
      tokens.inputTokens +
      tokens.outputTokens +
      tokens.cacheReadTokens +
      tokens.cacheCreationTokens,
    inputCostUsd: cost.inputUsd,
    outputCostUsd: cost.outputUsd,
    cacheReadCostUsd: cost.cacheReadUsd,
    cacheWriteCostUsd: cost.cacheWriteUsd,
    totalCostUsd: cost.totalUsd,
    model,
    isActive,
  }
}

/**
 * The cost result the breakdown card describes. The function also returns
 * the parent/sub-agent split alongside it.
 *
 * A sub-agent launches no sub-agent of its own. Its own transcript is the
 * whole story. The headline shows its own (`topLevel`) cost. The headline
 * pairs that cost with its own tokens.
 *
 * An orchestrator's headline is `inclusive`. It covers every sub-agent the
 * session launched. The headline pairs the inclusive dollar total with the
 * inclusive token total.
 *
 * The split below the headline shows two rows. The parent row pairs the
 * parent's own tokens with the parent's own dollars. The sub-agents row
 * pairs the sub-agents' tokens with the sub-agents' dollars.
 */
function toLocalCost(
  subject: SessionSubject,
  payload: SessionAnalysisPayload,
): { cost: LocalSessionCost | null; costSplit: TokensCostSplit | null } {
  const metrics = payload.summary?.sessions[0]
  const parentTokens: BillableTokens = {
    inputTokens: metrics?.billableInputTokens ?? 0,
    outputTokens: metrics?.billableOutputTokens ?? 0,
    cacheReadTokens: metrics?.billableCacheReadTokens ?? 0,
    cacheCreationTokens: metrics?.billableCacheCreationTokens ?? 0,
  }
  const model = metrics?.model ?? null

  if (subject.subagent) {
    const cost = localCost(
      topLevelCostSubject(subject.agent, subject.sessionId, subject.wslDistro),
      payload.cost,
      parentTokens,
      model,
      payload.isActive,
    )
    return { cost, costSplit: null }
  }

  const cost = localCost(
    inclusiveCostSubject(subject.agent, subject.sessionId, subject.wslDistro),
    payload.cost,
    payload.inclusiveTokens ?? ZERO_TOKENS,
    model,
    payload.isActive,
  )

  const subagentCount = payload.orchestration?.subagentCount ?? 0
  const parent =
    subagentCount > 0
      ? localCost(
          topLevelCostSubject(subject.agent, subject.sessionId, subject.wslDistro),
          payload.topLevelCost,
          parentTokens,
          model,
          payload.isActive,
        )
      : null
  const subagents =
    subagentCount > 0
      ? localCost(
          subagentsCostSubject(subject.agent, subject.sessionId, subject.wslDistro),
          payload.subagentsCost,
          payload.subagentsTokens ?? ZERO_TOKENS,
          null,
          payload.isActive,
        )
      : null

  return {
    cost,
    costSplit:
      parent && subagents
        ? {
            parent,
            subagents,
            subagentCount,
            members: payload.orchestration?.members ?? [],
            sessionStartedAtEpoch: payload.startedAtEpoch,
          }
        : null,
  }
}

export function SessionPane({
  subject,
  payload,
  loading,
  refreshing,
  error,
  onBack,
  onPrev,
  onNext,
  onOpenSession,
  onDeleted,
}: SessionPaneProps) {
  /**
   * Delete: antiburn's own records only.
   *
   * The copy says so explicitly, because "delete session" in an app that reads
   * someone else's files could reasonably be read as deleting the conversation.
   * It does not: the agent's transcript is the agent's.
   */
  const handleDelete = useCallback(async () => {
    const proceed = await withPopoverHold(() =>
      confirm(
        "This removes antiburn’s stored analysis for the session. The agent’s own transcript file is not touched, and a later scan will find the session again.",
        { title: "Remove this session from antiburn?", kind: "warning", okLabel: "Remove" },
      ),
    )
    if (!proceed) return
    await deleteSessionData(subject.agent, subject.sessionId, subject.wslDistro)
    onDeleted()
  }, [subject, onDeleted])

  const sourcePath = payload?.sourcePath ?? null
  const handleReveal = useCallback(() => {
    if (!sourcePath) return
    void revealSource(sourcePath)
  }, [sourcePath])

  const hygieneIdentity = {
    agent: subject.agent,
    sessionId: subject.sessionId,
    wslDistro: subject.wslDistro ?? null,
  }
  const hygieneBySession = useSessionHygiene([hygieneIdentity])
  const hygiene = sessionHygieneFor(hygieneBySession, hygieneIdentity)
  const { cost, costSplit } = payload
    ? toLocalCost(subject, payload)
    : { cost: null, costSplit: null }
  const relations: LocalSessionRelations | null = payload?.relations ?? null
  // The stored title is the authority once it arrives; the one the list handed
  // over is what keeps the header from being blank in the meantime.
  const title = payload?.title ?? subject.title ?? undefined

  const openRelated = useCallback(
    (target: LocalSessionRelation, title: string) => {
      onOpenSession({
        agent: target.identity.agent,
        sessionId: target.identity.sessionId,
        wslDistro: target.identity.wslDistro ?? null,
        title,
      })
    },
    [onOpenSession],
  )

  const openSubagent = useCallback(
    (subagentId: string, label: string) => {
      onOpenSession({
        agent: subject.agent,
        sessionId: subagentId,
        ...(subject.repo ? { repo: subject.repo } : {}),
        ...(subject.timestamp ? { timestamp: subject.timestamp } : {}),
        wslDistro: subject.wslDistro ?? null,
        title: label,
        subagent: {
          parentSessionId: subject.sessionId,
          subagentId,
          ...(subject.title ? { parentTitle: subject.title } : {}),
        },
      })
    },
    [
      onOpenSession,
      subject.agent,
      subject.repo,
      subject.sessionId,
      subject.timestamp,
      subject.title,
      subject.wslDistro,
    ],
  )

  const openOrchestrator = useCallback(() => {
    if (!subject.subagent) return
    onOpenSession({
      agent: subject.agent,
      sessionId: subject.subagent.parentSessionId,
      ...(subject.repo ? { repo: subject.repo } : {}),
      ...(subject.timestamp ? { timestamp: subject.timestamp } : {}),
      wslDistro: subject.wslDistro ?? null,
      ...(subject.subagent.parentTitle ? { title: subject.subagent.parentTitle } : {}),
    })
  }, [onOpenSession, subject])

  return (
    <SessionDetailPresentation
      summary={payload?.summary ?? null}
      loading={loading}
      hygiene={hygiene}
      refreshing={refreshing}
      error={error}
      session={{
        agent: subject.agent,
        sessionId: subject.sessionId,
        ...(subject.repo ? { repo: subject.repo } : {}),
        ...(subject.timestamp ? { timestamp: subject.timestamp } : {}),
        ...(title ? { title } : {}),
        wslDistro: subject.wslDistro ?? null,
        ...(subject.subagent
          ? {
              subagent: subject.subagent.parentTitle
                ? { parentTitle: subject.subagent.parentTitle }
                : {},
            }
          : {}),
      }}
      supportsAnalysis={payload?.supportsAnalysis ?? agentSupportsAnalysis(subject.agent)}
      analysisPending={payload?.analysisPending ?? false}
      cost={cost}
      costSplit={costSplit}
      efficiency={payload?.efficiency ?? null}
      subagentCount={payload?.orchestration?.subagentCount ?? 0}
      modelRuns={payload?.modelRuns ?? []}
      relations={relations}
      onBack={onBack}
      {...(onPrev ? { onPrev } : {})}
      {...(onNext ? { onNext } : {})}
      onOpenSubagent={openSubagent}
      onOpenOrchestrator={openOrchestrator}
      onOpenRelatedSession={openRelated}
      onDeleteSession={() => void handleDelete()}
      {...(sourcePath ? { onRevealSource: handleReveal } : {})}
      renderAgentIcon={renderAgentIcon}
    />
  )
}
