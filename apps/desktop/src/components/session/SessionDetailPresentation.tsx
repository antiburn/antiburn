// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import * as DropdownMenu from "@radix-ui/react-dropdown-menu"
import {
  ChevronLeft,
  ChevronRight,
  FolderOpen,
  GitBranchPlus,
  GitFork,
  Info,
  Moon,
  Share,
  Trash2,
} from "lucide-react"
import { useCallback, useState, useSyncExternalStore, type ReactNode } from "react"

import { cn } from "../../lib/cn"
import { agentDisplayName } from "../../lib/presentation/agents"
import { sessionIdentityKey } from "../../lib/presentation/localIdentity"
import { mockSessionHygiene } from "../../lib/presentation/mockSessionHygiene"
import { modelRunNames, modelRunShortNames } from "../../lib/presentation/models"
import { relativeTime } from "../../lib/presentation/relativeTime"
import {
  costBreakdownRows,
  costFigureLabel,
  formatCompact,
  formatDuration,
  formatPct,
  isEmptySummary,
  PHASES,
  phaseBreakdown,
} from "../../lib/presentation/sessionAnalytics"
import { resultComponentCost, type LocalSessionCost } from "../../lib/presentation/sessionCosts"
import type {
  ActiveSessionsSummary,
  LocalOrchestrationStatus,
  LocalSessionRelation,
  LocalSessionRelations,
  SessionPhase,
  SkillDetail,
} from "../../lib/types/session"
import { useGlobalKeydown } from "../../lib/useGlobalKeydown"
import { Tooltip } from "../presentation/Tooltip"
import { TruncatedText } from "../presentation/TruncatedText"
import { WslOriginBadge } from "../presentation/WslOriginBadge"
import { Skeleton } from "../ui/Skeleton"
import { ContextDriftChart } from "./analytics/ContextDriftChart"
import { CostBreakdown } from "./analytics/CostBreakdown"
import { Hypnogram } from "./analytics/Hypnogram"
import { HypnogramSegments, type MarkerLayer } from "./analytics/HypnogramSegments"
import { InitialContextChart } from "./analytics/InitialContextChart"
import type { TimelineMarker } from "./analytics/markerCluster"
import { PatternScore } from "./analytics/PatternScore"
import { PhaseDonut } from "./analytics/PhaseDonut"
import { skillMarkerKind } from "./analytics/skillMarkerKind"
import { createSpawnMarkerKind, type SpawnPayload } from "./analytics/spawnMarkerKind"
import { TokenAreaChart } from "./analytics/TokenAreaChart"
import { ToolMixChart } from "./analytics/ToolMixChart"
import { SessionCostBadge } from "./metrics/SessionCostBadge"
import { OrchestratedBadge } from "./orchestration/OrchestratedBadge"
import type { AgentIconRenderer } from "./orchestration/SubagentRosterRow"
import { SubagentBadge } from "./orchestration/SubagentBadge"
import { tokensCardModel, type TokensCostSplit } from "./tokensCard"

/**
 * Skeleton anti-flash timing. A fast load finishes before the delay elapses,
 * so the placeholder never renders — no jarring flash and no added latency.
 * Once it does appear it stays for at least the floor, so it cannot flicker on
 * and off in the medium window.
 */
const SKELETON_DELAY_MS = 200
const SKELETON_MIN_VISIBLE_MS = 400

/** The session this view shows. */
interface SessionDetailSubject {
  agent: string
  sessionId: string
  repo?: string
  timestamp?: string
  title?: string
  wslDistro: string | null
  /**
   * Present when the view is showing a sub-agent rather than a session the
   * user drove themselves.
   */
  subagent?: {
    /** Title of the orchestrator that launched it, for the provenance badge. */
    parentTitle?: string
  }
}

export interface SessionDetailPresentationProps {
  /** The analysis to render; null while loading or after a failure. */
  summary: ActiveSessionsSummary | null
  /** Whether the analysis is still being produced. */
  loading: boolean
  /** Whether producing it failed. */
  error: boolean
  /** The session this view describes. */
  session: SessionDetailSubject
  /**
   * False when the engine has no adapter for this agent, which changes the
   * empty state from "nothing happened" to "we cannot read this yet".
   */
  supportsAnalytics: boolean

