import { expect, test } from 'claude-code/testing'
import {
  dotFraction,
  filledDotCount,
  graphicDotCount,
  normalizeUsage,
  pace,
  paceLabel,
  percentage,
  primaryWindow,
  reachedPace,
  resetText,
  terminalDotCount,
  usageFailure,
  UsageOrder,
} from './usage'

const NOW = Date.parse('2026-01-01T00:00:00Z')
const limit = (percentUsed: number, resetsAt = '2026-01-01T02:48:00Z') => ({ kind: 'five_hour', percentUsed, resetsAt })

test('normalization preserves exact valid points, including exceeded values', () => {
  for (const value of [0, 1, 2.5, 8, 94, 99.5, 100, 105]) {
    const snapshot = normalizeUsage([limit(value)], NOW)
    expect(primaryWindow(snapshot, 'five_hour').percent).toBe(value)
    expect(percentage(primaryWindow(snapshot, 'five_hour'))).toBe(`${value}%`)
    expect(primaryWindow(snapshot, 'seven_day').percent).toBeNull()
  }
  for (const value of [-1, NaN, Infinity, -Infinity]) {
    expect(primaryWindow(normalizeUsage([limit(value)], NOW), 'five_hour').percent).toBeNull()
  }
})

test('empty clears previous values; errors retain them with an explicit error status', () => {
  const ready = normalizeUsage([limit(8)], NOW)
  expect(usageFailure(ready, NOW).status).toBe('error')
  expect(primaryWindow(usageFailure(ready, NOW), 'five_hour').percent).toBe(8)
  const empty = normalizeUsage([], NOW)
  expect(empty.status).toBe('empty')
  expect(empty.windows).toEqual([])
  expect(percentage(primaryWindow(empty, 'five_hour'))).toBe('—')
})

test('recognized additional windows stay separate from the primary windows', () => {
  const value = normalizeUsage(
    [
      { kind: 'spend_limit', percentUsed: 105 },
      { kind: 'unknown', percentUsed: 1 },
    ],
    NOW,
  )
  expect(value.windows.map((window) => window.label)).toEqual(['Spend'])
  expect(primaryWindow(value, 'five_hour').percent).toBeNull()
})

test('reset wording and pace share one clock and reject invalid period bounds', () => {
  const window = primaryWindow(normalizeUsage([limit(44)], NOW), 'five_hour')
  expect(resetText(window, NOW)).toBe('RESETS IN 2H 48M')
  const value = pace(window, NOW)!
  expect(paceLabel(value)).toBe('Even pace: 44% used by now')
  expect(reachedPace({ ...window, percent: value * 100 }, value)).toBe(true)
  expect(reachedPace({ ...window, percent: 43.999 }, value)).toBe(false)
  expect(reachedPace({ ...window, percent: 45 }, value)).toBe(true)
  expect(reachedPace({ ...window, percent: 43 }, value)).toBe(false)
  for (const resetsAt of [null, 'bad', '2025-12-31T23:59:00Z', '2026-01-01T06:00:00Z']) {
    expect(pace({ ...window, resetsAt }, NOW)).toBeNull()
  }
  expect(resetText({ ...window, resetsAt: null }, NOW)).toBe('Reset unavailable')
  expect(resetText({ ...window, resetsAt: '2025-12-31T23:59:00Z' }, NOW)).toBe('Reset due')
  expect(resetText({ ...window, resetsAt: '2026-01-06T17:00:00Z' }, NOW)).toBe('RESETS IN 5D 17H')
  expect(pace({ ...window, percent: null }, NOW)).toBeNull()
})

