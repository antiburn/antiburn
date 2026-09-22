import { useSyncExternalStore } from "react"
import { ArrowRight, GitFork } from "lucide-react"

import {
  primaryLine,
  sessionLimitAllocationKey,
  sessionLimitBadge,
  sessionRowInteractiveProps,
  type SessionListEntry,
} from "../../../components/session/SessionList"
import { BurnCheckStatus } from "../../../components/burn-checks/BurnCheckStatus"
import { SessionCostBadge } from "../../../components/session/metrics/SessionCostBadge"
import { SessionLimitBadge } from "../../../components/session/metrics/SessionLimitBadge"
import { Tooltip } from "../../../components/presentation/Tooltip"
import { SkeletonCard } from "../../../components/ui/Skeleton"
import { cn } from "../../../lib/cn"
import type { BurnCheckDetectorId, SessionHygienePayload } from "../../../lib/insightsIpc"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { modelRunNames, modelRunShortPairs } from "../../../lib/presentation/models"
import { formatTokenFigure } from "../../../lib/presentation/providerUsage"
import { relativeTime } from "../../../lib/presentation/relativeTime"
import { localSessionKey } from "../../../lib/presentation/localIdentity"
import { sessionBurnCheckPresentation } from "../../../lib/presentation/burnChecks"
import {
  INITIAL_SESSION_HYGIENE,
  sessionHygieneChecks,
} from "../../../lib/presentation/sessionHygiene"
import type {
  LiveUsageSummaryPayload,
  SessionLimitAllocationSummaryPayload,
} from "../../../lib/providerUsageIpc"
import {
  snoozedDetectorIds,
  useSnoozedBurnChecks,
  visibleSessionHygieneChecks,
} from "../../../lib/snoozedBurnChecks"
import {
  sessionHygieneFor,
  sessionHygieneIdentities,
  useSessionHygiene,
} from "../../../lib/useSessionHygiene"
import { OVERVIEW_RECENT_SESSION_COUNT } from "../MainOverviewSession"
import type { OverviewMetric } from "./OverviewUsage"

function subscribeClock(listener: () => void): () => void {
  let timer: ReturnType<typeof setInterval> | undefined
  function sync() {
    if (timer !== undefined) clearInterval(timer)
    timer = undefined
    if (!document.hidden) {
      listener()
      timer = setInterval(listener, 15_000)
    }
  }
  document.addEventListener("visibilitychange", sync)
  sync()
  return () => {
    if (timer !== undefined) clearInterval(timer)
    document.removeEventListener("visibilitychange", sync)
  }
}

function clockSnapshot(): number {
  return Math.floor(Date.now() / 1_000)
}

function subscribeInactiveClock(): () => void {
  return () => undefined
}

