import { GitBranchPlus, GitFork, SquareTerminal } from "lucide-react"
import type { ReactNode } from "react"

import { cn } from "../../lib/cn"
import { agentDisplayName, type AgentSurface } from "../../lib/presentation/agents"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { sessionHygieneChecks } from "../../lib/presentation/sessionHygiene"
import {
  modelRunNames,
  modelRunShortPairs,
  type PresentableModelRun,
} from "../../lib/presentation/models"
import { useSessionHygiene } from "../../lib/useSessionHygiene"
import { Tooltip } from "../presentation/Tooltip"
import { TruncatedText } from "../presentation/TruncatedText"
import { WslOriginBadge } from "../presentation/WslOriginBadge"
import { SessionStatusBar } from "./SessionStatusBar"
import { type SessionCostBadgeProps } from "./metrics/SessionCostBadge"
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
        <p className="type-body font-medium! text-label-secondary">{title}</p>
        <p className="mt-1.5 max-w-[230px] type-callout text-label-tertiary">{description}</p>
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
  const modelPairs = modelRunShortPairs(modelRuns)
  const hygiene = useSessionHygiene(entry.agent, entry.sessionId, entry.wslDistro)
  const hygieneChecks = sessionHygieneChecks(hygiene)

  return (
    <div
      className={cn(
        // The provider mark holds the first column, so every line of text
        // starts on the verdict's left edge. The column equals the icon size.
        "group relative grid w-full grid-cols-[14px_minmax(0,1fr)] gap-x-1.5 gap-y-1 text-left",
        "rounded-[var(--radius-popover)] bg-surface-card px-3 py-3",
        "transition-colors duration-[var(--duration-fast)] ease-out",
        entry.isActive && "activity-row-active",
        clickable &&
          "cursor-pointer hover:bg-surface-secondary [&:has([data-state*=open])]:bg-surface-secondary",
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

      <span className="flex h-[14px] items-center justify-center">
        {renderAgentIcon?.(entry.agent, 14, entry.surface)}
      </span>

      <SessionStatusBar
        checks={hygieneChecks}
        evidenceState={hygiene.evidenceState}
        cost={entry.cost ?? null}
        timestamp={entry.timestamp}
      />

      <div className="col-start-2 min-w-0 space-y-1">
        {/* The title runs the full row width; the time lives in the
            status line and shows on hover. */}
        <div className="flex min-w-0 items-center gap-1">
          <TruncatedText
            // One ink for every title. The shimmer overlay is the only
            // difference an active session shows.
            className="min-w-0 type-body-large text-label"
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

        {(modelPairs.length > 0 || hasRepo || entry.branch || entry.wslDistro) && (
          // The models and the repo lines read as one metadata unit, so
          // they sit closer to each other than to the title.
          <div className="space-y-px">
            {modelPairs.length > 0 && (
              <div
                // The name anchors each unit at 500; the thinking mode sits a
                // size down after a space. Type does the separating, not a
                // slash.
                className="min-w-0 truncate type-callout text-label-tertiary"
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
            )}

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
          </div>
        )}
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
          // The inset matches the cards, so the label sits on their left
          // edge. The type matches the usage view's group labels.
          className="shrink-0 px-3 py-1 type-caption font-medium tracking-wide uppercase text-label-tertiary"
        >
          {pinnedLabel}
        </div>
      )}

      <ScrollPane
        topEdgeFade
        viewportRef={assignViewportRef}
        viewportClassName={cn("px-3", visibleCount === 0 && "[&>div]:h-full")}
      >
        {visibleCount === 0 ? (
          <EmptySessionList title={resolvedEmptyTitle} description={emptyDescription} />
        ) : (
          <div className="space-y-2 pb-3">
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
                        : "py-1 type-caption font-medium tracking-wide uppercase text-label-tertiary"
                    }
                  >
                    {group.label}
                  </h3>

                  <div className="space-y-2">
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
