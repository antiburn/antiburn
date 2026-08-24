// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { BarChartHorizontalBig, Loader2 } from "lucide-react"
import { useId, type ReactNode } from "react"

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

/**
 * The diameter of a provider's ring on the closed bar, in logical pixels.
 *
 * Larger than the 22 it was inside a pill. The pill and the percentage beside
 * it are gone, so the arc is the whole reading, and an arc has to be big
 * enough to tell a low share from an empty one. It stops short of a size that
 * competes with the session rows under it. Local geometry for this one
 * visualization, not a shared token.
 */
const RING_SIZE = 26

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
 * The popover's usage-limits bar: one ring per provider carrying the worst
 * live window, and a chart-icon disclosure that replaces the row with
 * per-provider segmented meters.
 *
 * The two states do not stack. A closed bar is the ring row. An open one is
 * the meters alone, with the disclosure moved onto the first provider's name:
 * the meters restate every ring above them, and the popover needs the row's
 * height for the activity list more than it needs the same reading twice.
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
  const limited = live.providers.filter((provider) => liveWindows(provider).length > 0)
  const unavailable = liveUnavailableProviders(live)
  const regionId = useId()
  // The instant the elapsed notches are measured from: the snapshot's own
  // time, not the wall clock. A render must not read the clock.
  const at = Date.parse(live.generatedAt) || 0

  if (limited.length === 0 && unavailable.length === 0) return null

  // The provider whose name line carries the disclosure while the meters are
  // open — the topmost group, whichever list it came from.
  const firstGroup = limited[0]?.provider ?? unavailable[0]?.provider

  const disclosure = (compact: boolean) => (
    <LimitsDisclosure
      expanded={expanded}
      onToggle={onToggleExpanded}
      regionId={regionId}
      refreshing={refreshing}
      compact={compact}
    />
  )

  return (
    <div data-testid="usage-limits-bar" className="relative shrink-0 border-b border-separator">
      {!expanded && (
        <div className="flex min-w-0 items-center gap-2 px-3 py-2.5">
          <div className="flex min-w-0 flex-1 items-center gap-2">
            {limited.map((provider) => (
              <ProviderRadial key={provider.provider} provider={provider} onOpen={onViewAll} />
            ))}
            {unavailable.map((entry) => (
              <UnavailableRadial key={entry.provider} entry={entry} />
            ))}
          </div>
          {disclosure(false)}
        </div>
      )}

      {expanded && (
        <div
          id={regionId}
          role="region"
          aria-label="Usage limits"
          // Roomier than the collapsed bar: a beat of air above, wider
          // gutters, and clear space between providers and between their
          // meters. The gutter splits between this region and each group, so
          // a group's hover highlight has room without moving the text.
          //
          // The top padding also holds the disclosure still. The button moves
          // from this bar's own row onto the first provider's name line, and
          // the two must sit at the same height, or the control jumps under
          // the pointer that just clicked it.
          className="space-y-1 px-2 pt-3 pb-2"
        >
          {limited.map((provider) => (
            <ProviderGroup
              key={provider.provider}
              provider={provider}
              now={at}
              action={provider.provider === firstGroup ? disclosure(true) : undefined}
            />
          ))}
          {unavailable.map((entry) => (
            <UnavailableGroup
              key={entry.provider}
              entry={entry}
              action={entry.provider === firstGroup ? disclosure(true) : undefined}
            />
          ))}
        </div>
      )}
    </div>
  )
}

/**
 * The chart-icon disclosure, and the refresh spinner that sits beside it.
 *
 * The same size, weight, and grey as the settings gear in the popover footer
 * (`PopoverView.tsx`), and the same in both states. The control marks itself
 * pressed for assistive technology, but the meters under it are the sighted
 * answer to "is it open", and an orange glyph competed with the orange arcs
 * and segments it sits among.
 *
 * `compact` is the open placement: the button shares a line with a provider's
 * name, so it pulls its own padding back out of the line box and leaves the
 * line the height the name alone would give it.
 */
function LimitsDisclosure({
  expanded,
  onToggle,
  regionId,
  refreshing,
  compact,
}: {
  expanded: boolean
  onToggle: () => void
  regionId: string
  refreshing: boolean
  compact: boolean
}) {
  return (
    <>
      {refreshing && (
        <span role="status" className="inline-flex shrink-0 items-center text-label-tertiary">
          <Loader2 size={12} strokeWidth={2} aria-hidden="true" className="animate-spin" />
          <span className="sr-only">Refreshing usage limits</span>
        </span>
      )}
      {/* A chart icon rather than a rotating chevron: the control shows what
          it reveals — the per-window meters. */}
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        aria-pressed={expanded}
        aria-controls={expanded ? regionId : undefined}
        aria-label={expanded ? "Collapse usage limits" : "Expand usage limits"}
        className={cn(
          "flex h-7 w-7 shrink-0 items-center justify-center rounded-control text-label-secondary transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover",
          compact && "-my-1.5",
        )}
      >
        <BarChartHorizontalBig size={14} strokeWidth={1.75} aria-hidden="true" />
      </button>
    </>
  )
}