function OverviewRecentSessionRow({
  entry,
  hygiene,
  snoozedDetectors,
  metric,
  limitBadge,
  now,
  onSelect,
}: {
  entry: SessionListEntry
  now: number
  hygiene: SessionHygienePayload
  snoozedDetectors: ReadonlySet<BurnCheckDetectorId>
  metric: OverviewMetric
  limitBadge?: ReturnType<typeof sessionLimitBadge>
  onSelect?: () => void
}) {
  const selectionMode = !!entry.sessionId && !!onSelect
  const clickable = selectionMode
  const interactiveProps = sessionRowInteractiveProps({
    clickable,
    selectionMode,
    onSelect,
    onOpenDetail: onSelect,
  })
  const hygieneChecks = visibleSessionHygieneChecks(
    sessionHygieneChecks(hygiene),
    snoozedDetectors,
  )
  const presentation = sessionBurnCheckPresentation(hygieneChecks, hygiene.evidenceState)
  const title = primaryLine(entry)
  const modelRuns = entry.modelRuns ?? []
  const modelPairs = modelRunShortPairs(modelRuns)
  const modelNames = modelRunNames(modelRuns)
  const contextDescription = [
    `Session source: ${agentDisplayName(entry.agent)}.`,
    modelNames.length > 0 ? `Models: ${modelNames.join(", ")}.` : "",
  ]
    .filter(Boolean)
    .join(" ")
  const cost = metric === "cost" ? entry.cost : undefined

  return (
    <div
      className={cn(
        "col-span-full grid grid-cols-subgrid items-center gap-x-8 @max-[720px]:gap-x-4 px-3 py-2",
        "session-card bg-session-card rounded-(--radius-popover)",
        entry.isActive && "activity-row-active",
        clickable &&
          "cursor-pointer! hover:bg-surface-secondary/50 [&:has([data-state*=open])]:bg-surface-secondary/50",
      )}
      data-session-row-compact=""
      {...interactiveProps}
    >
      {entry.isActive && <span className="sr-only">Active session</span>}

      <span className="shrink-0 whitespace-nowrap font-mono type-metadata tabular-nums text-label-tertiary">
        {entry.isActive ? (
          <span>active</span>
        ) : (
          entry.timestamp && (
            <time
              dateTime={entry.timestamp}
              aria-label={`Last activity ${relativeTime(entry.timestamp, { now })}`}
            >
              {relativeTime(entry.timestamp, { now })}
            </time>
          )
        )}
      </span>

      <span className="flex min-w-0 items-center">
        <span className="min-w-0 truncate">
          {/* An inline-block, so the shimmer overlay (`::before`, `inset: 0`)
            gets the line box as its containing block and sits on the text. */}
          <span
            className={cn(
              "inline-block type-body font-medium! text-label",
              entry.isActive && "activity-row-title-shimmer",
            )}
            data-text={entry.isActive ? title : undefined}
            aria-label={entry.isActive ? title : undefined}
          >
            {title}
          </span>

          {modelPairs.length > 0 && (
            <span
              aria-label={contextDescription}
              className="ms-2 font-mono type-metadata text-label-tertiary"
            >
              <span aria-hidden="true" className="text-label">
                ·{" "}
              </span>
              {modelPairs.map((pair, index) => (
                <span key={`${pair.model}/${pair.thinkingMode ?? ""}`}>
                  {index > 0 && <span aria-hidden="true"> · </span>}
                  <span>{pair.model}</span>
                  {pair.thinkingMode && (
                    <span className="opacity-70"> {pair.thinkingMode}</span>
                  )}
                </span>
              ))}
            </span>
          )}
        </span>
        {entry.hasForkParent && (
          <Tooltip label="Forked from another session" delayMs={500}>
            <span
              className="ms-1 inline-flex shrink-0 text-label-tertiary"
              aria-label="Forked from another session"
            >
              <GitFork size={12} strokeWidth={2} aria-hidden="true" />
            </span>
          </Tooltip>
        )}
      </span>

      {/* A narrow page drops the tokens column. The parent grid loses the
          track at the same width, so the later cells keep their columns. */}
      {entry.totalTokens ? (
        <span
          className="@max-[720px]:hidden text-center font-mono type-footnote tabular-nums text-label-tertiary whitespace-nowrap"
          aria-label={`${formatTokenFigure(entry.totalTokens)} tokens`}
        >
          {formatTokenFigure(entry.totalTokens)} tokens
        </span>
      ) : (
        <span className="@max-[720px]:hidden" />
      )}

      <span className="flex justify-center">
        {metric === "cost"
          ? cost && <SessionCostBadge {...cost} isHighCost={false} appearance="bare" />
          : limitBadge && <SessionLimitBadge limitBadge={limitBadge} suffix="of week" plain />}
      </span>

      <Tooltip label={presentation.accessibleDescription} delayMs={150}>
        <BurnCheckStatus presentation={presentation} />
      </Tooltip>
    </div>
  )
}

