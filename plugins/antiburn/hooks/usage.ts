export type RateLimitReading = { kind: string; percentUsed: number; resetsAt?: string }
export type UsageWindow = { kind: string; label: string; percent: number | null; resetsAt: string | null; startsOnMessage?: boolean }
export type UsageSnapshot = {
  status: 'ready' | 'empty' | 'error'
  windows: UsageWindow[]
  capturedAt: number
}

const LABELS: Readonly<Record<string, string>> = {
  five_hour: '5-hour',
  seven_day: 'Weekly',
  seven_day_opus: 'Weekly Opus',
  seven_day_sonnet: 'Weekly Sonnet',
  spend_limit: 'Spend',
}
const PERIODS: Readonly<Record<string, number>> = { five_hour: 5 * 60 * 60_000, seven_day: 7 * 24 * 60 * 60_000 }
export const PRIMARY_WINDOWS = ['five_hour', 'seven_day'] as const

export function normalizeUsage(readings: readonly RateLimitReading[], capturedAt: number): UsageSnapshot {
  const windows: UsageWindow[] = []
  for (const reading of readings) {
    const label = LABELS[reading.kind]
    if (label === undefined || windows.some((window) => window.kind === reading.kind)) continue
    windows.push({
      kind: reading.kind,
      label,
      percent: Number.isFinite(reading.percentUsed) && reading.percentUsed >= 0 ? reading.percentUsed : null,
      resetsAt: typeof reading.resetsAt === 'string' && Number.isFinite(Date.parse(reading.resetsAt)) ? reading.resetsAt : null,
    })
  }
  return { status: windows.some((window) => window.percent !== null) ? 'ready' : 'empty', windows, capturedAt }
}

export function primaryWindow(snapshot: UsageSnapshot, kind: (typeof PRIMARY_WINDOWS)[number]): UsageWindow {
  return (
    snapshot.windows.find((window) => window.kind === kind) ?? {
      kind,
      label: LABELS[kind]!,
      percent: null,
      resetsAt: null,
      startsOnMessage:
        kind === 'five_hour' &&
        snapshot.status === 'ready' &&
        snapshot.windows.some((window) => window.kind === 'seven_day' && window.percent !== null),
    }
  )
}

export function usageFailure(previous: UsageSnapshot, capturedAt: number): UsageSnapshot {
  return { status: 'error', windows: previous.windows, capturedAt }
}

export function percentage(window: UsageWindow): string {
  return window.percent === null ? '—' : `${window.percent}%`
}

export function resetText(window: UsageWindow, now: number): string {
  if (window.resetsAt === null) return window.startsOnMessage ? 'Starts when a message is sent' : 'Reset unavailable'
  const remaining = Date.parse(window.resetsAt) - now
  if (!Number.isFinite(remaining)) return 'Reset unavailable'
  if (remaining <= 0) return 'Reset due'
  const minutes = Math.floor(remaining / 60_000)
  if (minutes >= 24 * 60) return `RESETS IN ${Math.floor(minutes / (24 * 60))}D ${Math.floor(minutes / 60) % 24}H`
  return `RESETS IN ${Math.floor(minutes / 60)}H ${minutes % 60}M`
}

export function pace(window: UsageWindow, now: number): number | null {
  const period = PERIODS[window.kind]
  if (period === undefined || window.percent === null || window.resetsAt === null) return null
  const remaining = Date.parse(window.resetsAt) - now
  if (!Number.isFinite(remaining) || remaining <= 0 || remaining > period) return null
  return Math.max(0, Math.min(1, 1 - remaining / period))
}

export function paceLabel(value: number): string {
  return `Even pace: ${Math.round(value * 100)}% used by now`
}

export function reachedPace(window: UsageWindow, value: number): boolean {
  return window.percent !== null && window.percent / 100 >= value
}

export function dotFraction(percent: number | null, count: number, index: number): number {
  if (percent === null || count < 2) return 0
  return Math.max(0, Math.min(1, (Math.min(100, percent) / 100) * count - index))
}

export function filledDotCount(percent: number | null, count: number): number {
  if (percent === null || !Number.isFinite(percent) || percent <= 0 || count < 2) return 0
  if (percent >= 100) return count
  return Math.max(1, Math.min(count - 1, Math.round((percent / 100) * count)))
}

export function graphicDotCount(width: number): number {
  return Math.max(0, Math.min(20, Math.floor((width + 1) / 10)))
}

export function terminalDotCount(columns: number): number {
  return Math.max(0, Math.min(256, Math.floor((columns + 1) / 2)))
}

// A read can commit only while its session and event revision still match.
export class UsageOrder {
  private generation = 0
  private revision = 0

  beginRead(): { generation: number; revision: number } {
    this.revision += 1
    return { generation: this.generation, revision: this.revision }
  }

  acceptEvent(): { generation: number; revision: number } {
    this.revision += 1
    return { generation: this.generation, revision: this.revision }
  }

  isCurrent(ticket: { generation: number; revision: number }): boolean {
    return ticket.generation === this.generation && ticket.revision === this.revision
  }

  reset(): void {
    this.generation += 1
    this.revision += 1
  }
}
