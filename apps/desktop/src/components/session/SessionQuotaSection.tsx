import { ChevronRight } from "lucide-react"

import type { SessionQuotaEntryPayload, SessionQuotaPayload } from "../../lib/ipc"
import { cn } from "../../lib/cn"
import { planLabel } from "../../lib/presentation/liveUsage"
import { formatSpendFigure } from "../../lib/presentation/providerUsage"
import { SegmentedMeter } from "../ui/SegmentedMeter"
import { toggleLimitsExpanded, useLimitsExpanded } from "./limitsExpandedStore"

/** One window a session's Quota row links to, opened on the Quota screen. */
export interface SessionQuotaOpenTarget {
  provider: string
  accountKey: string
  lane: string
  rangeStart: number
  rangeEnd: number
}

const ROW_CLASS =
  "group col-span-full grid grid-cols-subgrid items-center gap-3 rounded-control px-1.5 py-1.5 text-left type-callout transition-colors duration-[var(--duration-fast)] ease-out"

/** A muted pill for a row's confidence: the same shape the list uses for a
 *  small count, so a reader recognizes it as metadata rather than a value. */
function ConfidenceTag({ children }: { children: string }) {
  return (
    <span className="shrink-0 rounded-full bg-surface-tertiary/40 px-1.5 py-0.5 text-center type-caption text-label-tertiary">
      {children}
    </span>
  )
}

function confidenceLabel(confidence: SessionQuotaEntryPayload["confidence"]): string {
  if (confidence === "measured") return "Measured"
  if (confidence === "learned") return "Learned"
  if (confidence === "seeded") return "Seeded"
  return "Unattributed"
}

/** "42%", or an em dash when the lane cannot state one. */
function formatPercent(value: number | null): string {
  return value == null ? "—" : `${Math.round(value)}%`
}

/** Two digits, zero-padded. */
function pad(value: number): string {
  return String(value).padStart(2, "0")
}

/** A wall-clock time as `4pm` or `4:05pm`, in the reader's own zone. */
function timeLabel(date: Date): string {
  const hours24 = date.getHours()
  const hour = hours24 % 12 === 0 ? 12 : hours24 % 12
  const suffix = hours24 < 12 ? "am" : "pm"
  const minutes = date.getMinutes()
  return minutes === 0 ? `${hour}${suffix}` : `${hour}:${pad(minutes)}${suffix}`
}

/** A calendar day as `Mon 14 Sep`, in the reader's own zone. */
function dayLabel(date: Date): string {
  const weekday = date.toLocaleDateString(undefined, { weekday: "short" })
  const month = date.toLocaleDateString(undefined, { month: "short" })
  return `${weekday} ${date.getDate()} ${month}`
}

/**
 * A window's span: same-day windows (a five-hour lane) collapse to one date
 * with both times, and a weekly window states both ends in full.
 */
function windowSpanLabel(startEpoch: number, resetEpoch: number): string {
  const start = new Date(startEpoch * 1000)
  const end = new Date(resetEpoch * 1000)
  const sameDay = start.toDateString() === end.toDateString()
  if (sameDay) return `${dayLabel(start)}, ${timeLabel(start)} to ${timeLabel(end)}`
  return `${dayLabel(start)} ${timeLabel(start)} to ${dayLabel(end)} ${timeLabel(end)}`
}

/** A group's own key: `provider` and `accountKey` together, since an account
 *  key can repeat across providers. An unbound entry has no account, so it
 *  gets one shared group per provider instead. */
function groupKey(entry: SessionQuotaEntryPayload): string {
  return `${entry.provider}:${entry.accountKey ?? "unbound"}`
}

/** A window entry with a resolved lane to link to. */
type BoundEntry = SessionQuotaEntryPayload & {
  accountKey: string
  lane: string
  laneLabel: string
  period: NonNullable<SessionQuotaEntryPayload["period"]>
}

/** An entry renders as a linked window only when every field the link and
 *  the row both need is present; `confidence: "unbound"` alone already
 *  implies this, but the field checks are the ground truth. */
function isBoundEntry(entry: SessionQuotaEntryPayload): entry is BoundEntry {
  return (
    entry.confidence !== "unbound" &&
    Boolean(entry.accountKey) &&
    Boolean(entry.lane) &&
    Boolean(entry.laneLabel) &&
    Boolean(entry.period)
  )
}