export function OverviewRecentSessions({
  active = true,
  entries,
  loading = false,
  onSelect,
  onOpenAll,
  metric,
  liveUsage,
  sessionLimitAllocations,
}: {
  entries: SessionListEntry[] | null
  active?: boolean
  loading?: boolean
  onSelect: (entry: SessionListEntry) => void
  onOpenAll: () => void
  metric: OverviewMetric
  liveUsage?: LiveUsageSummaryPayload | undefined
  sessionLimitAllocations?: SessionLimitAllocationSummaryPayload | null | undefined
}) {
  const now =
    useSyncExternalStore(
      active ? subscribeClock : subscribeInactiveClock,
      clockSnapshot,
      clockSnapshot,
    ) * 1_000
  const hygieneBySession = useSessionHygiene(sessionHygieneIdentities(entries ?? []))
  const snoozes = useSnoozedBurnChecks()
  const snoozedDetectors = snoozedDetectorIds(snoozes.records)
  // Keyed the same way the Sessions list keys its allocation lookup, but for
  // the weekly lane alone: Recent shows the overall weekly share, never a
  // model-scoped supplemental window.
  const weeklyAllocationBySession = new Map(
    (sessionLimitAllocations?.allocations ?? [])
      .filter((allocation) => allocation.metric === "weekly")
      .map((allocation) => [
        sessionLimitAllocationKey(
          allocation.agent,
          allocation.sessionId,
          allocation.wslDistro,
          "weekly",
        ),
        allocation,
      ]),
  )
  return (
    <section
      aria-label="Recent sessions"
      aria-busy={loading || snoozes.status === "loading" || undefined}
      className="flex flex-col gap-[var(--space-sm)]"
    >
      <div className="flex items-baseline justify-between">
        <h2 className="type-caption text-label-secondary">Recent sessions</h2>

        <button
          type="button"
          onClick={onOpenAll}
          className="inline-flex items-center gap-1 type-caption text-label-secondary hover:text-label hover:underline hover:underline-offset-[3px]"
        >
          All sessions
          <ArrowRight size={12} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>
      {snoozes.status === "error" ? (
        <p role="status" className="type-callout text-label-secondary">
          Recent sessions are unavailable.
        </p>
      ) : entries?.length === 0 && snoozes.status === "ready" ? (
        <p className="type-callout text-label-secondary">No sessions yet.</p>
      ) : (
        <div className="overview-recent-rows grid grid-cols-[auto_1fr_auto_auto_auto] @max-[720px]:grid-cols-[auto_1fr_auto_auto] gap-y-1.5">
          {entries && snoozes.status === "ready"
            ? entries.map((entry) => (
                <OverviewRecentSessionRow
                  key={localSessionKey(
                    entry.agent,
                    entry.sessionId ?? entry.timestamp,
                    entry.wslDistro,
                  )}
                  entry={entry}
                  now={now}
                  hygiene={
                    entry.sessionId
                      ? sessionHygieneFor(hygieneBySession, {
                          agent: entry.agent,
                          sessionId: entry.sessionId,
                          wslDistro: entry.wslDistro ?? null,
                        })
                      : INITIAL_SESSION_HYGIENE
                  }
                  snoozedDetectors={snoozedDetectors}
                  metric={metric}
                  {...(metric === "allowance"
                    ? {
                        limitBadge: sessionLimitBadge(
                          "weeklyPercent",
                          entry.agent,
                          liveUsage,
                          entry.sessionId
                            ? weeklyAllocationBySession.get(
                                sessionLimitAllocationKey(
                                  entry.agent,
                                  entry.sessionId,
                                  entry.wslDistro,
                                  "weekly",
                                ),
                              )
                            : undefined,
                        ),
                      }
                    : {})}
                  {...(entry.sessionId ? { onSelect: () => onSelect(entry) } : {})}
                />
              ))
            : Array.from({ length: OVERVIEW_RECENT_SESSION_COUNT }, (_, index) => (
                <SkeletonCard
                  key={index}
                  leading
                  className="col-span-full py-2"
                  lines={["w-56"]}
                />
              ))}
        </div>
      )}
    </section>
  )
}
