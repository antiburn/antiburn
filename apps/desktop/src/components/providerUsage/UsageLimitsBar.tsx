import { LoaderCircle, Text } from "lucide-react"
import { useId, type ReactNode } from "react"

import type { AnchoredTriggerActivation } from "../../lib/anchoredTrigger"
import { cn } from "../../lib/cn"
import { measureAnchorRegion, type AnchorRegion } from "../../lib/anchorRegion"
import type {
  LiveProviderUsagePayload,
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
} from "../../lib/ipc"
import { EMPTY_LIVE_USAGE } from "../../lib/ipc"
import type {
  LiveProviderStatus,
  UnavailableLiveProvider,
} from "../../lib/presentation/liveUsage"
import {
  liveDisplayableProviders,
  liveErrorNote,
  liveGraceNote,
  livePlanLabel,
  liveProviderStatus,
  liveResetLabel,
  liveUnavailableProviders,
  liveUnavailableReason,
  liveWindowElapsed,
  liveWindowLabel,
  liveWindows,
  orderedLiveAccounts,
  maxLiveUsedPercent,
} from "../../lib/presentation/liveUsage"
import { providerInitial } from "../../lib/presentation/providerUsage"
import { SegmentedMeter } from "../ui/SegmentedMeter"
import { SegmentFigure } from "../ui/SegmentFigure"
import { providerMark } from "./ProviderUsagePrimitives"
import { UsageRing } from "./UsageRing"
import { useStableAccountNumbers } from "./useStableAccountNumbers"

/**
 * The diameter of a provider's ring on the closed bar, in logical pixels.
 *
 * The arc is large enough to show a low share clearly. It also leaves room
 * for the percentage beside it without competing with the session rows.
 * This is local geometry for one visualization, not a shared token.
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
  /** Report provider hover for a passive companion preview. */
  onHoverProvider?: (provider: string | null, anchor: AnchorRegion | null) => void
  /** The provider trigger retained by the active companion lifecycle. */
  activeProvider?: {
    provider: string
    activation: Exclude<AnchoredTriggerActivation, "idle">
  } | null
}

/**
 * The popover's usage-limits bar: one ring and percentage per provider for
 * the worst live window, and a list-icon disclosure that replaces the rings
 * with per-provider segmented meters.
 *
 * The two states do not stack. A closed bar is the ring row. An open bar is
 * the meter list. The disclosure moves beside the first provider name.
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
  onHoverProvider,
  activeProvider,
}: UsageLimitsBarProps) {
  const limited = orderedLiveAccounts(liveDisplayableProviders(live)).filter(
    ({ reading }) => liveWindows(reading).length > 0,
  )
  const unavailable = liveUnavailableProviders(live)
  const providerCounts = new Map<string, number>()
  for (const { reading } of limited) {
    providerCounts.set(reading.provider, (providerCounts.get(reading.provider) ?? 0) + 1)
  }
  const accountNumbers = useStableAccountNumbers(
    limited.map(({ key, reading }) => ({ key, provider: reading.provider })),
  )
  const regionId = useId()
  // The instant the elapsed notches are measured from: the snapshot's own
  // time, not the wall clock. A render must not read the clock.
  const at = Date.parse(live.generatedAt) || 0

  if (limited.length === 0 && unavailable.length === 0) return null

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
    <div data-testid="usage-limits-bar" className="relative shrink-0">
      {!expanded && (
        <div className="flex min-w-0 items-center gap-2 px-3 py-2.5">
          <div className="flex min-w-0 flex-1 items-center gap-3">
            {limited.map(({ reading, key }) => (
              <ProviderRadial
                key={key}
                provider={reading}
                displayName={accountDisplayName(reading, key, accountNumbers, providerCounts)}
                status={liveProviderStatus(live, reading)}
                onOpen={onViewAll}
                onHover={onHoverProvider}
                activation={
                  activeProvider?.provider === reading.provider
                    ? activeProvider.activation
                    : null
                }
              />
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
          // The top space keeps the disclosure near its closed position.
          // The group padding gives each hover highlight clear space.
          className="space-y-1 px-2 pt-3 pb-2"
        >
          {limited.map(({ reading, key }, index) => (
            <ProviderGroup
              key={key}
              provider={reading}
              displayName={accountDisplayName(reading, key, accountNumbers, providerCounts)}
              status={liveProviderStatus(live, reading)}
              now={at}
              action={index === 0 ? disclosure(true) : undefined}
              activation={
                activeProvider?.provider === reading.provider ? activeProvider.activation : null
              }
              {...(onHoverProvider ? { onHover: onHoverProvider } : {})}
            />
          ))}
          {unavailable.map((entry, index) => (
            <UnavailableGroup
              key={entry.provider}
              entry={entry}
              action={limited.length === 0 && index === 0 ? disclosure(true) : undefined}
            />
          ))}
        </div>
      )}
    </div>
  )
}

function accountDisplayName(
  provider: LiveProviderUsagePayload,
  key: string,
  accountNumbers: ReadonlyMap<string, number>,
  counts: ReadonlyMap<string, number>,
): string {
  return (counts.get(provider.provider) ?? 0) > 1
    ? `${provider.displayName} account ${accountNumbers.get(key)}`
    : provider.displayName
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
    <span className="inline-flex items-center gap-1">
      {refreshing && (
        <span role="status" className="inline-flex shrink-0 items-center text-label-tertiary">
          <LoaderCircle size={12} strokeWidth={2} aria-hidden="true" className="animate-spin" />
          <span className="sr-only">Refreshing usage limits</span>
        </span>
      )}
      {/* Three text lines rather than a rotating chevron: the control shows
          what it reveals — the list of meter rows. */}
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        aria-pressed={expanded}
        aria-controls={expanded ? regionId : undefined}
        aria-label={expanded ? "Collapse usage limits" : "Expand usage limits"}
        // The pseudo-element extends the hit area past the glyph box. It
        // stops at the gap before the last ring, so the two do not overlap.
        className={cn(
          "relative flex h-7 w-7 shrink-0 items-center justify-center rounded-control text-label-secondary transition-colors duration-[var(--duration-fast)] before:absolute before:-inset-x-2 before:-inset-y-2.5 before:content-[''] hover:bg-surface-hover",
          compact && "-my-1.5",
        )}
      >
        <Text size={14} strokeWidth={1.75} aria-hidden="true" />
      </button>
    </span>
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
  displayName,
  status,
  now,
  action,
  onHover,
  activation,
}: {
  provider: LiveProviderUsagePayload
  displayName: string
  status: LiveProviderStatus
  now: number
  /** The disclosure, on the topmost group only. */
  action?: ReactNode
  onHover?: (provider: string | null, anchor: AnchorRegion | null) => void
  activation: Exclude<AnchoredTriggerActivation, "idle"> | null
}) {
  const plan = livePlanLabel(provider)
  const graceNote =
    status.kind === "grace"
      ? liveGraceNote(status.category, provider.provider, status.ageMs)
      : null
  return (
    <div
      role="group"
      aria-label={plan ? `${displayName}, ${plan} plan` : displayName}
      data-state={activation ?? "idle"}
      className="rounded-md px-2 py-2 transition-colors duration-[var(--duration-fast)] hover:bg-brand-tint/[0.08] data-[state=hovered]:bg-brand-tint/[0.08]"
      onMouseEnter={(event) =>
        onHover?.(provider.provider, measureAnchorRegion(event.currentTarget))
      }
      onMouseLeave={() => onHover?.(null, null)}
    >
      {/* The same type size and color as the window labels and figures
          below; the uppercase alone marks the grouping. The plan, when the
          source reports one, is a muted suffix on the same line rather than
          a second line — it is context for the name, not its own fact. */}
      <div className="flex items-center justify-between gap-2 pb-1.5">
        <h3 className="type-footnote font-medium tracking-wide text-label">
          <span className="uppercase">{displayName}</span>
          {plan && <span className="text-label-secondary"> · {plan}</span>}
        </h3>
        {action}
      </div>
      {/* Not orange: a grace-period reading is still fine, not a failure. */}
      {graceNote && <p className="pb-1.5 type-footnote text-label-tertiary">{graceNote}</p>}
      <div className="space-y-2.5">
        {liveWindows(provider).map((window) => (
          <WindowMeterRow key={window.id} window={window} now={now} resetOnHover />
        ))}
      </div>
    </div>
  )
}

