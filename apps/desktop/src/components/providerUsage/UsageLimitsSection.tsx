// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronDown, Loader2 } from "lucide-react"
import { useId } from "react"

import { cn } from "../../lib/cn"
import type {
  LiveProviderUsagePayload,
  LiveUsageSummaryPayload,
  ProviderUsagePayload,
} from "../../lib/ipc"
import { EMPTY_LIVE_USAGE } from "../../lib/ipc"
import type { UnavailableLiveProvider } from "../../lib/presentation/liveUsage"
import {
  liveErrorNote,
  liveSourceNote,
  liveUnavailableProviders,
  liveUnavailableReason,
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
 * has never turned the feature on sees nothing about it here at all. A
 * provider whose source *failed* is different: it keeps a subsection that
 * names the failure, because a section that silently vanishes reads as data
 * loss.
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
  // Providers whose only trace is a source error — a failed fetch with no
  // cached reading. They keep a degraded subsection rather than vanishing.
  const unavailable = liveUnavailableProviders(live)
  const headingId = useId()

  if (limited.length === 0 && unavailable.length === 0) return null

  const toggle = (
    <button
      type="button"
      onClick={onToggleExpanded}
      aria-expanded={expanded}
      aria-label={expanded ? "Collapse usage limits" : "Expand usage limits"}
      className="flex items-center justify-center h-6 w-6 rounded-md text-label-tertiary hover:bg-surface-hover"
    >
      <ChevronDown
        size={13}
        strokeWidth={2}
        aria-hidden="true"
        className={cn(
          "transition-transform duration-[var(--duration-fast)] ease-out",
          expanded && "rotate-270",
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
      className="relative shrink-0 border-b border-separator"
      {...(expanded ? { role: "region", "aria-labelledby": headingId } : {})}
    >
      {expanded ? (
        <>
          <div className="absolute top-2 right-2 flex items-center gap-1">{spinner}</div>

          <div className="flex">
            <div className="flex flex-col w-7 gap-y-2 py-2 shrink-0 items-center bg-surface-secondary">
              {toggle}

              <h2
                id={headingId}
                className="[writing-mode:vertical-rl] [text-orientation:upright] type-caption font-medium tracking-wider text-label-tertiary uppercase"
              >
                Limits
              </h2>
            </div>

            <div className="min-w-0 flex-1 space-y-2.5 py-3 px-2">
              {limited.map((provider) => (
                <ProviderLimitSubsection key={provider.provider} provider={provider} now={at} />
              ))}
              {unavailable.map((entry) => (
                <UnavailableLimitSubsection key={entry.provider} entry={entry} />
              ))}
            </div>
          </div>
        </>
      ) : (
        <div className="flex min-w-0 items-center gap-1 px-1 py-1">
          {toggle}

          <ProviderUsageChips
            providers={providers}
            live={live}
            now={at}
            onViewAll={onViewAll}
            panelAnchor="down"
            className="min-w-0 flex-1"
          />

          {spinner}
        </div>
      )}
    </div>
  )
}

/**
 * One provider's subsection while its live reading is unavailable — a failed
 * source with nothing cached to fall back on. The provider keeps its place
 * with the failure named, instead of vanishing without a word.
 */
function UnavailableLimitSubsection({ entry }: { entry: UnavailableLiveProvider }) {
  return (
    <div data-testid="usage-limits-unavailable">
      <div className="flex items-baseline justify-between gap-2 px-1 pb-1">
        <h3 className="truncate text-label-secondary">{entry.displayName}</h3>
        <span className="shrink-0 type-caption text-label-tertiary">
          {liveUnavailableReason(entry.category)}
        </span>
      </div>
      <p className="px-1 type-caption text-label-tertiary">{liveErrorNote(entry.category)}</p>
    </div>
  )
}

function ProviderLimitSubsection({
  provider,
  now,
}: {
  provider: LiveProviderUsagePayload
  now: number
}) {
  return (
    <div>
      <div className="group flex items-baseline justify-between gap-2 px-1 pb-1">
        <h3 className="truncate text-label-secondary">{provider.displayName}</h3>
        <span className="type-caption hidden shrink-0 text-label-tertiary group-hover:inline">
          {liveSourceNote(provider)}
        </span>
      </div>
      <LiveUsageWindowRows provider={provider} now={now} className="px-1" />
    </div>
  )
}
