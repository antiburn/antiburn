import type { SessionQuotaEntryPayload, SessionQuotaPayload } from "../../lib/ipc"
import { cn } from "../../lib/cn"
import { formatSpendFigure } from "../../lib/presentation/providerUsage"
import { SegmentedMeter } from "../ui/SegmentedMeter"

/** One window a session's Quota row links to, opened on the Quota screen. */
export interface SessionQuotaOpenTarget {
  provider: string
  accountKey: string
  lane: string
  rangeStart: number
  rangeEnd: number
}

const ROW_CLASS =
  "group flex w-full items-center gap-3 rounded-control px-1.5 py-1.5 text-left type-callout transition-colors duration-[var(--duration-fast)] ease-out"

/** A muted pill for a row's confidence: the same shape the list uses for a
 *  small count, so a reader recognizes it as metadata rather than a value. */
function ConfidenceTag({ children }: { children: string }) {
  return (
    <span className="shrink-0 rounded-full bg-surface-tertiary/40 px-1.5 py-0.5 type-caption text-label-tertiary">
      {children}
    </span>
  )
}

function confidenceLabel(confidence: SessionQuotaEntryPayload["confidence"]): string {
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

/** One bound window: lane, span, meter, figures, and the confidence tag. */
function BoundQuotaRow({
  entry,
  onOpenQuota,
}: {
  entry: SessionQuotaEntryPayload & {
    accountKey: string
    lane: string
    laneLabel: string
    period: NonNullable<SessionQuotaEntryPayload["period"]>
  }
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
      className={cn(ROW_CLASS, "hover:bg-surface-hover focus-visible:bg-surface-hover")}
    >
      <span className="min-w-0 flex-1">
        <span className="flex items-baseline gap-1.5">
          <span className="truncate text-label">{entry.laneLabel}</span>
          {inferred && <span className="type-caption text-label-tertiary">inferred</span>}
        </span>
        <span className="block truncate type-caption text-label-tertiary">
          {windowSpanLabel(period.startsAtEpoch, period.resetsAtEpoch)}
        </span>
      </span>
      <SegmentedMeter percent={entry.percent} segments={16} className="w-24 shrink-0" />
      <span className="w-10 shrink-0 text-right tabular-nums text-label">
        {formatPercent(entry.percent)}
      </span>
      <span className="w-16 shrink-0 text-right tabular-nums text-label-tertiary">
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

/** One provider account's rows, under its display name. */
function AccountGroup({
  displayName,
  entries,
  onOpenQuota,
}: {
  displayName: string
  entries: readonly SessionQuotaEntryPayload[]
  onOpenQuota: (target: SessionQuotaOpenTarget) => void
}) {
  return (
    <div className="flex flex-col gap-0.5">
      <h4 className="px-1.5 type-caption text-label-tertiary">{displayName}</h4>
      {entries.map((entry, index) =>
        entry.confidence === "unbound" ||
        !entry.accountKey ||
        !entry.lane ||
        !entry.laneLabel ||
        !entry.period ? (
          <UnboundQuotaRow key={index} entry={entry} />
        ) : (
          <BoundQuotaRow
            key={`${entry.lane}:${entry.period.periodId ?? entry.period.startsAtEpoch}`}
            entry={
              entry as SessionQuotaEntryPayload & {
                accountKey: string
                lane: string
                laneLabel: string
                period: NonNullable<SessionQuotaEntryPayload["period"]>
              }
            }
            onOpenQuota={onOpenQuota}
          />
        ),
      )}
    </div>
  )
}

/**
 * The Cost tab's Quota block: one row per provider account, lane, and window
 * this session's turns fell in, each linking back to that window on the
 * Quota screen.
 *
 * A failed load never renders here: `MainActivitySession` keeps whatever it
 * loaded last, and the caller simply omits this section while there is
 * nothing to show yet.
 */
export function SessionQuotaSection({
  sessionQuota,
  sessionQuotaError,
  onOpenQuota,
}: {
  sessionQuota: SessionQuotaPayload | null
  sessionQuotaError: boolean
  onOpenQuota: (target: SessionQuotaOpenTarget) => void
}) {
  if (!sessionQuota) return null
  if (sessionQuota.entries.length === 0) {
    if (sessionQuotaError) return null
    return (
      <section className="shrink-0" aria-label="Quota">
        <h3 className="type-caption font-medium tracking-wide uppercase text-label-tertiary">
          Quota
        </h3>
        <p className="mt-2 type-callout text-label-tertiary">
          No quota windows recorded for this session.
        </p>
      </section>
    )
  }

  const groups = new Map<string, { displayName: string; entries: SessionQuotaEntryPayload[] }>()
  for (const entry of sessionQuota.entries) {
    const key = groupKey(entry)
    const group = groups.get(key)
    if (group) group.entries.push(entry)
    else groups.set(key, { displayName: entry.displayName, entries: [entry] })
  }

  return (
    <section className="shrink-0" aria-label="Quota">
      <h3 className="type-caption font-medium tracking-wide uppercase text-label-tertiary">
        Quota
      </h3>
      <div className="mt-2 flex flex-col gap-3">
        {[...groups.entries()].map(([key, group]) => (
          <AccountGroup
            key={key}
            displayName={group.displayName}
            entries={group.entries}
            onOpenQuota={onOpenQuota}
          />
        ))}
      </div>
    </section>
  )
}