/**
 * Bound entries ordered newest window first: a later `startsAtEpoch` sorts
 * first, a shorter window breaks a tie at the same start (so a 5-hour
 * window sorts before a weekly one starting at the same moment), and lane
 * name is the final, stable tiebreaker. Unbound entries carry no window to
 * order by, so every one of them follows every bound entry, keeping their
 * original order.
 */
export function orderLimitEntries(
  entries: readonly SessionQuotaEntryPayload[],
): SessionQuotaEntryPayload[] {
  const bound = entries.filter(isBoundEntry)
  const unbound = entries.filter((entry) => !isBoundEntry(entry))
  const ordered = [...bound].sort((a, b) => {
    if (b.period.startsAtEpoch !== a.period.startsAtEpoch) {
      return b.period.startsAtEpoch - a.period.startsAtEpoch
    }
    const aLength = a.period.resetsAtEpoch - a.period.startsAtEpoch
    const bLength = b.period.resetsAtEpoch - b.period.startsAtEpoch
    if (aLength !== bLength) return aLength - bLength
    return a.lane.localeCompare(b.lane)
  })
  return [...ordered, ...unbound]
}

/** One bound window: lane, span, meter, figures, and the confidence tag. */
function BoundQuotaRow({
  entry,
  isCurrent,
  onOpenQuota,
}: {
  entry: BoundEntry
  isCurrent: boolean
  onOpenQuota: (target: SessionQuotaOpenTarget) => void
}) {
  const { period } = entry
  const inferred = period.startSource !== "reported" || period.resetSource !== "reported"
  return (
    <button
      type="button"
      onClick={() =>
        onOpenQuota({
          provider: entry.provider,
          accountKey: entry.accountKey,
          lane: entry.lane,
          rangeStart: period.startsAtEpoch,
          rangeEnd: period.resetsAtEpoch,
        })
      }
      aria-current={isCurrent ? "true" : undefined}
      className={cn(
        ROW_CLASS,
        "hover:bg-surface-hover focus-visible:bg-surface-hover",
        isCurrent && "bg-surface-selected/40",
      )}
    >
      <span className="min-w-0 flex-1">
        <span className="flex items-baseline gap-1.5">
          <span className="truncate text-label">{entry.laneLabel}</span>
          {isCurrent && <span className="type-caption text-label-secondary">current</span>}
          {inferred && <span className="type-caption text-label-tertiary">inferred</span>}
        </span>
        <span className="block truncate type-caption text-label-tertiary">
          {windowSpanLabel(period.startsAtEpoch, period.resetsAtEpoch)}
        </span>
      </span>
      <SegmentedMeter percent={entry.percent} segments={16} className="w-36 shrink-0" />
      <span className="w-12 shrink-0 text-center tabular-nums text-label">
        {formatPercent(entry.percent)}
      </span>
      <span className="w-16 shrink-0 text-center tabular-nums text-label-tertiary">
        {formatSpendFigure(entry.usd)}
      </span>
      <ConfidenceTag>{confidenceLabel(entry.confidence)}</ConfidenceTag>
    </button>
  )
}

/** An unbound window: no lane to link to, so it renders as a plain row. */
function UnboundQuotaRow({ entry }: { entry: SessionQuotaEntryPayload }) {
  return (
    <div className={ROW_CLASS}>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-label-secondary">Not linked to an account</span>
      </span>
      <span className="w-16 shrink-0 text-right tabular-nums text-label-tertiary">
        {formatSpendFigure(entry.usd)}
      </span>
      <ConfidenceTag>Unattributed</ConfidenceTag>
    </div>
  )
}

/** One provider account's rows, under its display name and, when the
 *  account's newest observation named one, its plan. */
