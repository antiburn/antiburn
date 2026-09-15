import { ArrowRight } from "lucide-react"

import { SessionRow, type SessionListEntry } from "../../../components/session/SessionList"
import { SkeletonCard } from "../../../components/ui/Skeleton"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { INITIAL_SESSION_HYGIENE } from "../../../lib/presentation/sessionHygiene"
import {
  sessionHygieneFor,
  sessionHygieneIdentities,
  useSessionHygiene,
} from "../../../lib/useSessionHygiene"
import { OVERVIEW_RECENT_SESSION_COUNT } from "../MainOverviewSession"

/**
 * The newest sessions as compact Sessions rows, one line each, under an
 * "All sessions" link. A row click selects that session in Sessions.
 */
export function OverviewRecentSessions({
  entries,
  loading = false,
  onSelect,
  onOpenAll,
}: {
  entries: SessionListEntry[] | null
  loading?: boolean
  onSelect: (entry: SessionListEntry) => void
  onOpenAll: () => void
}) {
  const hygieneBySession = useSessionHygiene(sessionHygieneIdentities(entries ?? []))
  return (
    <section
      aria-label="Recent sessions"
      aria-busy={loading || undefined}
      className="flex flex-col gap-[var(--space-sm)]"
    >
      <div className="flex items-baseline justify-between">
        <h2 className="type-caption text-label-secondary">Recent</h2>
        <button
          type="button"
          onClick={onOpenAll}
          className="inline-flex items-center gap-1 type-caption text-label-secondary hover:text-label hover:underline hover:underline-offset-[3px]"
        >
          All sessions
          <ArrowRight size={12} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>
      {entries ? (
        entries.length > 0 ? (
          <ul className="flex flex-col gap-1.5">
            {entries.map((entry) => (
              <li key={`${entry.agent}:${entry.sessionId ?? entry.timestamp}`}>
                <SessionRow
                  entry={entry}
                  hygiene={
                    entry.sessionId
                      ? sessionHygieneFor(hygieneBySession, {
                          agent: entry.agent,
                          sessionId: entry.sessionId,
                          wslDistro: entry.wslDistro ?? null,
                        })
                      : INITIAL_SESSION_HYGIENE
                  }
                  renderAgentIcon={renderAgentIcon}
                  showCost
                  compact
                  {...(entry.sessionId ? { onSelect: () => onSelect(entry) } : {})}
                />
              </li>
            ))}
          </ul>
        ) : (
          <p className="type-callout text-label-secondary">No sessions yet.</p>
        )
      ) : (
        <div className="flex flex-col gap-1.5" aria-hidden="true">
          {Array.from({ length: OVERVIEW_RECENT_SESSION_COUNT }, (_, index) => (
            <SkeletonCard key={index} leading lines={["w-56"]} className="py-2" />
          ))}
        </div>
      )}
    </section>
  )
}
