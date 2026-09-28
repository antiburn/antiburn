import { Search } from "lucide-react"
import type { DeepSearchSnapshot } from "./DeepSearchSession"

export function deepSearchSummary(state: DeepSearchSnapshot): string {
  const response = state.response
  if (state.phase === "idle")
    return "Search messages, reasoning and tool content on this device."
  if (state.phase === "failed") return "Content search could not finish. Results are partial."
  if (state.phase === "stopped") return "Stopped · results so far"
  if (state.phase === "partial") return "Search incomplete · results are partial"
  if (state.phase === "searching") return "Searching retained content"
  if (response?.status === "finished_unavailable") return "Finished · some content unavailable"
  return "Finished · retained scope searched"
}

export function DeepSearchEmptyState({
  state,
  scopeLabel,
}: {
  state: DeepSearchSnapshot
  scopeLabel: string
}) {
  const coverage = state.response?.coverage
  const unavailable = Boolean(coverage?.unavailableSessions || coverage?.changedSessions)
  return (
    <section
      aria-label="Search empty state"
      className="flex flex-col items-center justify-center px-8 py-12 text-center"
    >
      <Search size={28} aria-hidden className="mb-3 text-label-tertiary" />
      <h3 className="type-body text-label">
        {unavailable ? "No matches in available content" : "No matches found"}
      </h3>
      <p className="mt-1 type-callout text-label-tertiary tabular-nums">
        {coverage?.inspectedSessions} sessions searched · {scopeLabel}
      </p>
      <p className="mt-1 type-callout text-label-secondary">
        Try another word or a shorter phrase.
      </p>
      {unavailable && (
        <p className="mt-2 type-caption text-label-secondary">
          {coverage?.unavailableSessions} unavailable · {coverage?.changedSessions} changed
        </p>
      )}
      {Boolean(coverage?.ingestionTruncatedParts) && (
        <p className="mt-1 type-caption text-label-secondary">
          {coverage?.ingestionTruncatedParts} parts truncated during ingestion.
        </p>
      )}
    </section>
  )
}

export function DeepSearchPanel({
  state,
  scopeLabel,
  onStop,
  onContinue,
  onRetry,
}: {
  state: DeepSearchSnapshot
  scopeLabel: string
  onStop: () => void
  onContinue: () => void
  onRetry: () => void
}) {
  const response = state.response
  const complete = state.phase === "finished" && response?.coverage.scopeExhausted
  const active = state.phase === "searching"
  return (
    <section
      aria-label="Retained content search"
      className="border-t border-separator px-4 py-2"
    >
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
        <div className="min-w-0 flex-1 type-caption text-label-secondary">
          {complete && response ? (
            <>
              <p className="flex flex-wrap items-baseline gap-x-2">
                <strong className="type-callout text-label">
                  {response.totalMatchingSessions === 0
                    ? "No matches"
                    : `${response.totalMatchingSessions} matches`}
                </strong>
                <span>· {scopeLabel}</span>
                <span className="tabular-nums">
                  · {response.coverage.inspectedSessions} sessions searched
                </span>
                {response.totalMatchingSessions > 100 && <span>· Showing top 100</span>}
              </p>
              {response.status === "finished_unavailable" && <p>Some content unavailable</p>}
            </>
          ) : (
            <>
              <p>
                {scopeLabel} · {deepSearchSummary(state)}
              </p>
              {response && (
                <p className="tabular-nums">
                  {response.coverage.inspectedSessions} of {response.coverage.eligibleSessions}{" "}
                  sessions · {response.totalMatchingSessions} matches
                  {response.totalMatchingSessions > 100 && " · Showing top 100"}
                </p>
              )}
            </>
          )}
        </div>
        {active ? (
          <button
            type="button"
            className="app-search-control shrink-0 rounded-control px-3 type-callout text-label hover:bg-surface-hover"
            onClick={onStop}
          >
            Stop
          </button>
        ) : response?.continuationAvailable || (state.phase === "stopped" && state.settling) ? (
          <button
            type="button"
            disabled={state.settling}
            className="app-search-control shrink-0 rounded-control px-3 type-callout text-label hover:bg-surface-hover disabled:opacity-50"
            onClick={onContinue}
          >
            Continue
          </button>
        ) : state.phase === "failed" ||
          (state.phase === "stopped" && !response && !state.settling) ? (
          <button
            type="button"
            className="app-search-control shrink-0 rounded-control px-3 type-callout text-label hover:bg-surface-hover"
            onClick={onRetry}
          >
            Retry content search
          </button>
        ) : null}
      </div>
      {response &&
        (response.coverage.unavailableSessions > 0 ||
          response.coverage.changedSessions > 0 ||
          response.coverage.ingestionTruncatedParts > 0) && (
          <p className="type-caption text-label-secondary">
            {response.coverage.unavailableSessions} unavailable ·{" "}
            {response.coverage.changedSessions} changed ·{" "}
            {response.coverage.ingestionTruncatedParts} parts truncated during ingestion.
          </p>
        )}
    </section>
  )
}
