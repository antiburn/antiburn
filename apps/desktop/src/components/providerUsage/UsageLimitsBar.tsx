// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { BarChartHorizontalBig, Loader2 } from "lucide-react"
import { useId } from "react"

import { cn } from "../../lib/cn"
import type {
  LiveProviderUsagePayload,
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
} from "../../lib/ipc"
import { EMPTY_LIVE_USAGE } from "../../lib/ipc"
import type { UnavailableLiveProvider } from "../../lib/presentation/liveUsage"
import {
  liveErrorNote,
  liveResetLabel,
  liveUnavailableProviders,
  liveUnavailableReason,
  liveWindowElapsed,
  liveWindowLabel,
  liveWindows,
  maxLiveUsedPercent,
} from "../../lib/presentation/liveUsage"
import { providerInitial } from "../../lib/presentation/providerUsage"
import { SegmentedMeter } from "../ui/SegmentedMeter"
import { SegmentFigure } from "../ui/SegmentFigure"
import { providerMark } from "./ProviderUsagePrimitives"
import { UsageRing } from "./UsageRing"

export interface UsageLimitsBarProps {
  /** The provider's own limit figures, when a source could prove any. */
  live?: LiveUsageSummaryPayload
  /** Whether the bar shows its per-provider meters below the pill row. */
  expanded: boolean
  onToggleExpanded: () => void
  /** Whether a refresh is in flight, for the small spinner beside the toggle. */
  refreshing: boolean
  /** Open the full Usage view, from a provider pill. */
  onViewAll: () => void
}

/**
 * The popover's usage-limits bar: one pill per provider carrying the worst
 * live window as a mini radial and a percentage, and a chart-icon disclosure
 * that drops per-provider segmented meters below the row.
 *
 * Nothing here is the local spend estimate. This bar reports only what a
 * provider itself says about the reader's standing against its own allowance.
 *
 * The bar is silent when no provider has anything to say — live usage off, or
 * no reading has arrived yet — rather than rendering an empty shell. A
 * provider whose source *failed* is different: it keeps a degraded seat that
 * names the failure, because a provider that silently vanishes reads as data
 * loss and not as a passing failure.
 */
export function UsageLimitsBar({
  live = EMPTY_LIVE_USAGE,
  expanded,
  onToggleExpanded,
  refreshing,
  onViewAll,
}: UsageLimitsBarProps) {
  // The same predicate the chips use, so every limits surface agrees on which
  // providers are showing anything.
  const limited = live.providers.filter((provider) => liveWindows(provider).length > 0)
  const unavailable = liveUnavailableProviders(live)
  const regionId = useId()
  // The instant the elapsed notches are measured from: the snapshot's own
  // time, not the wall clock. A render must not read the clock.
  const at = Date.parse(live.generatedAt) || 0

  if (limited.length === 0 && unavailable.length === 0) return null

  return (
    <div data-testid="usage-limits-bar" className="relative shrink-0 border-b border-separator">
      <div className="flex min-w-0 items-center gap-2 px-3 py-3">
        <div className="flex min-w-0 flex-1 items-center gap-3.5">
          {limited.map((provider) => (
            <ProviderRadial key={provider.provider} provider={provider} onOpen={onViewAll} />
          ))}
          {unavailable.map((entry) => (
            <UnavailableRadial key={entry.provider} entry={entry} />
          ))}
        </div>

        {refreshing && (
          <span role="status" className="inline-flex shrink-0 items-center text-label-tertiary">
            <Loader2 size={12} strokeWidth={2} aria-hidden="true" className="animate-spin" />
            <span className="sr-only">Refreshing usage limits</span>
          </span>
        )}

        {/* A chart icon with a pressed state rather than a rotating chevron:
            the control shows what it reveals — the per-window meters — and
            its fill shows whether they are open. */}
        <button
          type="button"
          onClick={onToggleExpanded}
          aria-expanded={expanded}
          aria-pressed={expanded}
          aria-controls={expanded ? regionId : undefined}
          aria-label={expanded ? "Collapse usage limits" : "Expand usage limits"}
          // The same height as the provider pills on the row. The open state
          // is the orange glyph alone, with no filled pill behind it.
          className={cn(
            "flex h-8 w-8 shrink-0 items-center justify-center rounded-md transition-colors duration-[120ms] hover:bg-brand-tint/[0.08]",
            expanded ? "text-brand" : "text-label-tertiary",
          )}
        >
          <BarChartHorizontalBig size={15} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>

      {expanded && (
        <div
          id={regionId}
          role="region"
          aria-label="Usage limits"
          // Roomier than the collapsed bar: a beat of air above, wider
          // gutters, and clear space between providers and between their
          // meters. The gutter splits between this region and each group, so
          // a group's hover highlight has room without moving the text.
          className="space-y-1 px-2 pb-2"
        >
          {limited.map((provider) => (
            <ProviderGroup key={provider.provider} provider={provider} now={at} />
          ))}
          {unavailable.map((entry) => (
            <UnavailableGroup key={entry.provider} entry={entry} />
          ))}
        </div>
      )}
    </div>
  )
}

/**
 * One provider's expanded meter rows, under a small eyebrow label naming the
 * provider. An earlier review removed the heading and the next one put the
 * name back, so that two providers with the same window label stay tellable
 * apart.
 */
