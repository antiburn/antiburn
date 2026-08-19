// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Bot, GitBranchPlus, GitFork, SquareTerminal } from "lucide-react"
import type { ReactNode } from "react"

import { cn } from "../../lib/cn"
import { agentDisplayName } from "../../lib/presentation/agents"
import type { AgentSurface } from "../../lib/presentation/agents"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { Tooltip } from "../presentation/Tooltip"
import { WslOriginBadge } from "../presentation/WslOriginBadge"
import { MIN_ORCHESTRATED_SUBAGENTS } from "../../lib/types/session"
import { SessionActiveTimeBadge } from "../session/metrics/SessionActiveTimeBadge"
import {
  SessionCostBadge,
  type SessionCostBadgeProps,
} from "../session/metrics/SessionCostBadge"
import { ScrollPane } from "../ui/ScrollPane"
import { RowTimeCorner, TruncatedText } from "./ActivityRowPrimitives"
import {
  ROW_ACTIVE_CLASS,
  ROW_CLASS,
  ROW_CORNER_GUTTER_CLASS,
  ROW_LAYOUT_CLASS,
  ROW_TITLE_CLASS,
  SECONDARY_DETAIL_REVEAL_CLASS,
} from "./activityRow"
import { countGroupedItems, groupActivityByDay } from "./activityFeedGrouping"
import { useActivityGroupPinning } from "./useActivityGroupPinning"

import "../../styles/session-rows.css"

/**
 * Renders the icon for an agent, optionally marked with the surface the
 * session came from. An injected slot, so this layer ships no artwork.
 */
type ActivityAgentIconRenderer = (
  slug: string,
  size: number,
  surface?: AgentSurface,
) => ReactNode

/** One local coding session in the list. */
export interface LocalActivityEntry {
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
  /** Whether the transcript is still being written. */
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
  /** Sub-agents this session launched; 0 or absent when it launched none. */
  subagentCount?: number | undefined
  /** Display values for the cost pill; omit when nothing priced the session. */
  cost?: SessionCostBadgeProps | null | undefined
  /** Active/overall time for the time pill; omit when unmeasured. */
  activeTime?: { activeSecs: number; durationSecs?: number | null } | null | undefined
  /** What the corner timestamp means. Defaults to "Last activity". */
  timeLabel?: string | undefined
}

export interface LocalActivityListProps {
  /** Sessions to show. Ordering and grouping are this component's job. */
  entries: LocalActivityEntry[]
  /** Visible calendar-day window. Also drives the empty copy. */
  days: number
  /** Headline for the empty state; defaults to the range-aware wording. */
  emptyTitle?: string
  emptyDescription?: string
  /** Open a session's analytics. Omitted leaves rows inert. */
  onOpenSession?: (entry: LocalActivityEntry) => void
  /**
   * Open a session's sub-agent roster from the fan-out pill. Falls back to
   * {@link onOpenSession}, which is where the roster lives.
   */
  onOpenOrchestration?: (entry: LocalActivityEntry) => void
  /** The scrolling viewport, for a host that needs to observe it. */
  viewportRef?: (node: HTMLDivElement | null) => void
  /** Frozen clock, for tests. */
  now?: Date
  renderAgentIcon?: ActivityAgentIconRenderer
  /** Glyph for the WSL-origin badge. */
  wslIcon?: ReactNode
}

/** Title for a row: the resolved title, then a short id, then the agent name. */
function primaryLine(entry: LocalActivityEntry): string {
  const title = entry.title?.trim()
  if (title) return title
  if (entry.sessionId) return `Session ${entry.sessionId.slice(0, 7)}`
  return agentDisplayName(entry.agent)
}

function EmptyActivity({ title, description }: { title: string; description: string }) {
  return (
    <div className="flex h-full items-center justify-center px-6 text-center">
      <div className="flex max-w-[230px] flex-col items-center">
        <div className="mb-3 flex h-10 w-10 items-center justify-center rounded-full bg-surface-secondary text-label-tertiary">
          <SquareTerminal size={20} strokeWidth={1.75} aria-hidden="true" />
        </div>
        <p className="type-callout font-medium text-label-secondary">{title}</p>
        <p className="mt-1.5 max-w-[230px] type-footnote text-label-tertiary">{description}</p>
      </div>
    </div>
  )
}

interface SessionActivityRowProps {
  entry: LocalActivityEntry
  /** Opens this session's analytics. Omitted leaves the row inert. */
  onOpen?: () => void
  /** Opens the sub-agent roster from the fan-out pill. */
  onOpenOrchestration?: () => void
  renderAgentIcon?: ActivityAgentIconRenderer | undefined
  wslIcon?: ReactNode | undefined
}

