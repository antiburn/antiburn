import type { CheckUsageSummary } from "../../lib/checkAvailability"
import { CheckSettingsDisclosure } from "./CheckSettingsDisclosure"

export function hasCurrentCheckUsage(
  usage: CheckUsageSummary | null,
  loading: boolean,
  stale: boolean,
): usage is CheckUsageSummary {
  return (
    usage !== null &&
    !loading &&
    !stale &&
    (usage.unknownOutcomes === 0 ||
      usage.confirmedCalls > 0 ||
      usage.inputTokens > 0 ||
      usage.outputTokens > 0 ||
      usage.cacheHits > 0)
  )
}

export function CheckModelUsage({
  usage,
  control,
  targetRevision,
  loading,
  stale,
}: {
  usage: CheckUsageSummary | null
  control?: string | null | undefined
  targetRevision?: number | undefined
  loading: boolean
  stale: boolean
}) {
  if (!hasCurrentCheckUsage(usage, loading, stale)) return null
  const empty =
    usage.confirmedCalls === 0 &&
    usage.unknownOutcomes === 0 &&
    usage.inputTokens === 0 &&
    usage.outputTokens === 0 &&
    usage.cacheHits === 0
  const summary = empty
    ? "No model requests recorded on this device."
    : `${usage.confirmedCalls.toLocaleString()} ${usage.confirmedCalls === 1 ? "request" : "requests"} with recorded usage${usage.estimatedUsd === null ? "" : ` · ${usage.estimatedUsd} estimated`}`
  return (
    <CheckSettingsDisclosure
      searchId="checkModelUsage"
      control={control}
      targetRevision={targetRevision}
      summary={summary}
    >
      {!empty && (
        <dl className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-1 type-footnote">
          {(
            [
              ["Input tokens", usage.inputTokens],
              ["Output tokens", usage.outputTokens],
              ["Cached results reused", usage.cacheHits],
            ] as const
          ).map(([label, value]) => (
            <div className="contents" key={label}>
              <dt>{label}</dt>
              <dd>{value.toLocaleString()}</dd>
            </div>
          ))}
        </dl>
      )}
      <p className="mt-2 type-footnote text-label-secondary">
        Recorded device usage across connections. Not a bill.
      </p>
    </CheckSettingsDisclosure>
  )
}
