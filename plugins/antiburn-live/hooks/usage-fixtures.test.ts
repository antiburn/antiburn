import { expect, test } from 'claude-code/testing'
import { PREVIEW_PACE, USAGE_FIXTURES, usageFixture, previewRetryText } from './usage-fixtures'
import { pace, primaryWindow } from './usage'
import { meterSvg, usageStatusSvg, secondaryTextSvg } from './usage-graphics'

const NOW = 1_800_000_000_000

test('synthetic fixtures use a frozen clock and preserve their intended pace relation', () => {
  for (const item of USAGE_FIXTURES) {
    const preview = usageFixture(item.id, NOW)!
    expect(preview.snapshot.capturedAt).toBe(NOW)
    expect(usageFixture(item.id, NOW)).toEqual(preview)
  }
  const behind = primaryWindow(usageFixture('mid', NOW)!.snapshot, 'five_hour')
  expect(behind.percent! / 100 < pace(behind, NOW)!).toBe(true)
  const ahead = primaryWindow(usageFixture('beyond-pace', NOW)!.snapshot, 'five_hour')
  expect(ahead.percent! / 100 > pace(ahead, NOW)!).toBe(true)
  expect(usageFixture('invalid', NOW)).toBeUndefined()
  expect(usageFixture('exceeded', NOW)).toBeUndefined()
  expect(usageFixture('decimal', NOW)).toBeUndefined()
  expect(usageFixture('empty', NOW)!.snapshot.status).toBe('empty')
  expect(usageFixture('error', NOW)!.snapshot.status).toBe('error')
  expect(primaryWindow(usageFixture('inactive', NOW)!.snapshot, 'five_hour').percent).toBeNull()
})

test('warning motion is opt-in, finite, and disabled by reduced motion', () => {
  const window = primaryWindow(usageFixture('near-limit', NOW)!.snapshot, 'five_hour')
  expect(meterSvg(window, 288, NOW)).not.toMatch(/debug-pulse/)
  const preview = meterSvg(window, 288, NOW, false, true, { warning: 'pulse' })
  expect(preview).toMatch(/debug-pulse 1600ms cubic-bezier\(\.77,0,\.175,1\) 6/)
  expect(preview).toMatch(/prefers-reduced-motion:reduce/)
  expect(preview).toMatch(/animation:none!important/)
  expect(preview).not.toMatch(/infinite/)
})

test('preview fill uses a target front for growth, decrease and reduced motion', () => {
  const window = primaryWindow(usageFixture('mid', NOW)!.snapshot, 'five_hour')
  for (const previousFillEnd of [20, 260]) {
    const source = meterSvg(window, 288, NOW, false, true, { previousFillEnd, fillElapsed: 100 })
    expect(source).toMatch(/preview-fill 360ms/)
    expect(source).toMatch(/\.sweep\{clip-path:url\(#fill-front\)\}/)
    expect(source).not.toMatch(/\.fill-front\{animation:none;transform:none\}/)
  }
})

test('red begins at a reached pace marker while only near-limit uses all-red fill', () => {
  const ahead = primaryWindow(usageFixture('beyond-pace', NOW)!.snapshot, 'five_hour')
  const source = meterSvg(ahead, 288, NOW, false, true, { warning: 'red' })
  expect(source).toMatch(/used\{fill:#ff5914\}/)
  expect(source).toMatch(/id="over-pace"/)
  expect(source).toMatch(/class="over-used"/)
  const behind = primaryWindow(usageFixture('mid', NOW)!.snapshot, 'five_hour')
  expect(meterSvg(behind, 288, NOW, false, true, { warning: 'red' })).not.toMatch(/id="over-pace"/)
  expect(meterSvg(ahead, 288, NOW, false, true, { warning: 'pulse' })).toMatch(/used\{fill:#ed4545\}/)
})

test('all synthetic examples keep the same pace marker positions', () => {
  for (const item of USAGE_FIXTURES) {
    for (const [index, kind] of ['five_hour', 'seven_day'].entries()) {
      const window = primaryWindow(usageFixture(item.id, NOW)!.snapshot, kind as 'five_hour' | 'seven_day')
      if (window.resetsAt === null) continue
      expect(pace(window, NOW)).toBe(PREVIEW_PACE[index]! / 100)
    }
  }
})

test('current release state previews remain synthetic and keep retry time frozen', () => {
  for (const state of ['idle', 'checking', 'retry', 'success', 'stale']) {
    const fixture = usageFixture(`state-${state}`, NOW)!
    expect(fixture.state).toBe(state)
    expect(fixture.snapshot.capturedAt).toBe(NOW)
    expect(fixture).toEqual(usageFixture(`state-${state}`, NOW))
    expect(fixture.snapshot.status).toBe(state === 'stale' ? 'error' : state === 'success' ? 'ready' : 'empty')
    expect(primaryWindow(fixture.snapshot, 'five_hour').percent).toBe(state === 'success' || state === 'stale' ? 31 : null)
    expect(fixture.retryAt).toBe(state === 'retry' ? NOW + 60_000 : undefined)
  }
  expect(usageFixture('state-invalid', NOW)).toBeUndefined()
  const hint = previewRetryText(NOW + 60_001, NOW)!
  expect(hint.startsWith('Account retry at ')).toBe(true)
  expect(previewRetryText(NOW + 60_001, NOW)).toBe(hint)
  expect(previewRetryText(NOW, NOW)).toBeNull()
})

test('checking and retry previews use the current animated and dim-dot graphics', () => {
  const checking = usageStatusSvg('Checking usage...')
  expect(checking.description).toBe('Checking usage...')
  expect((checking.source.match(/class="checking-dot"/g) ?? []).length).toBe(3)
  expect(checking.source).toContain('@media(prefers-reduced-motion:reduce){.checking-dot{animation:none;opacity:1}}')
  const retry = secondaryTextSvg(['Account retry at 3:07pm GMT+10 · Refresh checks session.'], 192, 2)
  expect(retry.height).toBe(36)
  expect(retry.source).toContain('class="hint"')
  expect(retry.source).toContain('bitcount-pixel')
})
