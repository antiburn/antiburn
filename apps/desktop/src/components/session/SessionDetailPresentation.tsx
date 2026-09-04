import * as DropdownMenu from "@radix-ui/react-dropdown-menu"
import {
  ChevronLeft,
  FolderOpen,
  GitBranchPlus,
  GitFork,
  LoaderCircle,
  Moon,
  Trash2,
} from "lucide-react"
import { useCallback, useState, useSyncExternalStore, type ReactNode } from "react"

import { cn } from "../../lib/cn"
import { agentDisplayName } from "../../lib/presentation/agents"
import type { SessionHygienePayload } from "../../lib/insightsIpc"
import { sessionIdentityKey } from "../../lib/presentation/localIdentity"
import { sessionHygieneChecks } from "../../lib/presentation/sessionHygiene"
import {
  modelRunNames,
  modelRunShortPairs,
  type PresentableModelRun,
} from "../../lib/presentation/models"
import { relativeTime } from "../../lib/presentation/relativeTime"
import {
  costBreakdownRows,
  costFigureLabel,
  formatCompact,
  formatDuration,
  isEmptySummary,
  skillMcpUsage,
} from "../../lib/presentation/sessionAnalysis"
import { resultComponentCost, type LocalSessionCost } from "../../lib/presentation/sessionCosts"
import { efficiencyMetrics } from "../../lib/presentation/sessionEfficiency"
import type {
  ActiveSessionsSummary,
  SessionEfficiency,
  LocalSessionRelation,
  LocalSessionRelations,
} from "../../lib/types/session"
import { useGlobalKeydown } from "../../lib/useGlobalKeydown"
import "../../styles/session-detail.css"
import { Tooltip } from "../presentation/Tooltip"
import { TruncatedText } from "../presentation/TruncatedText"
import { WslOriginBadge } from "../presentation/WslOriginBadge"
import { SegmentedControl } from "../ui/SegmentedControl"
import { Skeleton } from "../ui/Skeleton"
import { CostBreakdown } from "./analysis/CostBreakdown"
import { ContextTokensChart, type ChartSeries } from "./analysis/ContextTokensChart"
import { EfficiencyBreakdown } from "./analysis/EfficiencyBreakdown"
import { HygieneBreakdown } from "./analysis/HygieneBreakdown"
import { SkillsMcpChart } from "./analysis/SkillsMcpChart"
import { SessionCostBadge } from "./metrics/SessionCostBadge"
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
  /** The hygiene reduction for this session's stored evidence. */
  hygiene: SessionHygienePayload
  /** Whether a newer analysis is on its way while `summary` stays on screen. */
  refreshing?: boolean
  /** Whether producing it failed. */
  error: boolean
  /** The session this view describes. */
  session: SessionDetailSubject
  /**
   * False when the engine has no adapter for this agent, which changes the
   * empty state from "nothing happened" to "we cannot read this yet".
   */
  supportsAnalysis: boolean
  /**
   * True when no published row set exists yet for this session. Changes the
   * empty state to an indexing message: the worker has not analyzed the
   * session yet, so "no analyzable messages" would be wrong.
   */
  analysisPending: boolean

  /** The cost result the breakdown describes, when one was priced. */
  cost: LocalSessionCost | null
  /** Parent/sub-agents split for an orchestration total. */
  costSplit: TokensCostSplit | null
  /** Where the spend behind `cost` went. The same subject as `cost`. */
  efficiency: SessionEfficiency | null
  /** How many sub-agents this session launched. Known before the cost is priced. */
  subagentCount: number
  /** Parent model runs followed by runs used only by sub-agents. */
  modelRuns: PresentableModelRun[]
  /** Direct fork relations resolved from local transcripts. */
  relations: LocalSessionRelations | null
  onBack: () => void
  /** Navigate to the newer adjacent session; omit when none exists. */
  onPrev?: () => void
  /** Navigate to the older adjacent session; omit when none exists. */
  onNext?: () => void
  /** Open one sub-agent's analysis from the roster. */
  onOpenSubagent: (subagentId: string, label: string) => void
  /** From a sub-agent view, open the launching orchestrator's analysis. */
  onOpenOrchestrator: () => void
  /** Open a fork parent or child. */
  onOpenRelatedSession: (target: LocalSessionRelation, title: string) => void

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
            className="rounded-control p-1 text-label-tertiary hover:bg-surface-tertiary hover:text-label-secondary"
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
            className="rounded-control p-1 text-label-tertiary hover:bg-surface-tertiary hover:text-label-secondary"
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
              className="inline-flex items-center gap-0.5 rounded-control px-1 py-0.5 type-caption text-label-tertiary hover:bg-surface-tertiary hover:text-label-secondary"
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

