import { GitBranchPlus, GitFork, SquareTerminal } from "lucide-react"
import type { ReactNode } from "react"

import { cn } from "../../lib/cn"
import { agentDisplayName, type AgentSurface } from "../../lib/presentation/agents"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { mockSessionHygiene } from "../../lib/presentation/mockSessionHygiene"
import {
  modelRunNames,
  modelRunShortNames,
  type PresentableModelRun,
} from "../../lib/presentation/models"
import { relativeTime } from "../../lib/presentation/relativeTime"
import { Tooltip } from "../presentation/Tooltip"
import { TruncatedText } from "../presentation/TruncatedText"
import { WslOriginBadge } from "../presentation/WslOriginBadge"
import { SessionHygieneBadges } from "./SessionHygieneBadges"
import { SessionCostBadge, type SessionCostBadgeProps } from "./metrics/SessionCostBadge"
import { ScrollPane } from "../ui/ScrollPane"
import { countGroupedItems, groupActivityByDay } from "../activity/activityFeedGrouping"
import { useActivityGroupPinning, type ViewportRef } from "../activity/useActivityGroupPinning"

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
  /** The scrolling viewport, for a host that needs to observe it. */
  viewportRef?: ViewportRef
  /** Frozen clock, for tests. */
  now?: Date
  renderAgentIcon?: SessionAgentIconRenderer
  /** Glyph for the WSL-origin badge. */
  wslIcon?: ReactNode
}

function primaryLine(entry: SessionListEntry): string {
  const title = entry.title?.trim()
  if (title) return title
  if (entry.sessionId) return `Session ${entry.sessionId.slice(0, 7)}`
  return agentDisplayName(entry.agent)
}

function EmptySessionList({ title, description }: { title: string; description: string }) {
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

interface SessionRowProps {
  entry: SessionListEntry
  onOpen?: () => void
  renderAgentIcon?: SessionAgentIconRenderer | undefined
  wslIcon?: ReactNode | undefined
}

/**
 * One session in the list: its identity, location, fork relationships,
 * cost, and last activity time.
 *
 * The whole card opens the session analysis. Unsupported agents open an empty
 * analysis state that explains why no data is available.
 */
function SessionRow({ entry, onOpen, renderAgentIcon, wslIcon }: SessionRowProps) {
  const clickable = !!entry.sessionId && !!onOpen
  const primary = primaryLine(entry)
  const hasRepo = entry.repo !== ""
  const modelRuns = entry.modelRuns ?? []
  const modelNames = modelRunShortNames(modelRuns)
  const hygieneSeed = entry.sessionId
    ? localSessionKey(entry.agent, entry.sessionId, entry.wslDistro)
    : `${entry.agent}|${entry.repo}|${entry.timestamp}`
  const hygieneChecks = mockSessionHygiene(hygieneSeed)

  return (
    <div
      className={cn(
        "session-row group relative flex items-start gap-3 py-4 px-2 w-full text-left rounded-md",
        "transition-colors duration-[var(--duration-fast)] ease-out",
        entry.isActive && "activity-row-active",
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

      <div className="mt-5 shrink-0">{renderAgentIcon?.(entry.agent, 18, entry.surface)}</div>

      <div className="min-w-0 flex-1 space-y-1">
        <div className="flex items-end justify-between gap-x-1">
          <div className="session-repo-names flex gap-x-2">
            {hasRepo && (
              <Tooltip
                label={
                  entry.additionalRepos?.length
                    ? `Also observed: ${entry.additionalRepos.join(", ")}`
                    : entry.repo
                }
              >
                <span className="min-w-0 truncate text-sm text-label-secondary">
                  {entry.repo}
                  {entry.additionalRepos?.length ? ` +${entry.additionalRepos.length}` : ""}
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

          <div className="relative flex shrink-0 items-center gap-1.5">
            <SessionHygieneBadges checks={hygieneChecks} />
            {entry.cost && <SessionCostBadge {...entry.cost} />}
          </div>
        </div>

        <div className="flex min-w-0 items-center gap-1">
          <TruncatedText
            className={cn("min-w-0 type-body", !entry.isActive && "text-label")}
            text={primary}
            lines={2}
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

        <div className="flex w-full justify-between min-w-0 items-center gap-x-1.5">
          <div
            className="min-w-0 type-footnote text-label-tertiary"
            {...(modelNames.length > 0 ? { title: modelRunNames(modelRuns).join("\n") } : {})}
          >
            {modelNames.length > 0 ? <TruncatedText text={modelNames.join(" · ")} /> : "\u00A0"}
          </div>

          <time
            dateTime={entry.timestamp}
            aria-label={`Last activity ${relativeTime(entry.timestamp)}`}
            className="text-sm text-label-secondary"
          >
            {relativeTime(entry.timestamp)}
          </time>
        </div>
      </div>
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
  viewportRef,
  now,
  renderAgentIcon,
  wslIcon,
}: SessionListProps) {
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

  const { assignViewportRef, registerHeading, pinnedLabel } = useActivityGroupPinning(
    groups.map((group) => group.label),
    viewportRef,
  )

  const resolvedEmptyTitle =
    emptyTitle ?? (days === 1 ? "No sessions today" : `No sessions in the last ${days} days`)

  return (
    <section aria-label="Sessions" className="flex h-full min-h-0 flex-col pt-2">
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
          <EmptySessionList title={resolvedEmptyTitle} description={emptyDescription} />
        ) : (
          <div className="space-y-3 pb-3">
            {groups.map((group, groupIndex) => {
              const headingId = `activity-${group.label.replaceAll(" ", "-").toLowerCase()}`
              return (
                <section key={group.label} aria-labelledby={headingId}>
                  <h3
                    ref={registerHeading(group.label)}
                    id={headingId}
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
                      <SessionRow
                        key={item.key}
                        entry={item.entry}
                        {...(onOpenSession ? { onOpen: () => onOpenSession(item.entry) } : {})}
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
