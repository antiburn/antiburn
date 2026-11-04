import { expect, test } from 'claude-code/testing'
import { BRAND_MARK_SVG, markSvg, responsiveMeterSvg } from './usage-meter-graphics'
import { usageLayout } from './usage-presentation'
import { secondaryTextSvg, usageStatusSvg } from './usage-meter-graphics'
import { normalizeUsage, primaryWindow } from './usage'
import { usageStatusStripSvg, usageStripSvg } from './usage-strip-graphics'

const NOW = Date.parse('2026-01-01T00:00:00Z')
const snapshot = normalizeUsage(
  [
    { kind: 'five_hour', percentUsed: 68, resetsAt: '2026-01-01T02:48:00Z' },
    { kind: 'seven_day', percentUsed: 42, resetsAt: '2026-01-04T00:00:00Z' },
  ],
  NOW,
)
const windows = [
  primaryWindow(snapshot, 'five_hour'),
  primaryWindow(snapshot, 'seven_day'),
]

function meterSources(): string[] {
  return windows.map((window, index) =>
    responsiveMeterSvg(window, NOW, {
      layout: 'inline',
      entranceElapsed: index === 0 ? 400 : 520,
      previousPercent: index === 0 ? '9%' : '39%',
      rollElapsed: 36,
      meterIndex: index as 0 | 1,
    }),
  )
}

function referencesAreLocal(svg: string): boolean {
  const ids = [...svg.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]!)
  if (new Set(ids).size !== ids.length) return false
  const known = new Set(ids)
  const references = [
    ...[...svg.matchAll(/\bhref="#([^"]+)"/g)].map((match) => match[1]!),
    ...[...svg.matchAll(/url\(#([^)]+)\)/g)].map((match) => match[1]!),
  ]
  return references.every((reference) => known.has(reference))
}

test('composes one responsive, interactive SVG with namespaced meter internals', () => {
  const layout = usageLayout(720)
  const svg = usageStripSvg(markSvg(BRAND_MARK_SVG, 120), meterSources(), layout, 80)

  expect(svg.startsWith('<svg ')).toBe(true)
  expect(svg.endsWith('</svg>')).toBe(true)
  expect(svg.match(/<svg\b/g)?.length).toBe(1)
  expect(svg.includes('translate(-6.2 -5.9)')).toBe(true)
  expect(svg.includes('strip-brand-mark-in')).toBe(true)
  expect(svg.includes('-120ms both')).toBe(true)
  expect(svg.includes('strip-meter-0-bitcount-pixel')).toBe(true)
  expect(svg.includes('strip-meter-1-bitcount-pixel')).toBe(true)
  expect(svg.includes('strip-meter-0-meter-in')).toBe(true)
  expect(svg.includes('strip-meter-1-meter-in')).toBe(true)
  expect(svg.includes('tabindex="0"')).toBe(true)
  expect(svg.includes('class="pace" tabindex="0"')).toBe(true)
  expect(svg.includes('RESETS IN')).toBe(true)
  expect(svg.includes('reset-copy')).toBe(true)
  expect(referencesAreLocal(svg)).toBe(true)
})

test('keeps stacked narrow meters and inline wide meters within the source bound', () => {
  for (const width of [200, 480, 960]) {
    const layout = usageLayout(width)
    const svg = usageStripSvg(markSvg(BRAND_MARK_SVG, 120), meterSources(), layout, 80)
    expect(svg.includes(`height="${layout.height}"`)).toBe(true)
    expect(svg.length < 131072).toBe(true)
    expect(referencesAreLocal(svg)).toBe(true)
    expect(svg.includes('var(--meter-width)')).toBe(true)
    expect(svg.includes('strip-mark')).toBe(true)
  }
})

test('rejects missing meters and marks without a usable viewBox', () => {
  const layout = usageLayout(480)
  expect(() =>
    usageStripSvg(BRAND_MARK_SVG, meterSources().slice(0, 1), layout, 80),
  ).toThrow()
  expect(() =>
    usageStripSvg('<svg><circle/></svg>', meterSources(), layout, 80),
  ).toThrow()
})

test('composes an accessible status and optional retry line into one branded SVG', () => {
  const status = usageStatusSvg('Retrying usage...', 'Next check at 12:04pm UTC')
  const secondary = secondaryTextSvg(
    ['Account usage is temporarily unavailable'],
    240,
    1,
  )
  const svg = usageStatusStripSvg(
    markSvg(BRAND_MARK_SVG, 120),
    status,
    80,
    secondary,
    true,
  )

  expect(svg.match(/<svg\b/g)?.length).toBe(1)
  expect(svg.includes(`height="${status.height + secondary.height}"`)).toBe(true)
  expect(
    svg.includes('<title>Retrying usage.... Next check at 12:04pm UTC</title>'),
  ).toBe(true)
  expect(svg.includes('strip-status-viewport')).toBe(true)
  expect(svg.includes('strip-status-secondary-viewport')).toBe(true)
  expect(svg).toContain('style="width:max(0px, calc(100vw - 8 * var(--cell)))"')
  expect(referencesAreLocal(svg)).toBe(true)
})

test('retains branded replay identities when flattening source roots', () => {
  const layout = usageLayout(720)
  const scene = (revision: number) =>
    usageStripSvg(
      markSvg(BRAND_MARK_SVG, 0).replace('<svg ', `<svg data-entrance="${revision}" `),
      meterSources().map((source) =>
        source.replace('<svg ', `<svg data-entrance="${revision}" `),
      ),
      layout,
      80,
    )
  const first = scene(1)
  const second = scene(2)
  expect(first.match(/data-entrance="1"/g)).toHaveLength(3)
  expect(second.match(/data-entrance="2"/g)).toHaveLength(3)
  expect(second).not.toBe(first)
})
