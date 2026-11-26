import { normalizeUsage, type RateLimitReading, type UsageSnapshot } from './usage'

export type AccountUsageResult =
  | { status: 'ready'; snapshot: UsageSnapshot }
  | { status: 'unsupported' }
  | { status: 'error'; retryAfter?: string }

const FLAT_KINDS = ['five_hour', 'seven_day', 'seven_day_opus', 'seven_day_sonnet'] as const
const LIMIT_KINDS: Readonly<Record<string, string>> = { session: 'five_hour', weekly_all: 'seven_day' }
const SCOPED_KINDS: Readonly<Record<string, string>> = { opus: 'seven_day_opus', sonnet: 'seven_day_sonnet' }

const record = (value: unknown): Record<string, unknown> | undefined =>
  value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined

function resetTime(value: unknown): string | undefined {
  if (typeof value === 'string') return value
  if (typeof value !== 'number' || !Number.isFinite(value)) return undefined
  const date = new Date(value * 1000)
  return Number.isFinite(date.getTime()) ? date.toISOString() : undefined
}

function limitKind(limit: Record<string, unknown>): string | undefined {
  const name = typeof limit.kind === 'string' ? limit.kind : limit.name
  if (typeof name !== 'string') return undefined
  if (name !== 'weekly_scoped') return LIMIT_KINDS[name]
  const model = record(record(record(limit.scope)?.model))?.display_name
  return typeof model === 'string' ? SCOPED_KINDS[model.trim().toLowerCase()] : undefined
}

function limitPercent(limit: Record<string, unknown>): number | undefined {
  if (typeof limit.percent === 'number') return limit.percent
  if (typeof limit.utilization !== 'number') return undefined
  // A utilization value in the limit list can be a fraction or a percentage.
  return limit.utilization >= 0 && limit.utilization <= 1 ? limit.utilization * 100 : limit.utilization
}

// Read the limit list when it has entries. Use the flat keys only when it does not.
// A limit list without a session entry shows that the account has no 5-hour limit.
export function accountSnapshot(payload: Record<string, unknown>, capturedAt: number): UsageSnapshot {
  const limits = Array.isArray(payload.limits)
    ? payload.limits.map(record).filter((limit) => limit !== undefined)
    : []
  const readings: RateLimitReading[] = []
  if (limits.length > 0) {
    for (const limit of limits) {
      const kind = limitKind(limit)
      const percentUsed = limitPercent(limit)
      if (kind === undefined || percentUsed === undefined) continue
      readings.push({ kind, percentUsed, resetsAt: resetTime(limit.resets_at) })
    }
    const fiveHourAbsent = !limits.some((limit) => (limit.kind ?? limit.name) === 'session')
    return normalizeUsage(readings, capturedAt, fiveHourAbsent)
  }
  for (const kind of FLAT_KINDS) {
    const window = record(payload[kind])
    if (!window || typeof window.utilization !== 'number') continue
    readings.push({
      kind,
      percentUsed: window.utilization,
      resetsAt: typeof window.resets_at === 'string' ? window.resets_at : undefined,
    })
  }
  return normalizeUsage(readings, capturedAt)
}

export function retryAfterTime(value: string | undefined, now: number): number {
  if (value === undefined) return 0
  const trimmed = value.trim()
  if (/^\d+(?:\.\d+)?$/.test(trimmed)) {
    const delay = Number(trimmed) * 1000
    return Number.isFinite(now + delay) ? now + delay : 0
  }
  const date = Date.parse(trimmed)
  return Number.isFinite(date) && date > now ? date : 0
}