  /** The cost result the breakdown describes, when one was priced. */
  cost: LocalSessionCost | null
  /** Parent/sub-agents split for an orchestration total. */
  costSplit: TokensCostSplit | null
  /** Sub-agents this session launched. */
  orchestration: LocalOrchestrationStatus | null
  /** Skill invocations to overlay on the per-turn ribbon. */
  skills: SkillDetail[]
  /** Models that contributed to this session. */
  models: string[]
  /** Direct fork relations resolved from local transcripts. */
  relations: LocalSessionRelations | null
  onBack: () => void
  /** Navigate to the newer adjacent session; omit when none exists. */
  onPrev?: () => void
  /** Navigate to the older adjacent session; omit when none exists. */
  onNext?: () => void
  /** Open one sub-agent's analytics from the roster or a spawn marker. */
  onOpenSubagent: (subagentId: string, label: string) => void
  /** From a sub-agent view, open the launching orchestrator's analytics. */
  onOpenOrchestrator: () => void
  /** Open a fork parent or child. */
  onOpenRelatedSession: (target: LocalSessionRelation, title: string) => void

  /** Export this session's analysis. */
  onExportSession: () => void
  /** Delete this session's local record. */
  onDeleteSession: () => void
  /** Reveal the session's transcript on disk. Omitted hides the control. */
  onRevealSource?: () => void
  renderAgentIcon: AgentIconRenderer
}

/* -------------------------------------------------------------------------
 * Internal pieces
 * ---------------------------------------------------------------------- */

function RelationControl({
  relations,
  onOpen,
}: {
  relations: LocalSessionRelations
  onOpen: (target: LocalSessionRelation, title: string) => void
}) {
  const titleFor = (target: LocalSessionRelation) => {
    if (target.title) return target.title
    return `Session ${target.identity.sessionId.slice(0, 7)}`
  }
  const { parent, children } = relations
  const soleChild = children.length === 1 ? children[0] : undefined

  return (
    <div className="flex shrink-0 items-center gap-0.5">
      {parent && parent.available && (
        <Tooltip label={`Open parent: ${titleFor(parent)}`}>
          <button
            type="button"
            onClick={() => onOpen(parent, titleFor(parent))}
            className="rounded-md p-1 text-label-tertiary hover:bg-surface-tertiary hover:text-label-secondary"
            aria-label="Open fork parent"
          >
            <GitFork size={14} aria-hidden="true" />
          </button>
        </Tooltip>
      )}
      {parent && !parent.available && (
        <Tooltip label="Parent transcript is no longer on this machine">
          <span
            className="p-1 text-label-tertiary opacity-40"
            aria-label="Fork parent is unavailable locally"
          >
            <GitFork size={14} aria-hidden="true" />
          </span>
        </Tooltip>
      )}

      {soleChild && soleChild.available && (
        <Tooltip label={`Open fork: ${titleFor(soleChild)}`}>
          <button
            type="button"
            onClick={() => onOpen(soleChild, titleFor(soleChild))}
            className="rounded-md p-1 text-label-tertiary hover:bg-surface-tertiary hover:text-label-secondary"
            aria-label="Open forked child"
          >
            <GitBranchPlus size={14} aria-hidden="true" />
          </button>
        </Tooltip>
      )}

      {children.length > 0 && !(soleChild && soleChild.available) && (
        <DropdownMenu.Root>
          <DropdownMenu.Trigger asChild>
            <button
              type="button"
              className="inline-flex items-center gap-0.5 rounded-md px-1 py-0.5 type-caption text-label-tertiary hover:bg-surface-tertiary hover:text-label-secondary"
              aria-label={`Show ${children.length} direct forks`}
            >
              <GitBranchPlus size={14} aria-hidden="true" />
              {children.length}
            </button>
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <DropdownMenu.Content
              className="ui-menu w-[250px]"
              side="bottom"
              align="end"
              sideOffset={4}
              collisionPadding={8}
            >
              <div className="px-2 py-1 type-caption text-label-tertiary">Direct forks</div>
              {children.map((child) => {
                const title = titleFor(child)
                return (
                  <DropdownMenu.Item
                    key={sessionIdentityKey(child.identity)}
                    className="ui-menu-item truncate"
                    disabled={!child.available}
                    onSelect={() => child.available && onOpen(child, title)}
                  >
                    {title}
                    {!child.available && (
                      <span className="ml-auto pl-2 text-label-tertiary">unavailable</span>
                    )}
                  </DropdownMenu.Item>
                )
              })}
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
      )}
    </div>
  )
}