test('fractional fills retain endpoints and saturate exceeded values at every count', () => {
  for (const count of [2, 3, 7, 19, 20]) {
    for (const percent of [0, 1, 2.5, 8, 94, 99.5, 100, 105]) {
      const sum = Array.from({ length: count }, (_, i) => dotFraction(percent, count, i)).reduce((a, b) => a + b, 0)
      expect(Math.abs(sum - (Math.min(100, percent) / 100) * count) < 0.000001).toBe(true)
    }
  }
  expect(dotFraction(8, 20, 0)).toBe(1)
  expect(Math.abs(dotFraction(8, 20, 1) - 0.6) < 0.000001).toBe(true)
  expect(dotFraction(2.5, 20, 0)).toBe(0.5)
  expect(dotFraction(null, 20, 0)).toBe(0)
  expect(graphicDotCount(199)).toBe(20)
  expect(graphicDotCount(198)).toBe(19)
  expect(graphicDotCount(18)).toBe(1)
  expect(terminalDotCount(39)).toBe(20)
  expect(terminalDotCount(38)).toBe(19)
  expect(terminalDotCount(99)).toBe(50)
})

test('late reads and failures cannot supersede a newer read, event, or session', () => {
  const order = new UsageOrder()
  const oldRead = order.beginRead()
  const event = order.acceptEvent()
  expect(order.isCurrent(oldRead)).toBe(false)
  expect(order.isCurrent(event)).toBe(true)
  const latest = order.beginRead()
  expect(order.isCurrent(event)).toBe(false)
  expect(order.isCurrent(latest)).toBe(true)
  order.reset()
  expect(order.isCurrent(latest)).toBe(false)
})

test('a weekly-only reading uses the inactive session copy without changing unknown usage', () => {
  const snapshot = normalizeUsage([{ kind: 'seven_day', percentUsed: 1 }], NOW)
  const window = primaryWindow(snapshot, 'five_hour')
  expect(resetText(window, NOW)).toBe('Starts when a message is sent')
  expect(percentage(window)).toBe('—')
  expect(resetText(primaryWindow(snapshot, 'seven_day'), NOW)).toBe('Reset unavailable')
  expect(resetText(primaryWindow(normalizeUsage([], NOW), 'five_hour'), NOW)).toBe('Reset unavailable')
  expect(resetText(primaryWindow(usageFailure(snapshot, NOW), 'five_hour'), NOW)).toBe('Reset unavailable')
  const invalid = normalizeUsage(
    [
      { kind: 'five_hour', percentUsed: NaN },
      { kind: 'seven_day', percentUsed: 1 },
    ],
    NOW,
  )
  expect(resetText(primaryWindow(invalid, 'five_hour'), NOW)).toBe('Reset unavailable')
})

test('unknown or malformed readings remain unavailable until a valid recognized reading exists', () => {
  for (const readings of [
    [{ kind: 'unknown', percentUsed: 1 }],
    [{ kind: 'five_hour', percentUsed: NaN }],
    [{ kind: 'five_hour', percentUsed: -1 }],
    [{ kind: 'five_hour', percentUsed: Infinity }],
  ]) {
    const snapshot = normalizeUsage(readings, NOW)
    expect(snapshot.status).toBe('empty')
    expect(primaryWindow(snapshot, 'five_hour').percent).toBeNull()
    expect(resetText(primaryWindow(snapshot, 'five_hour'), NOW)).toBe('Reset unavailable')
  }
  expect(normalizeUsage([limit(NaN), { kind: 'seven_day', percentUsed: 0 }], NOW).status).toBe('ready')
})

test('whole-dot rounding is monotonic and reserves the full track for 100 percent', () => {
  for (const count of [2, 3, 7, 20, 40]) {
    let prior = 0
    for (let percent = 0; percent <= 100; percent += 0.25) {
      const filled = filledDotCount(percent, count)
      expect(Number.isInteger(filled)).toBe(true)
      expect(filled >= prior && filled <= count).toBe(true)
      if (percent > 0 && percent < 100) expect(filled >= 1 && filled < count).toBe(true)
      prior = filled
    }
  }
  expect(filledDotCount(8, 20)).toBe(2)
  expect(filledDotCount(1, 20)).toBe(1)
  expect(filledDotCount(99, 20)).toBe(19)
  expect(filledDotCount(100, 20)).toBe(20)
  expect(filledDotCount(null, 20)).toBe(0)
  expect(filledDotCount(Number.NaN, 20)).toBe(0)
})
