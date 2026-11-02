import { normalizeUsage, type UsageSnapshot } from './usage'

export type PreviewWarning = 'red' | 'pulse'
export type UsagePreviewState = 'idle' | 'checking' | 'retry' | 'success' | 'stale'
const USAGE_STATES: readonly UsagePreviewState[] = ['idle', 'checking', 'retry', 'success', 'stale']
export const PREVIEW_PACE = [55, 50] as const
export const USAGE_FIXTURES = [
  { id: 'inactive', label: 'Inactive session', values: [null, 1] },
  { id: 'low', label: 'Low usage · 8% / 1%', values: [8, 1] },
  { id: 'mid', label: 'Mid usage · 48% / 36%', values: [48, 36] },
  { id: 'beyond-pace', label: 'Beyond pace · 68% / 58% · red', values: [68, 58], warning: 'red' },
  { id: 'high', label: 'High usage · 88% / 82% · red', values: [88, 82], warning: 'red' },
  { id: 'near-limit', label: 'Near limit · 98% / 96% · pulse + glow', values: [98, 96], warning: 'pulse' },
  { id: 'nine', label: 'Numeric roll · 9%', values: [9, 9] },
  { id: 'ten', label: 'Numeric roll · 10%', values: [10, 10] },
  { id: 'ninety-nine', label: 'Numeric roll · 99%', values: [99, 99] },
  { id: 'hundred', label: 'Numeric roll · 100%', values: [100, 100] },
  { id: 'empty', label: 'No reading yet', values: [] },
  { id: 'error', label: 'Read error · last known 48% / 36%', values: [48, 36] },
] as const

export function usageFixture(
  id: string,
  now: number,
): { snapshot: UsageSnapshot; warning?: PreviewWarning; state?: UsagePreviewState; retryAt?: number } | undefined {
  const state = USAGE_STATES.find((candidate) => id === `state-${candidate}`)
  const fixture = state
    ? { id, values: state === 'success' || state === 'stale' ? [31, 42] : [] }
    : USAGE_FIXTURES.find((candidate) => candidate.id === id)
  if (fixture === undefined) return undefined
  const snapshot = normalizeUsage(
    fixture.values.flatMap((value, index) =>
      value === null
        ? []
        : [
            {
              kind: index === 0 ? 'five_hour' : 'seven_day',
              percentUsed: value,
              resetsAt: new Date(now + (index === 0 ? 5 : 168) * 60 * 60_000 * (1 - PREVIEW_PACE[index]! / 100)).toISOString(),
            },
          ],
    ),
    now,
  )
  if (id === 'error' || state === 'stale') snapshot.status = 'error'
  return {
    snapshot,
    warning: 'warning' in fixture ? fixture.warning : undefined,
    state,
    retryAt: state === 'retry' ? now + 60_000 : undefined,
  }
}

export function previewRetryText(retryAt: number | null, now: number): string | null {
  if (retryAt === null || !Number.isFinite(retryAt) || retryAt <= now) return null
  const date = new Date(Math.ceil(retryAt / 1000) * 1000)
  if (!Number.isFinite(date.getTime())) return 'Account check is temporarily unavailable'
  try {
    const formatter = new Intl.DateTimeFormat('en-US', {
      hour: 'numeric',
      minute: '2-digit',
      second: date.getUTCSeconds() === 0 ? undefined : '2-digit',
      hour12: true,
      timeZoneName: 'short',
    })
    const timeZone = formatter.resolvedOptions().timeZone
    if (!timeZone) throw new Error('Time zone unavailable')
    const parts = formatter.formatToParts(date)
    const part = (kind: Intl.DateTimeFormatPartTypes) => parts.find((value) => value.type === kind)?.value ?? ''
    const calendar = new Intl.DateTimeFormat('en-CA', { timeZone, year: 'numeric', month: '2-digit', day: '2-digit' })
    const dateKey = (value: number) => {
      const fields = calendar.formatToParts(value)
      return ['year', 'month', 'day'].map((kind) => fields.find((field) => field.type === kind)?.value ?? '').join('-')
    }
    const day = dateKey(date.getTime()) === dateKey(now) ? '' : `${dateKey(date.getTime())} `
    const seconds = part('second') ? `:${part('second')}` : ''
    return `Account retry at ${day}${part('hour')}:${part('minute')}${seconds}${part('dayPeriod').toLowerCase()} ${part('timeZoneName')}`
  } catch {
    const timestamp = date.toISOString()
    const day = timestamp.slice(0, 10) === new Date(now).toISOString().slice(0, 10) ? '' : `${timestamp.slice(0, 10)} `
    const hour = date.getUTCHours()
    const seconds = date.getUTCSeconds() === 0 ? '' : `:${timestamp.slice(17, 19)}`
    return `Account retry at ${day}${hour % 12 || 12}:${timestamp.slice(14, 16)}${seconds}${hour < 12 ? 'am' : 'pm'} UTC`
  }
}