/**
 * One local session in the list: its identity, repository, branch, fork
 * relationships, cost, active time, and sub-agent fan-out.
 *
 * The whole card opens the session's analytics, including for agents the
 * engine cannot analyze — those land on the analytics view's own empty state,
 * which explains why, rather than being silently unclickable here.
 */
function SessionActivityRow({
  entry,
  onOpen,
  onOpenOrchestration,
  renderAgentIcon,
  wslIcon,
}: SessionActivityRowProps) {
  const clickable = !!entry.sessionId && !!onOpen
  const primary = primaryLine(entry)
  const hasRepo = entry.repo !== ""
  const subagentCount = entry.subagentCount ?? 0
  const isOrchestrator = subagentCount >= MIN_ORCHESTRATED_SUBAGENTS && !!onOpen
  const openRoster = onOpenOrchestration ?? onOpen

  return (
    <div
      className={cn(
        ROW_CLASS,
        entry.isActive && `${ROW_ACTIVE_CLASS} isolate`,
        ROW_LAYOUT_CLASS,
        "transition-colors duration-[120ms]",
        clickable &&
          "cursor-pointer hover:bg-surface-hover [&:has([data-state*=open])]:bg-surface-hover",
      )}
      {...(clickable
        ? {
            role: "button" as const,
            tabIndex: 0,
            onClick: onOpen,
            onKeyDown: (event: React.KeyboardEvent) => {
              // Only when the row itself has focus: a nested control's Enter
              // belongs to that control, not to the card behind it.
              if (
                event.currentTarget === event.target &&
                (event.key === "Enter" || event.key === " ")
              ) {
                event.preventDefault()
                onOpen?.()
              }
            },
          }
        : {})}
    >
      {entry.isActive && <span className="sr-only">Active session</span>}
      <div className="mt-0.5 shrink-0">{renderAgentIcon?.(entry.agent, 18, entry.surface)}</div>
      <div className="min-w-0 flex-1 space-y-0.5">
        <div className={cn(ROW_CORNER_GUTTER_CLASS, "flex min-w-0 items-center gap-1")}>
          <TruncatedText
            className={cn(
              ROW_TITLE_CLASS,
              "min-w-0 truncate type-callout",
              !entry.isActive && "text-label",
            )}
            text={primary}
            shimmer={entry.isActive}
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

        {(entry.branch || hasRepo || entry.wslDistro) && (
          <div className="flex min-w-0 items-center gap-1.5">
            {hasRepo && (
              <Tooltip
                label={
                  entry.additionalRepos?.length
                    ? `Also observed: ${entry.additionalRepos.join(", ")}`
                    : entry.repo
                }
              >
                <span className="inline-flex h-[15px] max-w-[55%] shrink-0 items-center rounded bg-label/[0.06] px-1.5">
                  <TruncatedText
                    className="min-w-0 truncate type-footnote leading-[13px] font-medium tracking-wide text-label-secondary"
                    text={`${entry.repo}${entry.additionalRepos?.length ? ` +${entry.additionalRepos.length}` : ""}`}
                  />
                </span>
              </Tooltip>
            )}
            <WslOriginBadge distro={entry.wslDistro} {...(wslIcon ? { icon: wslIcon } : {})} />
            {entry.branch && (
              <TruncatedText
                className="min-w-0 truncate type-footnote leading-[13px] tracking-wide text-label-secondary"
                text={entry.branch}
              />
            )}
          </div>
        )}

        {(isOrchestrator || entry.cost || entry.activeTime) && (
          <div className="flex min-h-[15px] min-w-0 items-center gap-1.5">
            {entry.cost && <SessionCostBadge {...entry.cost} />}
            {entry.activeTime && (
              <span className={cn("flex", SECONDARY_DETAIL_REVEAL_CLASS, "shrink-0")}>
                <SessionActiveTimeBadge
                  activeSecs={entry.activeTime.activeSecs}
                  {...(entry.activeTime.durationSecs != null
                    ? { durationSecs: entry.activeTime.durationSecs }
                    : {})}
                />
              </span>
            )}
            {isOrchestrator && openRoster && (
              <span className={cn("flex", SECONDARY_DETAIL_REVEAL_CLASS, "shrink-0")}>
                <Tooltip
                  label={`Orchestrated ${subagentCount} sub-agents — view roster`}
                  delayMs={700}
                >
                  <button
                    type="button"
                    onClick={(event) => {
                      // The card behind this also opens something; without
                      // this the click would fire both.
                      event.stopPropagation()
                      openRoster()
                    }}
                    className="flex shrink-0 items-center gap-0.5 rounded-full bg-system-indigo/15 px-1.5 py-px leading-[13px] text-system-indigo-text transition-colors hover:bg-system-indigo/25"
                    aria-label={`Orchestrated ${subagentCount} sub-agents, view roster`}
                  >
                    <Bot size={11} aria-hidden="true" className="shrink-0" />
                    <span className="type-caption font-medium tabular-nums">
                      {subagentCount}
                    </span>
                  </button>
                </Tooltip>
              </span>
            )}
          </div>
        )}
      </div>

      <RowTimeCorner
        timestamp={entry.timestamp}
        label={entry.timeLabel ?? "Last activity"}
        revealClassName={SECONDARY_DETAIL_REVEAL_CLASS}
      />
    </div>
  )
}

