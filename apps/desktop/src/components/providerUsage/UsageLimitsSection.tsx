// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronDown, Loader2 } from "lucide-react"

import { cn } from "../../lib/cn"
import type {
  LiveProviderUsagePayload,
  LiveUsageSummaryPayload,
  ProviderUsagePayload,
} from "../../lib/ipc"
import { EMPTY_LIVE_USAGE } from "../../lib/ipc"
import {
  liveFreshnessToneClass,
  liveSourceNote,
  liveWindows,
} from "../../lib/presentation/liveUsage"
import { LiveUsageWindowRows } from "./LiveUsageWindowRows"
import { ProviderUsageChips } from "./ProviderUsageChips"

export interface UsageLimitsSectionProps {
  /** Local spend, for the collapsed row's chips — the same figures the chips
   * always showed, independent of which providers stated a limit. */
  providers: readonly ProviderUsagePayload[]
  /** The provider's own limit figures, when a source could prove any. */
  live?: LiveUsageSummaryPayload
  /**
   * The instant countdowns and elapsed markers are measured from. Defaults to
   * when the shell collected the snapshot — a render must not read the clock.
   */
  now?: number
  /** Whether the section shows its per-provider rows or the collapsed chips. */
  expanded: boolean
  onToggleExpanded: () => void
  /** Whether a refresh is in flight, for the small spinner beside the toggle. */
  refreshing: boolean
  /** Open the full Usage view, from the collapsed row's overflow or panel. */
  onViewAll: () => void
}

/**
 * The popover's usage-limits section: what each provider itself says about
 * the reader's standing against its own allowance, one subsection per
 * provider that stated one.
 *
 * Nothing here is the local spend estimate — that stays where it always was,
 * in the Usage view and, collapsed, as the chip row this section can shrink
 * to. A subsection carries only the bars `LiveUsageWindowRows` already draws
 * for the Usage view: no pace, no trend, no runway. Those are a full page's
 * worth of derived reasoning, and this is a glance on the way past.
 *
 * The whole section is silent when no provider has anything to say — live
 * usage off, or no reading has arrived yet — rather than rendering an empty
 * shell. There is nothing to collapse *to* in that case, so the reader who
 * has never turned the feature on sees nothing about it here at all.
 */
export function UsageLimitsSection({
  providers,
  live = EMPTY_LIVE_USAGE,
  now,
  expanded,
  onToggleExpanded,
  refreshing,
  onViewAll,
}: UsageLimitsSectionProps) {
  const at = now ?? (Date.parse(live.generatedAt) || 0)
  const limited = live.providers.filter((provider) => liveWindows(provider).length > 0)

  if (limited.length === 0) return null

  const toggle = (
    <button
      type="button"
      onClick={onToggleExpanded}
      aria-expanded={expanded}
      aria-label={expanded ? "Collapse usage limits" : "Expand usage limits"}
      className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-control text-label-tertiary hover:bg-surface-hover"
    >
      <ChevronDown
        size={13}
        strokeWidth={2}
        aria-hidden="true"
        className={cn(
          "transition-transform duration-[120ms] ease-out",
          expanded && "rotate-180",
        )}
      />
    </button>
  )

  const spinner = refreshing && (
    <span role="status" className="inline-flex shrink-0 items-center text-label-tertiary">
      <Loader2 size={12} strokeWidth={2} aria-hidden="true" className="animate-spin" />
      <span className="sr-only">Refreshing usage limits</span>
    </span>
  )

  return (
    <div
      data-testid="usage-limits-section"
      className="relative shrink-0 border-b border-separator px-2 pt-2 pb-2.5"
    >
      {expanded ? (
        <>
          {/* Overlaid on the corner rather than given its own row: the
              product goal is minimal vertical footprint, and a toggle plus a
              spinner is not worth a whole row of its own. The first
              subsection reserves the horizontal space this needs — see
              `reserveToggleSpace` below. */}
          <div className="absolute top-2 right-2 flex items-center gap-1">
            {spinner}
            {toggle}
          </div>
          <div className="space-y-2.5">
            {limited.map((provider, index) => (
              <ProviderLimitSubsection
                key={provider.provider}
                provider={provider}
                now={at}
                reserveToggleSpace={index === 0}
              />
            ))}
          </div>
        </>
      ) : (
        <div className="flex min-w-0 items-center gap-1">
          <ProviderUsageChips
            providers={providers}
            live={live}
            now={at}
            onViewAll={onViewAll}
            panelAnchor="down"
            className="min-w-0 flex-1"
          />
          {spinner}
          {toggle}
        </div>
      )}
    </div>
  )
}

function ProviderLimitSubsection({
  provider,
  now,
  reserveToggleSpace = false,
}: {
  provider: LiveProviderUsagePayload
  now: number
  /**
   * Leaves room on the right of the header row for the toggle (and spinner)
   * overlaid on the section's top-right corner, so a "Live Xm ago" note never
   * renders under it. Only the first subsection needs this — the toggle sits
   * over the top of the section, not beside every subsection below it.
   */
  reserveToggleSpace?: boolean
}) {
  return (
    <div>
      <div
        className={cn(
          "flex items-baseline justify-between gap-2 px-1 pb-1",
          reserveToggleSpace && "pr-8",
        )}
      >
        <h3 className="truncate type-caption font-medium text-label-secondary">
          {provider.displayName}
        </h3>
        <span
          className={cn("shrink-0 type-caption", liveFreshnessToneClass(provider.freshness))}
        >
          {liveSourceNote(provider)}
        </span>
      </div>
      <LiveUsageWindowRows provider={provider} now={now} className="px-1" />
    </div>
  )
}
