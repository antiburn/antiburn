import { GitBranchPlus, GitFork, SquareTerminal } from "lucide-react"
import {
  defaultRangeExtractor,
  useVirtualizer,
  type VirtualItem,
} from "@tanstack/react-virtual"
import { useCallback, useRef, useState, type ReactNode } from "react"

import { cn } from "../../lib/cn"
import type { SessionHygienePayload } from "../../lib/insightsIpc"
import { agentDisplayName, type AgentSurface } from "../../lib/presentation/agents"
import { liveDisplayableProviders, liveWindows } from "../../lib/presentation/liveUsage"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import {
  INITIAL_SESSION_HYGIENE,
  sessionHygieneChecks,
} from "../../lib/presentation/sessionHygiene"
import {
  modelRunNames,
  modelRunShortPairs,
  type PresentableModelRun,
} from "../../lib/presentation/models"
import { relativeTime } from "../../lib/presentation/relativeTime"
import { sessionHygieneFor, useSessionHygiene } from "../../lib/useSessionHygiene"
import { Tooltip } from "../presentation/Tooltip"
import { TruncatedText } from "../presentation/TruncatedText"
import { WslOriginBadge } from "../presentation/WslOriginBadge"
import { SessionStatusBar } from "./SessionStatusBar"
import { SessionTooltipOwner } from "./SessionTooltipOwner"
import { type SessionCostBadgeProps } from "./metrics/SessionCostBadge"
import { ScrollPane } from "../ui/ScrollPane"
import { SegmentedControl } from "../ui/SegmentedControl"
import { countGroupedItems, groupActivityByDay } from "../activity/activityFeedGrouping"
import { useActivityGroupPinning, type ViewportRef } from "../activity/useActivityGroupPinning"
import type {
  LiveUsageSummaryPayload,
  SessionLimitAllocationPayload,
  SessionLimitAllocationSummaryPayload,
} from "../../lib/ipc"

import "../../styles/session-rows.css"

/** The renderer shows an agent icon and can mark its surface. The caller supplies the artwork. */
type SessionAgentIconRenderer = (
  slug: string,
  size: number,
  surface?: AgentSurface,
) => ReactNode

/** One coding session in the list. */
export interface SessionListEntry {
  agent: string
  /** Absent for a session whose transcript id could not be read. */
  sessionId?: string | undefined
  /** Repository the session ran in; empty when it could not be resolved. */
  repo: string
  /** Other repositories the same session touched. */
  additionalRepos?: string[] | undefined
  branch?: string | undefined
  /** ISO timestamp of the session's most recent activity. */
  timestamp: string
  /** Whether the session has recent meaningful activity. */
  isActive: boolean
  /** Where the session was discovered from. */
  surface?: AgentSurface | undefined
  wslDistro?: string | null | undefined
  /** Resolved session title; falls back to a short id, then the agent name. */
  title?: string | undefined
  /** Whether this session was forked from another. */
  hasForkParent?: boolean | undefined
  /** How many sessions were forked from this one. */
  forkChildCount?: number | undefined
  /** Display values for the cost pill; omit when nothing priced the session. */
  cost?: SessionCostBadgeProps | null | undefined
  /** Parent model runs followed by runs used only by sub-agents. */
  modelRuns?: PresentableModelRun[] | undefined
}

export interface SessionListProps {
  /** Sessions to show. Ordering and grouping are this component's job. */
  entries: SessionListEntry[]
  /** Calendar-day window for finished sessions. Also drives the empty copy. */
  days: number
  /** Headline for the empty state; defaults to the range-aware wording. */
  emptyTitle?: string
  emptyDescription?: string
  /** Open a session's analysis. Omitted leaves rows inert. */
  onOpenSession?: (entry: SessionListEntry) => void
  /** Select one session without opening it. This enables arrow-key navigation. */
  onSelect?: (entry: SessionListEntry) => void
  /** Move focus to the selected session's detail pane. */
  onOpenDetail?: (entry: SessionListEntry) => void
  /** Stable local identity for the selected session. */
  selectedKey?: string | null
  /** Pause hidden presentation work while keeping the list mounted. */
  active?: boolean
  /** Let a window host use the list header as a native drag region. */
  draggableHeader?: boolean
  /** The scrolling viewport, for a host that needs to observe it. */
  viewportRef?: ViewportRef
  initialScrollOffset?: number | (() => number)
  initialMeasurementsCache?: VirtualItem[]
  onMeasurementsChange?: (measurements: VirtualItem[]) => void
  /** Frozen clock, for tests. */
  now?: Date
  renderAgentIcon?: SessionAgentIconRenderer
  /** Glyph for the WSL-origin badge. */
  wslIcon?: ReactNode
  badgeMetric?: "cost" | "weeklyPercent" | "fiveHourPercent"
  onBadgeMetricChange?: (metric: "cost" | "weeklyPercent" | "fiveHourPercent") => void
  liveUsage?: LiveUsageSummaryPayload
  sessionLimitAllocations?: SessionLimitAllocationSummaryPayload
}