function Card({
  title,
  info,
  hint,
  children,
}: {
  title: string
  info?: string
  hint?: string
  children: ReactNode
}) {
  return (
    <div className="mx-3 mb-3 rounded-xl bg-surface-secondary/40 p-3">
      <div className="mb-2 flex items-baseline justify-between">
        <div className="flex items-center gap-1">
          <h3 className="type-headline text-label">{title}</h3>
          {info && (
            <Tooltip label={info} side="top" interactive delayMs={150}>
              <button
                type="button"
                aria-label={`About ${title}`}
                className="leading-none text-label-tertiary transition-colors hover:text-label-secondary"
              >
                <Info size={13} aria-hidden="true" />
              </button>
            </Tooltip>
          )}
        </div>
        {hint && <span className="type-caption text-label-tertiary">{hint}</span>}
      </div>
      {children}
    </div>
  )
}

function Legend() {
  // Reversed so the swatches read left → right in the same order the hypnogram
  // stacks its lanes top → bottom. `PHASES` is shared, so copy before reversing.
  return (
    <div className="mb-2 flex flex-nowrap gap-x-2">
      {[...PHASES].reverse().map((phase) => (
        <span
          key={phase.key}
          className="flex items-center gap-1 whitespace-nowrap type-caption text-label-secondary"
        >
          <span className="h-2 w-2 rounded-full" style={{ backgroundColor: phase.colorVar }} />
          {phase.label}
        </span>
      ))}
    </div>
  )
}

function PhaseBreakdownRows({
  summary,
  activePhase,
  onHoverPhase,
}: {
  summary: ActiveSessionsSummary
  activePhase?: SessionPhase | null
  onHoverPhase?: (phase: SessionPhase | null) => void
}) {
  return (
    <div className="mt-1 space-y-1.5">
      {phaseBreakdown(summary).map((row) => (
        <div
          key={row.key}
          className={cn(
            "-mx-2 flex items-center gap-2.5 rounded-lg px-2 py-1 transition-colors",
            activePhase === row.key && "bg-surface-secondary",
          )}
          onMouseEnter={() => onHoverPhase?.(row.key)}
          onMouseLeave={() => onHoverPhase?.(null)}
        >
          <span
            className="h-7 w-1 shrink-0 rounded-full"
            style={{ backgroundColor: row.colorVar }}
          />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-1.5">
              <span className="type-callout text-label">{row.label}</span>
              <Tooltip label={row.tip} side="top" interactive delayMs={150}>
                <button
                  type="button"
                  aria-label={`About ${row.label}`}
                  className="leading-none text-label-tertiary transition-colors hover:text-label-secondary"
                >
                  <Info size={11} aria-hidden="true" />
                </button>
              </Tooltip>
              {row.flag === "high" && (
                <span className="rounded bg-system-orange-tint/15 px-1 type-caption font-medium text-system-orange">
                  High
                </span>
              )}
              {row.flag === "low" && (
                <span className="rounded bg-surface-secondary px-1 type-caption font-medium text-label-tertiary">
                  Low
                </span>
              )}
            </div>
            <span className="type-caption text-label-tertiary">
              Reference: {formatPct(row.reference.lo)}–{formatPct(row.reference.hi)}
            </span>
          </div>
          <div className="shrink-0 text-right">
            <div className="type-headline text-label tabular-nums">
              {formatPct(row.fraction)}
            </div>
            <div className="type-caption text-label-tertiary">{row.minutes} min</div>
          </div>
        </div>
      ))}
    </div>
  )
}

/** Card shell matching {@link Card}, with a placeholder in place of the heading. */
function SkeletonCardShell({ children }: { children: ReactNode }) {
  return (
    <div className="mx-3 mb-3 rounded-xl bg-surface-secondary/40 p-3">
      <Skeleton className="mb-3 h-3.5 w-24" />
      {children}
    </div>
  )
}

/**
 * Structural placeholder for the first load. Mirrors the real header and card
 * layout, so the wait reads as progress with minimal layout shift. The circles
 * are plain spans rather than `Skeleton` so their round radius is not subject
 * to the primitive's own.
 */