/** The three views of one session's analysis. */
type SessionDetailTab = "overview" | "cost" | "tools"

const DETAIL_TABS: ReadonlyArray<{ value: SessionDetailTab; label: string }> = [
  { value: "overview", label: "Context" },
  { value: "cost", label: "Cost" },
  { value: "tools", label: "Tools" },
]

/** The name of one block inside a tab that holds more than one block. */
function TabSectionHeading({ children }: { children: string }) {
  return (
    <h3 className="mb-2 type-caption font-medium! text-label-tertiary uppercase">{children}</h3>
  )
}

/**
 * The swatch each key entry carries. Each series repeats the color its own
 * chart layer takes when it lights, so the grid reads as the chart's key.
 */
const SERIES_SWATCH_CLASS: Record<ChartSeries, string> = {
  context: "bg-context-stroke",
  in: "bg-token-in",
  out: "bg-token-out",
  rehydration: "bg-mark-rehydration",
  routingMiss: "bg-mark-routing-miss",
  compaction: "bg-mark-compaction",
}

/** A shorter caption for a stat whose full name does not fit one cell. */
const KEY_CAPTIONS: Record<string, string> = {
  "Provider cache misses": "Cache misses",
}

/**
 * The chart's key, drawn under the plot it explains.
 *
 * Each figure is a stat cell: a swatch in the color its chart layer takes
 * when it lights, the value in the label ink, and a caption under them.
 * The cells sit in a grid of three equal columns, so the key reads as one
 * table and not as six badges. The swatch alone carries the color, so the
 * text keeps its contrast on both surfaces.
 *
 * Pointing at a cell lights its layer in the plot above, and the cell takes
 * the hover wash. Clicking a cell pins that layer, so it stays lit when the
 * pointer leaves; clicking it again unpins it. An entry whose `series` is
 * absent counts something the chart draws no mark for, so it neither lights
 * nor pins.
 */
function ChartKey({
  stats,
  pinned,
  onHighlight,
  onPin,
}: {
  stats: ReadonlyArray<{
    label: string
    value: string
    series?: ChartSeries
  }>
  /** The layer held lit by a click, or null. */
  pinned: ChartSeries | null
  /** Names the layer under the pointer, or null when the pointer leaves. */
  onHighlight: (series: ChartSeries | null) => void
  /** Toggles the pinned layer. */
  onPin: (series: ChartSeries) => void
}) {
  return (
    <div className="-mx-1.5 grid grid-cols-3 gap-x-1 gap-y-1">
      {stats.map((stat) => {
        const series = stat.series ?? null
        const isPinned = series != null && series === pinned
        return (
          <Tooltip key={stat.label} label={stat.label}>
            <button
              type="button"
              aria-pressed={series != null ? isPinned : undefined}
              disabled={series == null}
              data-series={series ?? undefined}
              className={cn(
                "chart-key-stat flex min-w-0 flex-col items-start rounded-control px-1.5 py-1 text-left disabled:opacity-100",
                isPinned && "bg-surface-secondary",
              )}
              onMouseEnter={() => onHighlight(series)}
              onMouseLeave={() => onHighlight(null)}
              onClick={() => series != null && onPin(series)}
            >
              <span className="flex items-center gap-x-1.5 type-body font-medium text-label tabular-nums">
                <span
                  aria-hidden="true"
                  className={cn(
                    "size-2 shrink-0 rounded-full",
                    series != null ? SERIES_SWATCH_CLASS[series] : "bg-surface-tertiary",
                  )}
                />
                {stat.value}
              </span>
              <span className="max-w-full truncate type-caption text-label-secondary">
                {KEY_CAPTIONS[stat.label] ?? stat.label}
              </span>
            </button>
          </Tooltip>
        )
      })}
    </div>
  )
}