function primaryLine(entry: SessionListEntry): string {
  const title = entry.title?.trim()
  if (title) return title
  if (entry.sessionId) return `Session ${entry.sessionId.slice(0, 7)}`
  return agentDisplayName(entry.agent)
}

type BadgeMetric = "cost" | "weeklyPercent" | "fiveHourPercent"

function keepScrollOffset(): false {
  return false
}

function isFiveHourWindow(provider: string, id: string): boolean {
  if (provider === "anthropic") return id === "five-hour"
  if (provider === "openai") return id === "five-hour" || id.endsWith("-300m")
  if (provider === "google") {
    return id === "antigravity-gemini-5h" || id === "antigravity-claude-gpt-5h"
  }
  return false
}

function hasDisplayedFiveHourWindow(live: LiveUsageSummaryPayload | undefined): boolean {
  return live
    ? liveDisplayableProviders(live).some((provider) =>
        liveWindows(provider).some((window) => isFiveHourWindow(provider.provider, window.id)),
      )
    : false
}

function sessionLimitBadge(
  metric: Exclude<BadgeMetric, "cost">,
  allocation?: SessionLimitAllocationPayload,
): {
  label: string
  percent: number | null
  provider?: string
  windowId?: string
} {
  if (!allocation || !Number.isFinite(allocation.percent)) {
    return {
      label: `No ${metric === "weeklyPercent" ? "weekly" : "5h"} limit for this session.`,
      percent: null,
    }
  }
  return {
    label: `Estimated share of your ${allocation.displayName} ${metric === "weeklyPercent" ? "weekly" : "5-hour"} limit.`,
    percent: allocation.percent,
    provider: allocation.provider,
    windowId: allocation.windowId,
  }
}

function EmptySessionList({ title, description }: { title: string; description: string }) {
  return (
    <div className="flex h-full items-center justify-center px-6 text-center">
      <div className="flex max-w-[230px] flex-col items-center">
        <div className="mb-3 flex h-10 w-10 items-center justify-center rounded-full bg-surface-secondary text-label-tertiary">
          <SquareTerminal size={20} strokeWidth={1.75} aria-hidden="true" />
        </div>
        <p className="type-body font-medium! text-label-secondary">{title}</p>
        <p className="mt-1.5 max-w-[230px] type-callout text-label-tertiary">{description}</p>
      </div>
    </div>
  )
}

type GroupedSessionItem = ReturnType<
  typeof groupActivityByDay<{
    entry: SessionListEntry
    at: string
    isActive: boolean
    key: string
  }>
>[number]["items"][number]

type VirtualSessionItem =
  | {
      type: "heading"
      key: string
      groupIndex: number
      groupLabel: string
    }
  | {
      type: "row"
      key: string
      groupIndex: number
      groupLabel: string
      itemIndex: number
      rowPosition: number
      item: GroupedSessionItem
    }

function groupHeadingId(label: string): string {
  return `activity-${label.replaceAll(" ", "-").toLowerCase()}`
}

interface SessionRowProps {
  entry: SessionListEntry
  hygiene: SessionHygienePayload
  onOpen?: () => void
  onSelect?: () => void
  onOpenDetail?: () => void
  selected?: boolean
  tabIndex?: number
  active?: boolean
  renderAgentIcon?: SessionAgentIconRenderer | undefined
  wslIcon?: ReactNode | undefined
  showCost?: boolean
  limitBadge?:
    | {
        label: string
        percent: number | null
        provider?: string
        windowId?: string
      }
    | undefined
}