function SessionDetailSkeleton() {
  return (
    <div aria-hidden data-testid="session-analytics-skeleton">
      <div className="flex items-start gap-3 px-4 pb-3">
        <Skeleton className="mt-0.5 h-5 w-5 shrink-0" />
        <div className="min-w-0 flex-1 space-y-2">
          <Skeleton className="h-4 w-40" />
          <Skeleton className="h-3 w-28" />
        </div>
      </div>

      <SkeletonCardShell>
        <Skeleton className="h-[120px] w-full rounded-lg" />
        <div className="mt-2 flex justify-between">
          <Skeleton className="h-2.5 w-8" />
          <Skeleton className="h-2.5 w-8" />
        </div>
      </SkeletonCardShell>

      <SkeletonCardShell>
        <div className="mb-3 flex justify-center">
          <span className="block h-[120px] w-[120px] animate-pulse rounded-full bg-surface-tertiary" />
        </div>
        <div className="space-y-2.5">
          {[0, 1, 2].map((i) => (
            <div key={i} className="flex items-center gap-2.5">
              <span className="h-7 w-1 shrink-0 animate-pulse rounded-full bg-surface-tertiary" />
              <div className="min-w-0 flex-1 space-y-1.5">
                <Skeleton className="h-3 w-24" />
                <Skeleton className="h-2.5 w-32" />
              </div>
              <div className="flex flex-col items-end space-y-1.5">
                <Skeleton className="h-3.5 w-10" />
                <Skeleton className="h-2.5 w-12" />
              </div>
            </div>
          ))}
        </div>
      </SkeletonCardShell>

      <SkeletonCardShell>
        <Skeleton className="mb-2 h-7 w-16" />
        <Skeleton className="mb-2 h-2 w-full rounded-full" />
        <Skeleton className="h-3 w-3/4" />
      </SkeletonCardShell>

      <SkeletonCardShell>
        <Skeleton className="h-28 w-full rounded-lg" />
      </SkeletonCardShell>
      <SkeletonCardShell>
        <Skeleton className="h-28 w-full rounded-lg" />
      </SkeletonCardShell>

      <SkeletonCardShell>
        <div className="flex items-center gap-4">
          <span className="block h-24 w-24 shrink-0 animate-pulse rounded-full bg-surface-tertiary" />
          <div className="grid flex-1 grid-cols-2 gap-x-3 gap-y-2">
            {[0, 1, 2, 3].map((i) => (
              <Skeleton key={i} className="h-3 w-full" />
            ))}
          </div>
        </div>
      </SkeletonCardShell>
    </div>
  )
}

/**
 * Per-instance timer/timestamp bookkeeping behind {@link useSkeletonVisible}.
 * Held once per component instance (via `useState`) rather than recreated
 * each render, so `shownAt` survives the resubscribes that happen every time
 * `loading` flips: the anti-flash delay and the minimum-visible floor both
 * depend on remembering *when* the skeleton last appeared, not just whether a
 * transition is currently in flight.
 */
class SkeletonGate {
  visible = false
  private shownAt: number | null = null
  private notify: (() => void) | null = null

  getSnapshot = (): boolean => this.visible

  /**
   * `useSyncExternalStore`'s `subscribe`, closed over the `loading` value it
   * was built for. Called fresh whenever `loading` changes (that is the
   * deliberate keyed resubscription — see useExternalSubscription.ts), so
   * each call is exactly one loading/not-loading transition.
   */
  subscribe =
    (loading: boolean) =>
    (onChange: () => void): (() => void) => {
      this.notify = onChange
      let timer: ReturnType<typeof setTimeout> | null = null

      if (loading) {
        timer = setTimeout(() => {
          timer = null
          this.shownAt = Date.now()
          this.setVisible(true)
        }, SKELETON_DELAY_MS)
      } else if (this.shownAt != null) {
        // Loading finished while the skeleton was up: hold it for the rest of
        // its minimum-visible window.
        const remaining = SKELETON_MIN_VISIBLE_MS - (Date.now() - this.shownAt)
        this.shownAt = null
        if (remaining > 0) {
          timer = setTimeout(() => {
            timer = null
            this.setVisible(false)
          }, remaining)
        } else {
          this.setVisible(false)
        }
      } else {
        // It never showed, so there is nothing to hold.
        this.setVisible(false)
      }

      return () => {
        if (timer != null) clearTimeout(timer)
        this.notify = null
      }
    }

  private setVisible(value: boolean): void {
    if (this.visible === value) return
    this.visible = value
    this.notify?.()
  }
}