/** Placeholder block matching a tab panel's content spacing. */
function SkeletonCardShell({ children }: { children: ReactNode }) {
  return (
    <div className="mx-4 mb-4">
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
    <div aria-hidden data-testid="session-analysis-skeleton">
      <div className="flex items-start gap-3 px-4 pb-3">
        <Skeleton className="mt-0.5 h-5 w-5 shrink-0" />
        <div className="min-w-0 flex-1 space-y-2">
          <Skeleton className="h-4 w-40" />
          <Skeleton className="h-3 w-28" />
        </div>
      </div>

      <SkeletonCardShell>
        <Skeleton className="h-28 w-full rounded-control" />
      </SkeletonCardShell>
      <SkeletonCardShell>
        <Skeleton className="h-28 w-full rounded-control" />
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
 * The Session Detail surface shows one session's tokens, context, and tools.
 *
 * Entirely prop-driven. Every value arrives as data and every action as a
 * callback, so this file has no notion of where an analysis comes from, when
 * it refreshes, or what a host can do with a session. That is what makes the
 * whole surface renderable — and testable — from a literal.
 */
export function SessionDetailPresentation({
  summary,
  loading,
  hygiene,
  refreshing = false,
  error,
  session,
  supportsAnalysis,
  analysisPending,
  cost,
  costSplit,
  efficiency,
  subagentCount,
  modelRuns,
  relations,
  onBack,
  onPrev,
  onNext,
  onOpenSubagent,
  onOpenOrchestrator,
  onOpenRelatedSession,
  onDeleteSession,
  onRevealSource,
  renderAgentIcon,
}: SessionDetailPresentationProps) {
  const subagent = session.subagent
  const [tab, setTab] = useState<SessionDetailTab>("overview")
  // Which chart layer the key points at. The pointer sets it and the pointer
  // clears it; a click pins a layer, which holds when the pointer leaves.
  const [hovered, setHovered] = useState<ChartSeries | null>(null)
  const [pinned, setPinned] = useState<ChartSeries | null>(null)
  const highlight = hovered ?? pinned
  const togglePin = useCallback(
    (series: ChartSeries) => setPinned((current) => (current === series ? null : series)),
    [],
  )
  const modelPairs = modelRunShortPairs(modelRuns)
  const hygieneChecks = sessionHygieneChecks(hygiene)
  const hasAssessedHygieneChecks = hygieneChecks.some((check) => check.status !== "notAssessed")

  // Left and right arrows traverse adjacent sessions. A missing handler is a
  // no-op.
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

  const showSkeleton = useSkeletonVisible(loading && supportsAnalysis)
  // The settled gate the real content, error, and empty states all key off:
  // true only once loading is done *and* the skeleton's minimum-visible
  // window, if any, has elapsed.
  const ready = !loading && !showSkeleton

  const empty = !summary || isEmptySummary(summary)
  const showEmptyState = ready && !error && empty

  const costSubagentCount = subagentCount || (costSplit?.subagentCount ?? 0)
  const hasCostSubagents = !subagent && costSubagentCount > 0
  const costBadge = cost
    ? {
        totalUsd: cost.totalCostUsd,
        figureLabel: costFigureLabel(cost.isActive),
        models: modelRunNames(modelRuns),
        breakdownRows: costBreakdownRows(resultComponentCost(cost)),
      }
    : null

  const efficiencyCard = efficiency ? efficiencyMetrics(efficiency, session.agent) : null

  const firstSession = summary?.sessions[0]
  const toolsUsage = firstSession?.initialContext
    ? skillMcpUsage(firstSession.initialContext)
    : null

  const tokensCard = summary
    ? tokensCardModel({
        costScope: cost?.subject.scope ?? null,
        selectedCost: cost,
        selectedParentCost: costSplit?.parent ?? null,
        selectedSubagentsCost: costSplit?.subagents ?? null,
        hasCostSubagents,
        costSubagentCount,
        members: costSplit?.members ?? [],
        sessionStartedAtEpoch: costSplit?.sessionStartedAtEpoch ?? null,
        summaryCostTotalUsd: summary.costTotalUsd ?? null,
        tokensInTotal: summary.tokensInTotal,
        tokensOutTotal: summary.tokensOutTotal,
        compactionCount: summary.compactionCount ?? 0,
        cacheRehydrationCount: summary.cacheRehydrationCount ?? 0,
        // Only `SessionMetrics` carries this count, not the aggregate
        // summary, so it comes from the first (and, for this view, only)
        // session rather than from `summary` itself.
        cacheRoutingMissCount: firstSession?.cacheRoutingMissCount ?? 0,
      })
    : null
  // The context area is the chart's main shape, so the key names it first.
  // Its figure is the peak in tokens.
  const chartKeyStats =
    summary && tokensCard
      ? [
          {
            label: "Context",
            value: formatCompact(summary.peakContextTokens),
            series: "context" as const,
          },
          ...tokensCard.stats,
        ]
      : []

  const hasRelations = !!relations && (!!relations.parent || relations.children.length > 0)

  return (
    <div className="flex h-full flex-col overflow-hidden rounded-popover bg-surface text-label select-none">
      <div className="flex items-center justify-between gap-2 border-b border-separator px-3 py-3">
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
          {refreshing && (
            <span
              role="status"
              className="inline-flex shrink-0 items-center text-label-tertiary"
            >
              <LoaderCircle
                size={12}
                strokeWidth={2}
                aria-hidden="true"
                className="animate-spin"
              />
              <span className="sr-only">Refreshing session analysis</span>
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
                className="rounded-control p-1 text-label-tertiary hover:bg-surface-tertiary hover:text-label-secondary"
              >
                <FolderOpen size={14} aria-hidden="true" />
              </button>
            </Tooltip>
          )}
          <Tooltip label="Delete this session">
            <button
              type="button"
              onClick={onDeleteSession}
              aria-label="Delete this session"
              className="rounded-control p-1 text-label-tertiary hover:bg-surface-tertiary hover:text-system-red-text"
            >
              <Trash2 size={14} aria-hidden="true" />
            </button>
          </Tooltip>
        </div>
      </div>

      <div key={sessionIdentityKey(session)} className="flex min-h-0 flex-1 flex-col">
        {(showSkeleton || (ready && (error || empty))) && (
          <div className="min-h-0 flex-1 overflow-y-auto py-3">
            {showSkeleton && <SessionDetailSkeleton />}
            {ready && error && (
              <p className="px-4 type-callout text-system-orange">
                Couldn't read this session.
              </p>
            )}

            {showEmptyState && (
              <div className="flex flex-col items-center justify-center px-8 py-12 text-center">
                <Moon size={28} aria-hidden="true" className="mb-3 text-label-tertiary" />
                {analysisPending ? (
                  <>
                    <p className="type-body text-label">Analyzing this session…</p>
                    <p className="mt-1 type-callout text-label-tertiary">
                      Indexing is in progress. This view updates on its own once it finishes.
                    </p>
                  </>
                ) : !supportsAnalysis ? (
                  <p className="type-body text-label">
                    Session analysis for {agentDisplayName(session.agent)} sessions isn&apos;t
                    available yet
                  </p>
                ) : (
                  <>
                    <p className="type-body text-label">No session analysis available</p>
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
          </div>
        )}

        {ready && !error && !empty && summary && (
          <>
            <div className="flex flex-col gap-y-1 px-4 pt-3 pb-3" aria-label="Session summary">
              {/* The repo comes first because it is the container: you are
                  inside a repo, and the session is the work you do in it. */}
              {(session.repo || session.wslDistro) && (
                <div className="flex min-w-0 items-center gap-1.5 type-caption text-label-tertiary">
                  {session.repo && (
                    <Tooltip label={session.repo}>
                      <span className="truncate">{session.repo}</span>
                    </Tooltip>
                  )}
                  <WslOriginBadge distro={session.wslDistro} />
                </div>
              )}

              <TruncatedText
                className="min-w-0 type-body-large font-semibold text-label break-words"
                text={relations?.title?.trim() || session.title?.trim() || "Session"}
                lines={2}
              />

              {/* Active time, last activity, and the models on one quiet
                  line. The figures that carry a reading live in the meter nav
                  below; these three are context for the title. */}
              <div className="flex min-w-0 flex-wrap items-baseline gap-x-1.5 type-caption text-label-secondary">
                <Tooltip
                  label={`Active time (${formatDuration(summary.avgDurationSecs)} overall)`}
                >
                  <span className="tabular-nums">
                    {formatDuration(summary.avgActiveSecs)} active
                  </span>
                </Tooltip>
                {session.timestamp && (
                  <>
                    <span aria-hidden="true" className="text-label-tertiary">
                      ·
                    </span>
                    <time dateTime={session.timestamp}>{relativeTime(session.timestamp)}</time>
                  </>
                )}
                {modelPairs.length > 0 && (
                  <>
                    <span aria-hidden="true" className="text-label-tertiary">
                      ·
                    </span>
                    <span
                      className="min-w-0 truncate"
                      title={modelRunNames(modelRuns).join("\n")}
                    >
                      {modelPairs.map((pair, index) => (
                        <span key={`${pair.model}/${pair.thinkingMode ?? ""}`}>
                          {index > 0 && " · "}
                          <span>{pair.model}</span>
                          {pair.thinkingMode && <span> {pair.thinkingMode}</span>}
                        </span>
                      ))}
                    </span>
                  </>
                )}
              </div>
            </div>

            {subagent && (
              <SubagentBadge
                parentAgent={session.agent}
                {...(subagent.parentTitle ? { parentTitle: subagent.parentTitle } : {})}
                onOpenOrchestrator={onOpenOrchestrator}
                renderAgentIcon={renderAgentIcon}
              />
            )}

            <div className="border-b border-separator px-4 pb-3">
              <SegmentedControl
                options={DETAIL_TABS}
                value={tab}
                onChange={setTab}
                ariaLabel="Session detail sections"
                semantics="tabs"
                variant="native-tabs"
                idPrefix="session-detail-tabs"
              />
            </div>

            <div
              id="session-detail-tabs-panel"
              role="tabpanel"
              aria-labelledby={`session-detail-tabs-${tab}`}
              className="min-h-0 flex-1 overflow-y-auto px-4 py-4"
            >
              {/* The chart takes every pixel the key leaves, so the tab
                  never ends in empty space. */}
              {tab === "overview" && tokensCard && (
                <div className="flex h-full flex-col gap-y-3">
                  <div className="min-h-0 flex-1">
                    <ContextTokensChart
                      buckets={summary.buckets}
                      contextWindow={summary.contextAvailable ? summary.contextWindow : null}
                      activeSecs={summary.avgActiveSecs}
                      highlight={highlight}
                    />
                  </div>
                  <ChartKey
                    stats={chartKeyStats}
                    pinned={pinned}
                    onHighlight={setHovered}
                    onPin={togglePin}
                  />
                  {/* The composition says what the context above cost. */}
                  {efficiencyCard && (
                    <div className="border-t border-separator pt-3">
                      <EfficiencyBreakdown metrics={efficiencyCard} section="composition" />
                    </div>
                  )}
                </div>
              )}

              {/* The checks lead: a failing check is more use than the cost
                  rows most of the time. */}
              {tab === "cost" && (
                <div className="flex min-h-full flex-col">
                  {hasAssessedHygieneChecks && (
                    <section>
                      <TabSectionHeading>Checks</TabSectionHeading>
                      <HygieneBreakdown checks={hygieneChecks} collapsePassing={false} />
                    </section>
                  )}
                  <section
                    className={cn(
                      hasAssessedHygieneChecks && "mt-4 border-t border-separator pt-4",
                      efficiencyCard && "pb-4",
                    )}
                  >
                    <TabSectionHeading>Cost</TabSectionHeading>
                    {cost && tokensCard ? (
                      <CostBreakdown
                        cost={cost}
                        split={tokensCard.split}
                        onOpenSubagent={onOpenSubagent}
                      />
                    ) : (
                      <p className="type-callout text-label-tertiary">
                        No cost has been recorded for this session.
                      </p>
                    )}
                  </section>
                  {/* The $/MTok scale closes the money story. It sits at the
                      foot of the tab, so a short tab keeps its slack between
                      the cost rows and the scale, not after the scale. */}
                  {efficiencyCard && (
                    <section className="mt-auto border-t border-separator pt-4">
                      <TabSectionHeading>Efficiency</TabSectionHeading>
                      <EfficiencyBreakdown metrics={efficiencyCard} section="cost" />
                    </section>
                  )}
                </div>
              )}

              {tab === "tools" &&
                (firstSession?.initialContext ? (
                  <div className="flex flex-col gap-y-2">
                    {/* The wasted tokens are the finding of this tab, so they
                        head the table they summarize. The figure has no
                        ceiling, so it is a headline and not a meter. */}
                    {toolsUsage != null && toolsUsage.wastedTokens > 0 && (
                      <p className="my-1 flex items-center gap-x-3">
                        <span className="type-large-title font-semibold! text-brand tabular-nums">
                          {formatCompact(toolsUsage.wastedTokens)}
                        </span>
                        <span className="flex flex-col type-callout leading-tight">
                          <span className="font-semibold text-label">tokens burned</span>
                          <span className="text-label-secondary">
                            by items loaded but never called in this session
                          </span>
                        </span>
                      </p>
                    )}
                    <SkillsMcpChart breakdown={firstSession.initialContext} />
                  </div>
                ) : (
                  <p className="type-callout text-label-tertiary">
                    No startup context has been recorded for this session.
                  </p>
                ))}
            </div>
          </>
        )}
      </div>
    </div>
  )
}