/**
 * One session in the list: its identity, location, fork relationships,
 * cost, and last activity time.
 *
 * The whole card opens the session analysis. Unsupported agents open an empty
 * analysis state that explains why no data is available.
 */
function SessionRow({
  entry,
  hygiene,
  onOpen,
  onSelect,
  onOpenDetail,
  selected = false,
  tabIndex,
  active = true,
  renderAgentIcon,
  wslIcon,
  limitBadge,
  showCost = true,
}: SessionRowProps) {
  const selectionMode = !!entry.sessionId && !!onSelect
  const clickable = !!entry.sessionId && (!!onOpen || selectionMode)
  const primary = primaryLine(entry)
  const hasRepo = entry.repo !== ""
  const modelRuns = entry.modelRuns ?? []
  const modelPairs = modelRunShortPairs(modelRuns)
  const hygieneChecks = sessionHygieneChecks(hygiene)

  return (
    <div
      className={cn(
        "group relative",
        "w-full grid grid-cols-[14px_minmax(0,1fr)] gap-x-2 gap-y-1",
        "items-center",
        "rounded-[var(--radius-popover)] px-3 py-3",
        selected ? "bg-surface-selected/60" : "bg-surface-card/50",
        "transition-colors duration-[var(--duration-fast)] ease-out",
        entry.isActive && active && "activity-row-active",
        clickable && "cursor-pointer",
        clickable &&
          !selected &&
          "hover:bg-surface-secondary/50 [&:has([data-state*=open])]:bg-surface-secondary/50",
      )}
      {...(clickable
        ? {
            role: "button" as const,
            tabIndex: tabIndex ?? 0,
            "aria-current": selected ? ("true" as const) : undefined,
            "data-session-row": "",
            onClick: selectionMode
              ? (event: React.MouseEvent<HTMLDivElement>) => {
                  const target = event.target as Element
                  const nestedControl = target.closest(
                    'button, a, input, select, textarea, [role="button"]',
                  )
                  if (nestedControl && nestedControl !== event.currentTarget) return
                  event.currentTarget.focus()
                  onSelect?.()
                }
              : onOpen,
            onKeyDown: (event: React.KeyboardEvent) => {
              // Only when the row itself has focus: a nested control's Enter
              // belongs to that control, not to the card behind it.
              if (
                event.currentTarget === event.target &&
                (event.key === "Enter" || event.key === " ")
              ) {
                event.preventDefault()
                if (selectionMode) {
                  if (event.key === "Enter") onOpenDetail?.()
                  else onSelect?.()
                } else {
                  onOpen?.()
                }
              }
            },
          }
        : {})}
    >
      {entry.isActive && <span className="sr-only">Active session</span>}

      <div className="row-1 col-2">
        <SessionStatusBar
          checks={hygieneChecks}
          evidenceState={hygiene.evidenceState}
          cost={showCost ? (entry.cost ?? null) : null}
          limitBadge={limitBadge}
        />
      </div>

      <span className="row-2 col-1 h-full pt-[3px]">
        {renderAgentIcon?.(entry.agent, 14, entry.surface)}
      </span>

      <div className="col-2 flex min-w-0 items-center gap-x-1">
        <TruncatedText
          // One ink for every title. The shimmer overlay is the only
          // difference an active session shows.
          className="min-w-0 mt-px mb-0.5 type-body-large text-label"
          text={primary}
          lines={2}
          shimmer={entry.isActive && active}
        />

        {entry.hasForkParent && (
          <Tooltip label="Forked from another session" delayMs={500}>
            <span
              className="inline-flex shrink-0 text-label-tertiary"
              aria-label="Forked from another session"
            >
              <GitFork size={12} strokeWidth={2} aria-hidden="true" />
            </span>
          </Tooltip>
        )}
        {!!entry.forkChildCount && (
          <Tooltip
            label={`${entry.forkChildCount} direct ${entry.forkChildCount === 1 ? "fork" : "forks"}`}
            delayMs={500}
          >
            <span
              className="inline-flex shrink-0 text-label-tertiary"
              aria-label={`${entry.forkChildCount} direct ${entry.forkChildCount === 1 ? "fork" : "forks"}`}
            >
              <GitBranchPlus size={12} strokeWidth={2} aria-hidden="true" />
            </span>
          </Tooltip>
        )}
      </div>

      {modelPairs.length > 0 && (
        <div className="col-2 min-w-0 space-y-px">
          <div
            className="min-w-0 max-w-full truncate type-callout text-label-tertiary"
            title={modelRunNames(modelRuns).join("\n")}
          >
            {modelPairs.map((pair, index) => (
              <span key={`${pair.model}/${pair.thinkingMode ?? ""}`}>
                {index > 0 && " · "}
                <span className="font-medium">{pair.model}</span>
                {pair.thinkingMode && (
                  <span className="type-caption"> {pair.thinkingMode}</span>
                )}
              </span>
            ))}
          </div>
        </div>
      )}

      {(entry.timestamp || hasRepo || entry.branch || entry.wslDistro) && (
        <div className="col-2 w-full flex justify-between">
          {(hasRepo || entry.branch || entry.wslDistro) && (
            <div className="flex min-w-0 items-baseline gap-x-2">
              {hasRepo && (
                <Tooltip
                  label={
                    entry.additionalRepos?.length
                      ? `Also observed: ${entry.additionalRepos.join(", ")}`
                      : entry.repo
                  }
                >
                  <span className="min-w-0 truncate type-callout text-label-tertiary">
                    {entry.repo}
                    {entry.additionalRepos?.length ? ` +${entry.additionalRepos.length}` : ""}
                  </span>
                </Tooltip>
              )}

              <WslOriginBadge
                distro={entry.wslDistro}
                {...(wslIcon ? { icon: wslIcon } : {})}
              />

              {entry.branch && (
                <TruncatedText
                  className="min-w-0 truncate type-callout text-label-tertiary"
                  text={entry.branch}
                />
              )}
            </div>
          )}

          {entry.timestamp && (
            <time
              dateTime={entry.timestamp}
              aria-label={`Last activity ${relativeTime(entry.timestamp)}`}
              // The host row reveals the timestamp on hover.
              className="tabular-nums type-footnote font-mono text-label-tertiary opacity-0 transition-opacity duration-[var(--duration-fast)] group-hover:opacity-100"
            >
              {relativeTime(entry.timestamp)}
            </time>
          )}
        </div>
      )}
    </div>
  )
}

