import type { UsageSnapshot } from './usage'

export type AccountUsageResult =
  | { status: 'ready'; snapshot: UsageSnapshot }
  | { status: 'unsupported' }
  | { status: 'error'; retryAfter?: string }

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