/**
 * Gate the skeleton on time, not just on `loading`, to kill the flash on fast
 * loads. Returns true only once loading has persisted past the delay; once
 * shown it stays true for the minimum-visible window, so a quick finish cannot
 * flicker it. A load that resolves within the delay never flips it at all.
 */
function useSkeletonVisible(loading: boolean): boolean {
  const [gate] = useState(() => new SkeletonGate())
  const subscribe = useCallback(
    (onChange: () => void) => gate.subscribe(loading)(onChange),
    [gate, loading],
  )
  return useSyncExternalStore(subscribe, gate.getSnapshot, gate.getSnapshot)
}

/* -------------------------------------------------------------------------
 * The view
 * ---------------------------------------------------------------------- */

/**
 * The Session Detail surface shows one session's rhythm, modes, health, tokens,
 * context, and tools.
 *
 * Entirely prop-driven. Every value arrives as data and every action as a
 * callback, so this file has no notion of where an analysis comes from, when
 * it refreshes, or what a host can do with a session. That is what makes the
 * whole surface renderable — and testable — from a literal.
 */
export function SessionDetailPresentation({
  summary,
  loading,
  error,
  session,
  supportsAnalytics,
  cost,
  costSplit,
  orchestration,
  skills,
  models,
  relations,
  onBack,
  onPrev,
  onNext,
  onOpenSubagent,
  onOpenOrchestrator,
  onOpenRelatedSession,
  onExportSession,
  onDeleteSession,
  onRevealSource,
  renderAgentIcon,
}: SessionDetailPresentationProps) {
  const [modesPhase, setModesPhase] = useState<SessionPhase | null>(null)

  const subagent = session.subagent
  const modelRuns = models.map((model) => ({ model }))
  const modelNames = modelRunShortNames(modelRuns)
  const hygieneChecks = mockSessionHygiene(sessionIdentityKey(session))

  // Left and right arrows traverse adjacent sessions, mirroring the header
  // chevrons. A missing handler is a no-op, matching the chevrons' own state.
  useGlobalKeydown(true, (event) => {
    if (event.metaKey || event.ctrlKey || event.altKey) return
    const target = event.target as HTMLElement | null
    if (
      target &&
      (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)
    ) {
      return
    }
    if (event.key === "ArrowLeft" && onPrev) {
      event.preventDefault()
      onPrev()
    } else if (event.key === "ArrowRight" && onNext) {
      event.preventDefault()
      onNext()
    }
  })

  const showSkeleton = useSkeletonVisible(loading && supportsAnalytics)
  // The settled gate the real content, error, and empty states all key off:
  // true only once loading is done *and* the skeleton's minimum-visible
  // window, if any, has elapsed.
  const ready = !loading && !showSkeleton

  const empty = !summary || isEmptySummary(summary)
  const showEmptyState = ready && !error && empty

  const isOrchestrator = !subagent && !!orchestration?.orchestrating
  const costSubagentCount = orchestration?.subagentCount ?? costSplit?.subagentCount ?? 0
  const hasCostSubagents = !subagent && costSubagentCount > 0
  const costBadge = cost
    ? {
        totalUsd: cost.totalCostUsd,
        figureLabel: costFigureLabel(cost.isActive),
        models: modelRunNames(modelRuns),
        breakdownRows: costBreakdownRows(resultComponentCost(cost)),
      }
    : null

  const tokensCard = summary
    ? tokensCardModel({
        costScope: cost?.subject.scope ?? null,
        selectedCost: cost,
        selectedParentCost: costSplit?.parent ?? null,
        selectedSubagentsCost: costSplit?.subagents ?? null,
        hasCostSubagents,
        costSubagentCount,
        summaryCostTotalUsd: summary.costTotalUsd ?? null,
        tokensInTotal: summary.tokensInTotal,
        tokensOutTotal: summary.tokensOutTotal,
      })
    : null

  // Spawn dots on the parent ribbon: where each discovered sub-agent launched
  // on this session's active-time axis. Unlike the roster, markers do not
  // require fan-out — one child is still useful timeline context and a route
  // into that child's analytics.
  const spawnMarkers: TimelineMarker<SpawnPayload>[] =
    !subagent && orchestration
      ? orchestration.members
          .filter((member) => member.spawnProgress != null)
          .map((member) => ({
            progress: member.spawnProgress as number,
            data: {
              patternScore: member.patternScore,
              agent: member.agent,
              label: member.label,
              open: () => onOpenSubagent(member.subagentId, member.label),
            },
          }))
      : []

  // Skill dots show where each skill invocation landed.
  const skillMarkers: TimelineMarker<SkillDetail>[] = skills.map((detail) => ({
    progress: detail.progress,
    data: detail,
  }))

  const markerLayers: MarkerLayer[] = [
    ...(spawnMarkers.length > 0
      ? [
          {
            markers: spawnMarkers,
            kind: createSpawnMarkerKind({ renderAgentIcon }),
          } as MarkerLayer,
        ]
      : []),
    ...(skillMarkers.length > 0
      ? [
          {
            markers: skillMarkers,
            kind: skillMarkerKind,
          } as MarkerLayer,
        ]
      : []),
  ]

  const firstSession = summary?.sessions[0]
  const hasRelations = !!relations && (!!relations.parent || relations.children.length > 0)

  return (
    <div className="flex h-full flex-col overflow-hidden rounded-popover bg-surface text-label select-none">
      <div className="flex items-center justify-between gap-2 border-b border-separator px-4 py-3">
        {/* The control and the title are two things, not one. Wrapping the
            heading text inside the back button made a screen reader announce
            "Session Detail, button" for the control that leaves this view,
            and left the view itself with no heading at all. */}
        <div className="flex min-w-0 items-center gap-1.5">
          <button
            type="button"
            onClick={onBack}
            aria-label="Back"
            className="-ml-1 inline-flex h-6 shrink-0 items-center rounded-control px-1 text-label hover:bg-surface-hover"
          >
            <ChevronLeft size={14} aria-hidden="true" className="shrink-0" />
          </button>
          <h2
            data-view-heading
            tabIndex={-1}
            className="truncate type-headline text-label outline-none"
          >
            Session Detail
          </h2>
          {subagent && (
            <span className="shrink-0 rounded bg-system-indigo/15 px-1.5 py-px type-caption font-medium text-system-indigo-text">
              Sub-agent
            </span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {hasRelations && relations && (
            <RelationControl relations={relations} onOpen={onOpenRelatedSession} />
          )}
          {onRevealSource && (
            <Tooltip label="Reveal in file manager">
              <button
                type="button"
                onClick={onRevealSource}
                aria-label="Reveal in file manager"
                className="rounded-md p-1 text-label-tertiary hover:bg-surface-tertiary hover:text-label-secondary"
              >
                <FolderOpen size={14} aria-hidden="true" />
              </button>
            </Tooltip>
          )}
          <Tooltip label="Export this session">
            <button
              type="button"
              onClick={onExportSession}
              aria-label="Export this session"
              className="rounded-md p-1 text-label-tertiary hover:bg-surface-tertiary hover:text-label-secondary"
            >
              <Share size={14} aria-hidden="true" />
            </button>
          </Tooltip>
          <Tooltip label="Delete this session">
            <button
              type="button"
              onClick={onDeleteSession}
              aria-label="Delete this session"
              className="rounded-md p-1 text-label-tertiary hover:bg-surface-tertiary hover:text-system-red-text"
            >
              <Trash2 size={14} aria-hidden="true" />
            </button>
          </Tooltip>
        </div>
      </div>

      <div key={sessionIdentityKey(session)} className="min-h-0 flex-1 overflow-y-auto py-3">
        {showSkeleton && <SessionDetailSkeleton />}

        {ready && error && (
          <p className="px-4 type-callout text-system-orange">Couldn't read this session.</p>
        )}

        {showEmptyState && (
          <div className="flex flex-col items-center justify-center px-8 py-12 text-center">
            <Moon size={28} aria-hidden="true" className="mb-3 text-label-tertiary" />
            {!supportsAnalytics ? (
              <p className="type-body text-label">
                Session health for {agentDisplayName(session.agent)} sessions isn&apos;t
                available yet
              </p>
            ) : (
              <>
                <p className="type-body text-label">No session health available</p>
                <p className="mt-1 type-callout text-label-tertiary">
                  {relations?.parent
                    ? "This fork has no analyzable child activity yet."
                    : "This session has no analyzable messages in its local transcript."}
                </p>
              </>
            )}
            {costBadge && (
              <div className="mt-3">
                <SessionCostBadge {...costBadge} />
              </div>
            )}
          </div>
        )}

        {ready && !error && !empty && summary && (
          <>
            {/* Header */}
            <div className="flex items-start gap-3 px-4 pb-3">
              <div className="min-w-0 flex-1 flex flex-col gap-y-2">
                <div className="flex items-center gap-x-2" aria-label="Session summary">
                  <span className="shrink-0">{renderAgentIcon(session.agent, 20)}</span>

                  {session.repo && (
                    <Tooltip label={session.repo}>
                      <span className="truncate text-label-secondary">{session.repo}</span>
                    </Tooltip>
                  )}

                  {costBadge && <SessionCostBadge {...costBadge} />}

                  {session.timestamp && (
                    <time
                      dateTime={session.timestamp}
                      aria-label={`Last activity ${relativeTime(session.timestamp)}`}
                      className="shrink-0 type-footnote text-label-tertiary"
                    >
                      {relativeTime(session.timestamp)}
                    </time>
                  )}

                  <div className="ml-auto flex shrink-0 items-center">
                    <Tooltip
                      label={
                        <span className="inline-flex items-center gap-1.5">
                          Newer session
                          <kbd className="rounded bg-surface-tertiary px-1 text-label-tertiary">
                            ←
                          </kbd>
                        </span>
                      }
                    >
                      <button
                        type="button"
                        onClick={onPrev}
                        disabled={!onPrev}
                        aria-label="Newer session"
                        className={cn(
                          "shrink-0 rounded-md p-1 transition-colors",
                          onPrev
                            ? "text-label-tertiary hover:bg-surface-tertiary hover:text-label-secondary"
                            : "cursor-default text-label-tertiary opacity-40",
                        )}
                      >
                        <ChevronLeft size={16} aria-hidden="true" />
                      </button>
                    </Tooltip>
                    <Tooltip
                      label={
                        <span className="inline-flex items-center gap-1.5">
                          Older session
                          <kbd className="rounded bg-surface-tertiary px-1 text-label-tertiary">
                            →
                          </kbd>
                        </span>
                      }
                    >
                      <button
                        type="button"
                        onClick={onNext}
                        disabled={!onNext}
                        aria-label="Older session"
                        className={cn(
                          "shrink-0 rounded-md p-1 transition-colors",
                          onNext
                            ? "text-label-tertiary hover:bg-surface-tertiary hover:text-label-secondary"
                            : "cursor-default text-label-tertiary opacity-40",
                        )}
                      >
                        <ChevronRight size={16} aria-hidden="true" />
                      </button>
                    </Tooltip>
                  </div>
                </div>

                <div className="flex items-center gap-1 bg-(--color-bg-secondary) border-dashed border-1 border-(--color-border) px-3 py-2 rounded-md">
                  <TruncatedText
                    className="min-w-0 flex-1 text-sm break-all"
                    text={relations?.title?.trim() || session.title?.trim() || "Session"}
                    lines={3}
                  />
                </div>

                <div
                  className="flex min-w-0 items-center gap-1.5 type-footnote text-label-tertiary"
                  aria-label="Session timing and models"
                >
                  <span className="shrink-0">
                    {formatDuration(summary.avgActiveSecs)} active ·{" "}
                    {formatDuration(summary.avgDurationSecs)} overall
                  </span>
                  <WslOriginBadge distro={session.wslDistro} />
                  {modelNames.length > 0 && (
                    <div className="min-w-0" title={modelRunNames(modelRuns).join("\n")}>
                      <TruncatedText text={modelNames.join(" · ")} />
                    </div>
                  )}
                </div>

                <div className="flex items-center gap-x-2">
                  <span className="type-footnote text-label-tertiary">Hygiene Checks:</span>
                  <div
                    className="flex items-center gap-x-2"
                    aria-label="Session hygiene checks"
                  >
                    {hygieneChecks.map((check) => (
                      <span
                        key={check.id}
                        className={cn(
                          "inline-flex h-5 w-5 items-center justify-center rounded-full text-[0.75rem] leading-none text-white",
                          check.passed
                            ? "bg-(--color-system-green) opacity-30 hover:opacity-80"
                            : "bg-(--color-system-red) opacity-50 hover:opacity-80",
                        )}
                        title={check.title}
                        aria-label={check.title}
                      >
                        {check.passed ? "✓" : "×"}
                      </span>
                    ))}
                  </div>
                </div>
              </div>
            </div>

            {/* Provenance: mark a sub-agent view as an autonomous worker and
                link up to the orchestrator that launched it. */}
            {subagent && (
              <SubagentBadge
                parentAgent={session.agent}
                {...(subagent.parentTitle ? { parentTitle: subagent.parentTitle } : {})}
                onOpenOrchestrator={onOpenOrchestrator}
                renderAgentIcon={renderAgentIcon}
              />
            )}

            {/* The orchestrator's roster, expanding inline. */}
            {isOrchestrator && orchestration && (
              <OrchestratedBadge
                status={orchestration}
                onOpenSubagent={onOpenSubagent}
                renderAgentIcon={renderAgentIcon}
              />
            )}

            <Card
              title="Session rhythm"
              info="The leading activity at each moment of the session. Thick bands mean one mode clearly dominated; thin, shifting bands mean it kept switching."
              {...(isOrchestrator ? { hint: "parent agent" } : {})}
            >
              <Legend />
              {firstSession?.segments?.length ? (
                // Per-turn segments show the session without resampling.
                <HypnogramSegments
                  segments={firstSession.segments}
                  activeSecs={summary.avgActiveSecs}
                  contextWindow={summary.contextAvailable ? summary.contextWindow : 0}
                  layers={markerLayers}
                />
              ) : (
                // Use the resampled buckets when per-turn segments are absent.
                <Hypnogram
                  buckets={summary.buckets}
                  durationSecs={summary.avgActiveSecs}
                  contextWindow={summary.contextAvailable ? summary.contextWindow : 0}
                />
              )}
              <div className="mt-1 flex justify-between type-caption text-label-tertiary">
                <span>0:00</span>
                <span>{formatDuration(summary.avgActiveSecs)}</span>
              </div>
            </Card>

            <Card
              title="Modes"
              info="Each mode's share of active time. High and Low flag it against the healthy reference band shown under each mode."
              {...(isOrchestrator ? { hint: "parent agent" } : {})}
            >
              <PhaseDonut
                summary={summary}
                activePhase={modesPhase}
                onHoverPhase={setModesPhase}
              />
              <PhaseBreakdownRows
                summary={summary}
                activePhase={modesPhase}
                onHoverPhase={setModesPhase}
              />
            </Card>

            <Card
              title="Pattern health"
              info="Rates session steadiness from pattern signals like rework loops and lost context. 75+ is healthy, 50–74 drifting, under 50 thrashing."
            >
              <PatternScore score={summary.avgPatternScore} signals={summary.signals} />
            </Card>

            {tokensCard && (
              <Card title="Tokens" info={tokensCard.info} hint={tokensCard.hint}>
                <TokenAreaChart buckets={summary.buckets} />
                {cost && <CostBreakdown cost={cost} split={tokensCard.split} />}
              </Card>
            )}

            <Card
              title="Context"
              info="Share of the model's context window in use. The marker is at 90% occupancy."
              hint={
                summary.contextAvailable
                  ? `peak ${formatPct(
                      Math.min(1, summary.peakContextTokens / summary.contextWindow),
                    )}`
                  : "unavailable"
              }
            >
              {summary.contextAvailable ? (
                <ContextDriftChart
                  buckets={summary.buckets}
                  contextWindow={summary.contextWindow}
                />
              ) : (
                <p className="py-6 text-center type-callout text-label-tertiary">
                  Context occupancy is unavailable for this model.
                </p>
              )}
            </Card>

            {/* Show where the context window went before the first response. */}
            {firstSession?.initialContext && (
              <Card
                title="Initial context"
                info="Tokens loaded before the first response, by source. 'Fixed overhead' is the platform's baseline context that's always loaded and can't be edited — the system prompt and built-in tool definitions; exact makeup varies by agent."
                {...(firstSession.initialContext.totalTokens != null
                  ? { hint: formatCompact(firstSession.initialContext.totalTokens) }
                  : {})}
              >
                <InitialContextChart breakdown={firstSession.initialContext} />
              </Card>
            )}

            <Card
              title="Tools"
              info="Which tool types the agent leaned on. Heavy searching often means it's hunting for context rather than making changes."
            >
              <ToolMixChart summary={summary} />
            </Card>
          </>
        )}
      </div>
    </div>
  )
}