function AccountGroup({
  displayName,
  entries,
  nowEpoch,
  onOpenQuota,
}: {
  displayName: string
  entries: readonly SessionQuotaEntryPayload[]
  nowEpoch: number | null
  onOpenQuota: (target: SessionQuotaOpenTarget) => void
}) {
  const firstBound = entries.find(isBoundEntry)
  const plan = firstBound ? planLabel(firstBound.provider, firstBound.plan) : null
  const ordered = orderLimitEntries(entries)
  return (
    <>
      <h4 className="col-span-full mt-1.5 px-1.5 type-caption text-label-tertiary">
        {displayName}
        {plan && (
          <>
            <span aria-hidden="true"> · </span>
            {plan}
          </>
        )}
      </h4>

      {ordered.map((entry) =>
        isBoundEntry(entry) ? (
          <BoundQuotaRow
            key={`${entry.lane}:${entry.period.periodId ?? entry.period.startsAtEpoch}`}
            entry={entry}
            isCurrent={nowEpoch != null && entry.period.resetsAtEpoch > nowEpoch}
            onOpenQuota={onOpenQuota}
          />
        ) : (
          // One unbound entry per provider by construction: see the ordered
          // build above.
          <UnboundQuotaRow key={`unbound:${entry.provider}`} entry={entry} />
        ),
      )}
    </>
  )
}

/** "contributed to 7 separate windows", or "contributed to 1 window". */
function windowCountLabel(count: number): string {
  return count === 1 ? "contributed to 1 window" : `contributed to ${count} separate windows`
}

/** The session payload's own stamp, read as a Unix second epoch, or `null`
 *  when it does not parse: render stays pure and a bad stamp marks nothing
 *  as current rather than guessing "now". */
function generatedAtEpoch(generatedAt: string): number | null {
  const parsedMs = Date.parse(generatedAt)
  return Number.isNaN(parsedMs) ? null : Math.floor(parsedMs / 1000)
}

/**
 * The Cost tab's collapsible Limits card: one row per provider account,
 * lane, and window this session's turns fell in, each linking back to that
 * window on the Limits screen.
 *
 * A failed load never renders here: `MainActivitySession` keeps whatever it
 * loaded last, and the caller simply omits this section while there is
 * nothing to show yet. An empty result renders nothing too, for the same
 * reason the Unused-context card omits itself when it has nothing to show:
 * an empty collapsible card is noise.
 */
export function SessionQuotaSection({
  sessionQuota,
  onOpenQuota,
}: {
  sessionQuota: SessionQuotaPayload | null
  onOpenQuota: (target: SessionQuotaOpenTarget) => void
}) {
  const expanded = useLimitsExpanded()
  if (!sessionQuota || sessionQuota.entries.length === 0) return null

  const nowEpoch = generatedAtEpoch(sessionQuota.generatedAt)

  const groups = new Map<string, { displayName: string; entries: SessionQuotaEntryPayload[] }>()
  for (const entry of sessionQuota.entries) {
    const key = groupKey(entry)
    const group = groups.get(key)
    if (group) group.entries.push(entry)
    else groups.set(key, { displayName: entry.displayName, entries: [entry] })
  }

  return (
    <section
      aria-label="Limits"
      className="grid w-full min-w-0 gap-y-1 rounded-control bg-surface-card/50 px-3 py-2"
    >
      <button
        type="button"
        onClick={toggleLimitsExpanded}
        aria-expanded={expanded}
        className="flex w-full items-center justify-between gap-x-3 rounded-control px-1 py-1 text-left type-body cursor-pointer! transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover active:transform-none active:opacity-100"
      >
        <span className="flex min-w-0 items-center gap-x-1.5">
          <ChevronRight
            size={14}
            aria-hidden="true"
            className={cn(
              "shrink-0 transition-transform duration-[var(--duration-fast)] ease-out",
              expanded && "rotate-90",
            )}
          />
          <span className="truncate text-label">Limits</span>
        </span>
        <span className="flex shrink-0 items-baseline gap-1 text-right tabular-nums">
          <span className="text-label">{windowCountLabel(sessionQuota.entries.length)}</span>
        </span>
      </button>

      {expanded && (
        <div className="mt-1 grid grid-cols-[1fr_auto_auto_auto_auto] gap-1.5">
          {[...groups.entries()].map(([key, group]) => (
            <AccountGroup
              key={key}
              displayName={group.displayName}
              entries={group.entries}
              nowEpoch={nowEpoch}
              onOpenQuota={onOpenQuota}
            />
          ))}
        </div>
      )}
    </section>
  )
}