function ProviderGroup({ provider, now }: { provider: LiveProviderUsagePayload; now: number }) {
  return (
    <div
      role="group"
      aria-label={provider.displayName}
      className="rounded-md px-2 py-2 transition-colors duration-[120ms] hover:bg-brand-tint/[0.08]"
    >
      {/* The same type size and color as the window labels and figures
          below; the uppercase alone marks the grouping. */}
      <h3 className="pb-1.5 type-footnote font-medium tracking-wide uppercase text-label">
        {provider.displayName}
      </h3>
      <div className="space-y-2.5">
        {liveWindows(provider).map((window) => (
          <WindowMeterRow
            key={liveWindowLabel(window)}
            window={window}
            now={now}
            resetOnHover
          />
        ))}
      </div>
    </div>
  )
}

/**
 * One provider's mini radial and worst-window percentage, dressed as a pill
 * button so that it reads as interactive. A click opens the full Usage view —
 * the review removed the separate "Show All…" text button, so the radials
 * themselves are the entry point.
 */
function ProviderRadial({
  provider,
  onOpen,
}: {
  provider: LiveProviderUsagePayload
  onOpen?: (() => void) | undefined
}) {
  const percent = maxLiveUsedPercent(provider)
  return (
    <button
      type="button"
      onClick={onOpen}
      className="flex shrink-0 items-center gap-1.5 rounded-full border border-separator px-2.5 py-1 transition-colors duration-[120ms] hover:bg-brand-tint/[0.08]"
      aria-label={`${provider.displayName}${
        percent != null ? ` at ${Math.round(percent)} percent` : ", no stated figure"
      }`}
    >
      <UsageRing
        percent={percent}
        mark={providerMark(provider.provider)}
        glyph={providerInitial(provider.displayName)}
        size={22}
        className="shrink-0 text-label-secondary"
      />
      <span aria-hidden="true" className="type-footnote leading-none text-label">
        <SegmentFigure>{percent != null ? `${Math.round(percent)}%` : "—"}</SegmentFigure>
      </span>
    </button>
  )
}

/**
 * One provider's seat on the bar while its live reading is unavailable — a
 * failed source with nothing cached to fall back on.
 *
 * The same pill silhouette as `ProviderRadial`, so the provider visibly keeps
 * its place, with the indeterminate dashed ring and the failure in two words.
 * It is not a button: a click would open a surface with nothing on it.
 */
function UnavailableRadial({ entry }: { entry: UnavailableLiveProvider }) {
  return (
    <div
      data-testid="usage-limits-unavailable"
      className="flex shrink-0 items-center gap-1.5 rounded-full border border-separator px-2.5 py-1"
      aria-label={`${entry.displayName}, usage unavailable (${liveUnavailableReason(entry.category)})`}
    >
      <UsageRing
        percent={null}
        mark={providerMark(entry.provider)}
        glyph={providerInitial(entry.displayName)}
        size={22}
        className="shrink-0 text-label-tertiary"
      />
      <span aria-hidden="true" className="type-footnote leading-none text-label-tertiary">
        {liveUnavailableReason(entry.category)}
      </span>
    </div>
  )
}

/**
 * One provider's expanded rows while its live reading is unavailable: the
 * same eyebrow as `ProviderGroup`, then the failure and what it means in
 * place of the meters.
 */
function UnavailableGroup({ entry }: { entry: UnavailableLiveProvider }) {
  return (
    <div role="group" aria-label={entry.displayName} className="rounded-md px-2 py-2">
      <h3 className="pb-1.5 type-footnote font-medium tracking-wide uppercase text-label">
        {entry.displayName}
      </h3>
      <p className="type-footnote text-label-secondary">
        Usage unavailable — {liveUnavailableReason(entry.category)}.
      </p>
      <p className="pt-0.5 type-footnote text-label-tertiary">
        {liveErrorNote(entry.category)}
      </p>
    </div>
  )
}

/**
 * One limit window: label, segmented orange meter with the linear-use notch,
 * figure.
 */
export function WindowMeterRow({
  window,
  now,
  resetOnHover = false,
}: {
  window: LiveUsageWindowPayload
  /** The instant the elapsed notch is measured from. */
  now: number
  /**
   * Show the reset time only while the pointer is on this row.
   *
   * The popover stacks a row for every window of every provider, and a reset
   * time on each one competes with the figure that matters: the percentage.
   * A surface that shows one provider keeps the reset in view instead.
   */
  resetOnHover?: boolean
}) {
  const percent = window.usedPercent
  return (
    <div className="group/meter">
      <div className="flex items-baseline justify-between gap-2 pb-0.5">
        {/* The same size and color as the provider name and the figure: one
            type treatment across the expanded rows. */}
        <span className="truncate type-footnote text-label">{liveWindowLabel(window)}</span>
        <span className="flex shrink-0 items-baseline gap-2">
          {/* The reset as a wall-clock time, and only when the provider
              stated one — there is no seat for "reset unavailable" here. The
              hidden state fades and does not unmount, so the row keeps the
              space and the figure does not move on hover. */}
          {window.resetsAt && (
            <span
              className={cn(
                "type-footnote text-label-tertiary transition-opacity duration-[120ms]",
                resetOnHover && "opacity-0 group-hover/meter:opacity-100",
              )}
            >
              {liveResetLabel(window, now)}
            </span>
          )}
          <span className="type-footnote text-label">
            <SegmentFigure>{percent != null ? `${Math.round(percent)}%` : "—"}</SegmentFigure>
          </span>
        </span>
      </div>
      <SegmentedMeter
        percent={percent ?? null}
        expectedFraction={liveWindowElapsed(window, now)}
      />
    </div>
  )
}