/**
 * A scrolling list of coding sessions, grouped by activity state and day.
 *
 * The host supplies entries, the visible range, and actions. The list controls
 * ordering, grouping, the sticky group label, and row presentation.
 */
export function SessionList({
  entries,
  days,
  emptyTitle,
  emptyDescription = "Coding sessions appear here as they are discovered on this machine.",
  onOpenSession,
  onSelect,
  onOpenDetail,
  selectedKey,
  active = true,
  draggableHeader = false,
  viewportRef,
  initialScrollOffset = 0,
  initialMeasurementsCache,
  onMeasurementsChange,
  now,
  renderAgentIcon,
  wslIcon,
  badgeMetric = "cost",
  onBadgeMetricChange,
  liveUsage,
  sessionLimitAllocations,
}: SessionListProps) {
  const fiveHourAvailable =
    badgeMetric === "fiveHourPercent" ||
    hasDisplayedFiveHourWindow(liveUsage) ||
    (sessionLimitAllocations?.allocations.some(
      (allocation) => allocation.metric === "fiveHour",
    ) ??
      false)
  const selectedMetric = badgeMetric
  const allocationBySession = new Map(
    (sessionLimitAllocations?.allocations ?? []).map((allocation) => [
      `${localSessionKey(allocation.agent, allocation.sessionId, allocation.wslDistro)}:${allocation.metric}`,
      allocation,
    ]),
  )
  const items = entries.map((entry, index) => ({
    entry,
    at: entry.timestamp,
    isActive: entry.isActive,
    key: entry.sessionId
      ? localSessionKey(entry.agent, entry.sessionId, entry.wslDistro)
      : `${entry.agent}|${index}`,
  }))

  const groups = groupActivityByDay(items, { days, ...(now ? { now } : {}) })
  const visibleCount = countGroupedItems(groups)
  let rowPosition = 0
  const virtualItems: VirtualSessionItem[] = groups.flatMap((group, groupIndex) => [
    {
      type: "heading" as const,
      key: `heading:${group.label}`,
      groupIndex,
      groupLabel: group.label,
    },
    ...group.items.map((item, itemIndex) => ({
      type: "row" as const,
      key: `row:${item.key}`,
      groupIndex,
      groupLabel: group.label,
      itemIndex,
      rowPosition: ++rowPosition,
      item,
    })),
  ])
  const rowIndexes = virtualItems.flatMap((item, index) => (item.type === "row" ? [index] : []))
  const selectableRowIndexes = virtualItems.flatMap((item, index) =>
    item.type === "row" && item.item.entry.sessionId ? [index] : [],
  )
  const hygieneSessions = active
    ? groups.flatMap((group) =>
        group.items.flatMap(({ entry }) =>
          entry.sessionId
            ? [
                {
                  agent: entry.agent,
                  sessionId: entry.sessionId,
                  wslDistro: entry.wslDistro ?? null,
                },
              ]
            : [],
        ),
      )
    : []
  const hygieneBySession = useSessionHygiene(hygieneSessions)
  const firstSelectableKey = groups
    .flatMap((group) => group.items)
    .find((item) => item.entry.sessionId)?.key
  const rovingKey = groups.some((group) =>
    group.items.some((item) => item.entry.sessionId && item.key === selectedKey),
  )
    ? selectedKey
    : firstSelectableKey

  const { assignViewportRef, registerHeading, pinnedLabel } = useActivityGroupPinning(
    groups.map((group) => group.label),
    viewportRef,
  )
  const topLabel = pinnedLabel ?? groups[0]?.label
  const scrollElementRef = useRef<HTMLDivElement | null>(null)
  const [pendingFocusIndex, setPendingFocusIndex] = useState<number | null>(null)
  // TanStack Virtual owns mutable measurement state, so the React Compiler cannot memoize this component.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: virtualItems.length,
    getScrollElement: () => scrollElementRef.current,
    getItemKey: (index) => virtualItems[index]?.key ?? index,
    estimateSize: (index) => (virtualItems[index]?.type === "heading" ? 28 : 88),
    // Mount the initial overscan before the viewport ref attaches.
    initialRect: { width: 0, height: 1 },
    initialOffset: initialScrollOffset,
    ...(initialMeasurementsCache ? { initialMeasurementsCache } : {}),
    // Three items keep short wheel and keyboard moves mounted without retaining
    // a large part of the session list.
    overscan: 3,
    rangeExtractor: (range) => {
      const indexes = new Set(defaultRangeExtractor(range))

      // Keep the few group headings mounted for semantic grouping and pinning.
      virtualItems.forEach((item, index) => {
        if (item.type === "heading") indexes.add(index)
      })
      if (pendingFocusIndex !== null) indexes.add(pendingFocusIndex)
      if (onSelect && rovingKey) {
        const tabStopIndex = virtualItems.findIndex(
          (item) => item.type === "row" && item.item.key === rovingKey,
        )
        if (tabStopIndex >= 0) indexes.add(tabStopIndex)
      }

      const activeElement = document.activeElement
      if (activeElement && scrollElementRef.current?.contains(activeElement)) {
        const focusedItem = activeElement.closest<HTMLElement>("[data-index]")
        const focusedIndex = Number(focusedItem?.dataset.index)
        if (Number.isInteger(focusedIndex)) indexes.add(focusedIndex)
      }

      return [...indexes].sort((left, right) => left - right)
    },
  })
  // The restored measurements preserve the old layout. Do not move the saved
  // offset when an uncached row receives its first measurement.
  virtualizer.shouldAdjustScrollPositionOnItemSizeChange = initialMeasurementsCache?.length
    ? keepScrollOffset
    : undefined
  const measuredItems = virtualizer.getVirtualItems()
  const measureAndRestoreFocus = useCallback(
    (node: HTMLDivElement | null) => {
      virtualizer.measureElement(node)
      if (!node || Number(node.dataset.index) !== pendingFocusIndex) return
      node.querySelector<HTMLElement>("[data-session-row]")?.focus()
      setPendingFocusIndex(null)
    },
    [pendingFocusIndex, virtualizer],
  )
  const focusVirtualRow = useCallback(
    (index: number) => {
      const nextRow = scrollElementRef.current?.querySelector<HTMLElement>(
        `[data-virtual-kind="row"][data-index="${index}"]`,
      )
      if (nextRow) {
        nextRow.querySelector<HTMLElement>("[data-session-row]")?.focus()
        return
      }
      setPendingFocusIndex(index)
      virtualizer.scrollToIndex(index, { align: "auto" })
    },
    [virtualizer],
  )
  const moveVirtualFocus = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      const target = event.target instanceof Element ? event.target : null
      const currentItem = target?.closest<HTMLElement>('[data-virtual-kind="row"]')
      const sessionRow = target?.closest<HTMLElement>("[data-session-row]")
      if (!currentItem || !sessionRow) return

      if (
        onSelect &&
        target === sessionRow &&
        !event.altKey &&
        !event.ctrlKey &&
        !event.metaKey &&
        ["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)
      ) {
        const currentIndex = Number(currentItem.dataset.index)
        const currentPosition = selectableRowIndexes.indexOf(currentIndex)
        let nextIndex: number | undefined
        if (event.key === "Home") nextIndex = selectableRowIndexes[0]
        else if (event.key === "End") nextIndex = selectableRowIndexes.at(-1)
        else {
          const offset = event.key === "ArrowDown" ? 1 : -1
          nextIndex = selectableRowIndexes[currentPosition + offset]
        }
        const nextItem = nextIndex === undefined ? undefined : virtualItems[nextIndex]
        if (
          nextIndex === undefined ||
          nextItem?.type !== "row" ||
          !nextItem.item.entry.sessionId
        )
          return
        event.preventDefault()
        onSelect(nextItem.item.entry)
        focusVirtualRow(nextIndex)
        return
      }

      if (onSelect) return
      if (event.key !== "Tab" || event.altKey || event.ctrlKey || event.metaKey) return
      if (!target?.closest('[role="button"][tabindex="0"]')) return
      const currentPosition = rowIndexes.indexOf(Number(currentItem.dataset.index))
      const nextIndex = rowIndexes[currentPosition + (event.shiftKey ? -1 : 1)]
      if (nextIndex === undefined) return
      event.preventDefault()
      focusVirtualRow(nextIndex)
    },
    [focusVirtualRow, onSelect, rowIndexes, selectableRowIndexes, virtualItems],
  )
  const assignVirtualViewportRef = useCallback(
    (node: HTMLDivElement | null) => {
      scrollElementRef.current = node
      const cleanup = assignViewportRef(node)
      if (!node) return cleanup
      return () => {
        onMeasurementsChange?.(virtualizer.takeSnapshot())
        scrollElementRef.current = null
        cleanup?.()
      }
    },
    [assignViewportRef, onMeasurementsChange, virtualizer],
  )

  const resolvedEmptyTitle =
    emptyTitle ?? (days === 1 ? "No sessions today" : `No sessions in the last ${days} days`)

  return (
    <section
      aria-label="Sessions"
      data-tauri-drag-region={draggableHeader ? "" : undefined}
      className="flex h-full min-h-0 flex-col pt-2"
      onKeyDownCapture={moveVirtualFocus}
    >
      <span className="sr-only" aria-live="polite" aria-atomic="true">
        {visibleCount === 0 ? resolvedEmptyTitle : ""}
      </span>

      {topLabel && (
        <div
          data-testid="activity-pinned-group-label"
          data-tauri-drag-region={draggableHeader ? "deep" : undefined}
          // The inset matches the cards, so the label sits on their left
          // edge. The type matches the usage view's group labels.
          className="mb-1 flex h-7 shrink-0 items-center justify-between gap-2 px-3 type-caption font-medium tracking-wide uppercase text-label-tertiary"
        >
          <span>{topLabel}</span>
          {onBadgeMetricChange && (
            <SegmentedControl
              options={[
                { value: "cost", label: "$" },
                { value: "weeklyPercent", label: "% week" },
                ...(fiveHourAvailable
                  ? [{ value: "fiveHourPercent" as const, label: "% 5h" }]
                  : []),
              ]}
              value={selectedMetric}
              onChange={onBadgeMetricChange}
              ariaLabel="Session badge metric"
              className="normal-case"
            />
          )}
        </div>
      )}

      <ScrollPane
        topEdgeFade
        viewportRef={assignVirtualViewportRef}
        viewportClassName={cn("px-3", visibleCount === 0 && "[&>div]:h-full")}
      >
        {visibleCount === 0 ? (
          <EmptySessionList title={resolvedEmptyTitle} description={emptyDescription} />
        ) : (
          <SessionTooltipOwner>
            <div>
              <div
                role="list"
                className="relative"
                style={{ height: virtualizer.getTotalSize() }}
              >
                {groups.map((group) => {
                  const headingId = groupHeadingId(group.label)
                  return (
                    <section
                      key={group.label}
                      aria-labelledby={headingId}
                      className="absolute inset-x-0 top-0"
                    >
                      {measuredItems.flatMap((measuredItem) => {
                        const virtualItem = virtualItems[measuredItem.index]
                        if (!virtualItem || virtualItem.groupLabel !== group.label) return []

                        return (
                          <div
                            key={virtualItem.key}
                            ref={measureAndRestoreFocus}
                            data-index={measuredItem.index}
                            data-virtual-kind={virtualItem.type}
                            {...(virtualItem.type === "row"
                              ? {
                                  role: "listitem" as const,
                                  "aria-posinset": virtualItem.rowPosition,
                                  "aria-setsize": visibleCount,
                                }
                              : {})}
                            className={cn(
                              "absolute top-0 left-0 w-full",
                              ((virtualItem.type === "heading" && virtualItem.groupIndex > 0) ||
                                (virtualItem.type === "row" && virtualItem.itemIndex > 0)) &&
                                "pt-2",
                            )}
                            style={{ transform: `translateY(${measuredItem.start}px)` }}
                          >
                            {virtualItem.type === "heading" ? (
                              <h3
                                ref={registerHeading(group.label)}
                                id={headingId}
                                className={
                                  virtualItem.groupIndex === 0
                                    ? "sr-only"
                                    : "py-1 type-caption font-medium tracking-wide uppercase text-label-tertiary"
                                }
                              >
                                {group.label}
                              </h3>
                            ) : (
                              <SessionRow
                                entry={virtualItem.item.entry}
                                active={active}
                                selected={virtualItem.item.key === selectedKey}
                                {...(onSelect && virtualItem.item.entry.sessionId
                                  ? {
                                      tabIndex: virtualItem.item.key === rovingKey ? 0 : -1,
                                      onSelect: () => onSelect(virtualItem.item.entry),
                                      ...(onOpenDetail
                                        ? {
                                            onOpenDetail: () =>
                                              onOpenDetail(virtualItem.item.entry),
                                          }
                                        : {}),
                                    }
                                  : {})}
                                showCost={selectedMetric === "cost"}
                                hygiene={
                                  virtualItem.item.entry.sessionId
                                    ? sessionHygieneFor(hygieneBySession, {
                                        agent: virtualItem.item.entry.agent,
                                        sessionId: virtualItem.item.entry.sessionId,
                                        wslDistro: virtualItem.item.entry.wslDistro ?? null,
                                      })
                                    : INITIAL_SESSION_HYGIENE
                                }
                                {...(onOpenSession
                                  ? {
                                      onOpen: () => {
                                        onMeasurementsChange?.(virtualizer.takeSnapshot())
                                        onOpenSession(virtualItem.item.entry)
                                      },
                                    }
                                  : {})}
                                {...(renderAgentIcon ? { renderAgentIcon } : {})}
                                {...(wslIcon ? { wslIcon } : {})}
                                {...(selectedMetric !== "cost"
                                  ? {
                                      limitBadge: sessionLimitBadge(
                                        selectedMetric,
                                        virtualItem.item.entry.sessionId
                                          ? allocationBySession.get(
                                              `${localSessionKey(
                                                virtualItem.item.entry.agent,
                                                virtualItem.item.entry.sessionId,
                                                virtualItem.item.entry.wslDistro,
                                              )}:${selectedMetric === "weeklyPercent" ? "weekly" : "fiveHour"}`,
                                            )
                                          : undefined,
                                      ),
                                    }
                                  : {})}
                              />
                            )}
                          </div>
                        )
                      })}
                    </section>
                  )
                })}
              </div>
              <div className="h-3" aria-hidden="true" />
            </div>
          </SessionTooltipOwner>
        )}
      </ScrollPane>
    </section>
  )
}