/**
 * One provider's expanded meter rows, under a small eyebrow label naming the
 * provider. An earlier review removed the heading and the next one put the
 * name back, so that two providers with the same window label stay tellable
 * apart.
 */
function ProviderGroup({
  provider,
  now,
  action,
}: {
  provider: LiveProviderUsagePayload
  now: number
  /** The disclosure, on the topmost group only. */
  action?: ReactNode
}) {
  return (
    <div
      role="group"
      aria-label={provider.displayName}
      className="rounded-md px-2 py-2 transition-colors duration-[var(--duration-fast)] hover:bg-brand-tint/[0.08]"
    >
      {/* The same type size and color as the window labels and figures
          below; the uppercase alone marks the grouping. */}
      <div className="flex items-center justify-between gap-2 pb-1.5">
        <h3 className="type-footnote font-medium tracking-wide uppercase text-label">
          {provider.displayName}
        </h3>
        {action}
      </div>
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
 * One provider's worst-window reading, as a ring and nothing else. A click
 * opens the full Usage view — the review removed the separate "Show All…"
 * text button, so the radials themselves are the entry point.
 *
 * The pill and the percentage beside it are both gone. The figure they stated
 * is restated by every meter one click away, and the row's job here is a
 * glance, not a readout. The words survive on hover and for assistive
 * technology, so nothing that was said is now unsayable.
 */
function ProviderRadial({
  provider,
  onOpen,
}: {
  provider: LiveProviderUsagePayload
  onOpen?: (() => void) | undefined
}) {
  const percent = maxLiveUsedPercent(provider)
  const figure = percent != null ? `${Math.round(percent)}%` : "no stated figure"
  return (
    <button
      type="button"
      onClick={onOpen}
      title={`${provider.displayName} — ${figure}`}
      className="shrink-0 rounded-full p-1 transition-colors duration-[var(--duration-fast)] hover:bg-brand-tint/[0.08]"
      aria-label={`${provider.displayName}${
        percent != null ? ` at ${Math.round(percent)} percent` : ", no stated figure"
      }`}
    >
      <UsageRing
        percent={percent}
        mark={providerMark(provider.provider)}
        glyph={providerInitial(provider.displayName)}
        size={RING_SIZE}
        className="block text-label-secondary"
      />
    </button>
  )
}

/**
 * One provider's seat on the bar while its live reading is unavailable — a
 * failed source with nothing cached to fall back on.
 *
 * The same silhouette as `ProviderRadial`, so the provider visibly keeps its
 * place, drawn with the indeterminate dashed ring. It is not a button: a
 * click would open a surface with nothing on it.
 *
 * The failure is named on hover and to assistive technology, and in full in
 * the open meters. A dashed ring on its own says "no reading", which is the
 * part a glance needs.
 */
function UnavailableRadial({ entry }: { entry: UnavailableLiveProvider }) {
  const reason = liveUnavailableReason(entry.category)
  return (
    <div
      data-testid="usage-limits-unavailable"
      className="shrink-0 p-1"
      title={`${entry.displayName} — ${reason}`}
      aria-label={`${entry.displayName}, usage unavailable (${reason})`}
    >
      <UsageRing
        percent={null}
        mark={providerMark(entry.provider)}
        glyph={providerInitial(entry.displayName)}
        size={RING_SIZE}
        className="block text-label-tertiary"
      />
    </div>
  )
}

/**
 * One provider's expanded rows while its live reading is unavailable: the
 * same eyebrow as `ProviderGroup`, then the failure and what it means in
 * place of the meters.
 */
function UnavailableGroup({
  entry,
  action,
}: {
  entry: UnavailableLiveProvider
  /** The disclosure, when no provider above this one can carry it. */
  action?: ReactNode
}) {
  return (
    <div role="group" aria-label={entry.displayName} className="rounded-md px-2 py-2">
      <div className="flex items-center justify-between gap-2 pb-1.5">
        <h3 className="type-footnote font-medium tracking-wide uppercase text-label">
          {entry.displayName}
        </h3>
        {action}
      </div>
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
function WindowMeterRow({
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
                "type-footnote text-label-tertiary transition-opacity duration-[var(--duration-fast)]",
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