/**
 * A scrolling list of local coding sessions, bucketed by calendar day.
 *
 * Entirely prop-driven: entries, the visible range, and every action arrive
 * from the host. The list owns ordering, day grouping, the sticky day label,
 * and the row treatment — and nothing else.
 */
export function LocalActivityList({
  entries,
  days,
  emptyTitle,
  emptyDescription = "Coding sessions appear here as they are discovered on this machine.",
  onOpenSession,
  onOpenOrchestration,
  viewportRef,
  now,
  renderAgentIcon,
  wslIcon,
}: LocalActivityListProps) {
  const items = entries.map((entry, index) => ({
    entry,
    at: entry.timestamp,
    // Key by stable identity, never the timestamp: a live session's timestamp
    // advances as it works, and embedding it would remount the row — dropping
    // hover state and re-running mount effects — on every tick.
    key: entry.sessionId
      ? localSessionKey(entry.agent, entry.sessionId, entry.wslDistro)
      : `${entry.agent}|${index}`,
  }))

  const groups = groupActivityByDay(items, { days, ...(now ? { now } : {}) })
  const visibleCount = countGroupedItems(groups)

  const { assignViewportRef, registerHeading, pinnedLabel } = useActivityGroupPinning(
    groups.map((group) => group.label),
    viewportRef,
  )

  const resolvedEmptyTitle =
    emptyTitle ?? (days === 1 ? "No sessions today" : `No sessions in the last ${days} days`)

  return (
    <section aria-label="Activity feed" className="flex h-full min-h-0 flex-col pt-2">
      {/* One live-region announcement for the whole list, rather than a
          placeholder per row. */}
      <span className="sr-only" aria-live="polite" aria-atomic="true">
        {visibleCount === 0 ? resolvedEmptyTitle : ""}
      </span>

      {pinnedLabel && (
        <div
          data-testid="activity-pinned-group-label"
          aria-hidden="true"
          className="shrink-0 px-4 pb-1 type-caption font-medium tracking-wide uppercase text-label-tertiary"
        >
          {pinnedLabel}
        </div>
      )}

      <ScrollPane
        topEdgeFade
        viewportRef={assignViewportRef}
        viewportClassName={cn("px-2", visibleCount === 0 && "[&>div]:h-full")}
      >
        {visibleCount === 0 ? (
          <EmptyActivity title={resolvedEmptyTitle} description={emptyDescription} />
        ) : (
          <div className="space-y-3 pb-3">
            {groups.map((group, groupIndex) => {
              const headingId = `activity-${group.label.replaceAll(" ", "-").toLowerCase()}`
              return (
                <section key={group.label} aria-labelledby={headingId}>
                  <h3
                    ref={registerHeading(group.label)}
                    id={headingId}
                    // The first day's heading is duplicated by the sticky
                    // label above the viewport, so it is announced but not
                    // painted twice.
                    className={
                      groupIndex === 0
                        ? "sr-only"
                        : "px-2 pb-1 type-caption font-medium tracking-wide uppercase text-label-tertiary"
                    }
                  >
                    {group.label}
                  </h3>
                  <div className="space-y-1">
                    {group.items.map((item) => (
                      <SessionActivityRow
                        key={item.key}
                        entry={item.entry}
                        {...(onOpenSession ? { onOpen: () => onOpenSession(item.entry) } : {})}
                        {...(onOpenOrchestration
                          ? { onOpenOrchestration: () => onOpenOrchestration(item.entry) }
                          : {})}
                        {...(renderAgentIcon ? { renderAgentIcon } : {})}
                        {...(wslIcon ? { wslIcon } : {})}
                      />
                    ))}
                  </div>
                </section>
              )
            })}
          </div>
        )}
      </ScrollPane>
    </section>
  )
}
