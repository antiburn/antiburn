import { useRef } from "react"

import type {
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
} from "../../../lib/providerUsageIpc"
import {
  liveDisplayableProviders,
  liveErrorNote,
  liveFreshnessToneClass,
  liveGraceNote,
  livePlanAccountLabel,
  liveProviderStatus,
  liveUnavailableProviders,
  liveWindows,
  orderedLiveAccounts,
} from "../../../lib/presentation/liveUsage"

import { WindowMeterRow } from "../../../components/providerUsage/UsageLimitsBar"
import { useStableAccountNumbers } from "../../../components/providerUsage/useStableAccountNumbers"
import { Skeleton } from "../../../components/ui/Skeleton"
import { useElementWidth } from "../../../lib/useElementWidth"

/** The popover's dot count, used until the group has a measured width. */
const OVERVIEW_METER_SEGMENTS = 32
/** One dot and its gap, in pixels: the popover's packing at its row width. */
const OVERVIEW_METER_PITCH = 9
/** Below this count the meter reads as a row of beads, not an instrument. */
const OVERVIEW_METER_MIN_SEGMENTS = 16

/** The dot count that packs a meter of `width` pixels like the popover's. */
export function meterSegmentsForWidth(width: number): number {
  if (width <= 0) return OVERVIEW_METER_SEGMENTS
  return Math.max(OVERVIEW_METER_MIN_SEGMENTS, Math.floor(width / OVERVIEW_METER_PITCH))
}

/**
 * One provider's meters. The group measures its own width and draws as many
 * dots as fit at the popover's pitch, so a wider card gets a longer meter
 * with the same lit share, not the same meter with wider gaps.
 */
function MeterGroup({ windows, now }: { windows: LiveUsageWindowPayload[]; now: number }) {
  const ref = useRef<HTMLDivElement | null>(null)
  const segments = meterSegmentsForWidth(useElementWidth(ref))
  return (
    <div ref={ref} className="flex flex-col gap-[var(--space-md)] pt-[var(--space-md)]">
      {windows.map((window) => (
        <WindowMeterRow
          key={window.id}
          window={window}
          now={now}
          resetPlacement="caption"
          segments={segments}
        />
      ))}
    </div>
  )
}

/**
 * The provider limits panel: one group per provider account, stacked with a
 * rule between, with a dot meter for each of its windows and the reset time
 * under each meter. The meters take the card's width and add dots as it
 * grows. The stale tag floats in the top-right corner. The panel shows no
 * local cost figure; those belong to the totals above the panel.
 */
export function OverviewProviderLimits({
  live,
  loading = false,
}: {
  live: LiveUsageSummaryPayload | null
  loading?: boolean
}) {
  const limited = live
    ? orderedLiveAccounts(liveDisplayableProviders(live)).filter(
        ({ reading }) => liveWindows(reading).length > 0,
      )
    : []
  const unavailable = live ? liveUnavailableProviders(live) : []
  const providerCounts = new Map<string, number>()
  for (const { reading } of limited) {
    providerCounts.set(reading.provider, (providerCounts.get(reading.provider) ?? 0) + 1)
  }
  const accountNumbers = useStableAccountNumbers(
    limited.map(({ key, reading }) => ({ key, provider: reading.provider })),
  )
  // The instant the elapsed notches are measured from: the snapshot's own
  // time, not the wall clock. A render must not read the clock.
  const at = live ? Date.parse(live.generatedAt) || 0 : 0
  const stale = limited.some(({ reading }) => reading.freshness === "stale")

  return (
    <section
      aria-label="Provider limits"
      aria-busy={loading || undefined}
      className="overview-provider-limits relative rounded-control bg-surface-card p-[var(--space-lg)] shadow-stats-card"
    >
      {limited.length > 0 && stale && (
        <p
          className={`type-caption absolute top-[var(--space-lg)] right-[var(--space-lg)] ${liveFreshnessToneClass("stale")}`}
        >
          Stale
        </p>
      )}
      {loading || !live ? (
        <div className="flex flex-col gap-[var(--space-2xl)]">
          {["first", "second"].map((seat) => (
            <div key={seat} className="flex flex-col gap-[var(--space-md)]">
              <Skeleton className="h-3 w-28" />
              <Skeleton className="h-3 w-full" />
              <Skeleton className="h-3 w-full" />
            </div>
          ))}
        </div>
      ) : limited.length === 0 && unavailable.length === 0 ? (
        <p className="type-callout text-label-secondary">
          No provider limits to show. Sign in with a coding tool, or turn a meter on in
          Settings.
        </p>
      ) : (
        <div className="flex flex-col divide-y divide-separator">
          {limited.map(({ reading, key }) => {
            const count = providerCounts.get(reading.provider) ?? 1
            const displayName =
              count > 1
                ? `${reading.displayName} account ${accountNumbers.get(key)}`
                : reading.displayName
            const plan = livePlanAccountLabel(reading, count)
            const status = liveProviderStatus(live, reading)
            const graceNote =
              status.kind === "grace"
                ? liveGraceNote(status.category, reading.provider, status.ageMs)
                : null
            return (
              <div
                key={key}
                role="group"
                aria-label={plan ? `${displayName}, ${plan} plan` : displayName}
                className="min-w-0 py-[var(--space-lg)] first:pt-0 last:pb-0"
              >
                <h3 className="type-footnote min-w-0 truncate pr-12 font-medium tracking-wide text-label">
                  <span className="uppercase">{displayName}</span>
                  {plan && <span className="text-label-secondary"> · {plan}</span>}
                </h3>
                {graceNote && (
                  <p className="type-footnote pt-1 text-label-tertiary">{graceNote}</p>
                )}
                <MeterGroup windows={liveWindows(reading)} now={at} />
              </div>
            )
          })}
          {unavailable.map((entry) => (
            <div
              key={entry.provider}
              role="group"
              aria-label={entry.displayName}
              className="min-w-0 py-[var(--space-lg)] first:pt-0 last:pb-0"
            >
              <h3 className="type-footnote truncate font-medium tracking-wide text-label uppercase">
                {entry.displayName}
              </h3>
              <p className="type-footnote pt-[var(--space-md)] text-label-secondary">
                {liveErrorNote(entry.category, entry.provider)}
              </p>
            </div>
          ))}
        </div>
      )}
    </section>
  )
}