/**
 * One provider's worst-window reading, as a ring and percentage. A click
 * opens the full Usage view. The review removed the separate "Show All…"
 * text button, so the radial and figure form the entry point.
 */
function ProviderRadial({
  provider,
  displayName,
  status,
  onOpen,
  onHover,
  activation,
}: {
  provider: LiveProviderUsagePayload
  displayName: string
  status: LiveProviderStatus
  onOpen?: (() => void) | undefined
  onHover?: ((provider: string | null, anchor: AnchorRegion | null) => void) | undefined
  activation: Exclude<AnchoredTriggerActivation, "idle"> | null
}) {
  const percent = maxLiveUsedPercent(provider)
  const figure = percent != null ? `${Math.round(percent)}%` : "no stated figure"
  const graceNote =
    status.kind === "grace"
      ? liveGraceNote(status.category, provider.provider, status.ageMs)
      : null
  const title = graceNote
    ? `${displayName} — ${figure} — ${graceNote}`
    : `${displayName} — ${figure}`
  const baseLabel = `${displayName}${
    percent != null ? ` at ${Math.round(percent)} percent` : ", no stated figure"
  }`
  const ariaLabel = graceNote ? `${baseLabel}. ${graceNote}` : baseLabel
  return (
    <button
      type="button"
      onClick={onOpen}
      onMouseEnter={(event) =>
        onHover?.(provider.provider, measureAnchorRegion(event.currentTarget))
      }
      onMouseLeave={() => onHover?.(null, null)}
      data-state={activation ?? "idle"}
      title={title}
      className="flex shrink-0 items-center gap-1.5 rounded-full p-1 transition-colors duration-[var(--duration-fast)] hover:bg-brand-tint/[0.08] data-[state=hovered]:bg-brand-tint/[0.08]"
      aria-label={ariaLabel}
    >
      <UsageRing
        percent={percent}
        mark={providerMark(provider.provider)}
        glyph={providerInitial(displayName)}
        size={RING_SIZE}
        className="block text-label-secondary"
      />
      <span
        aria-hidden="true"
        className={cn(
          "type-footnote leading-none",
          graceNote ? "text-label-tertiary" : "text-label",
        )}
      >
        <SegmentFigure>{percent != null ? `${Math.round(percent)}%` : "—"}</SegmentFigure>
      </span>
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
    <div
      role="group"
      aria-label={entry.displayName}
      className="flex items-center justify-between gap-2 rounded-md px-2 py-2"
    >
      <p className="type-footnote text-label-secondary">
        {liveErrorNote(entry.category, entry.provider)}
      </p>
      {action}
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
