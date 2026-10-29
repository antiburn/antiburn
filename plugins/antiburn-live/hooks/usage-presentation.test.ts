import { expect, test } from 'claude-code/testing'
import {
  desktopPercentage,
  EntranceOwner,
  METER_HEIGHT,
  PRESENTATION,
  entranceFrame,
  meterGeometry,
  rollingColumns,
  usageLayout,
} from './usage-presentation'
import { captionGlyphs, captionSvg, captionWidth } from './usage-font'
import type { UsageWindow } from './usage'
import { meterLayer, meterSvg } from './usage-graphics'

const NOW = Date.parse('2026-01-01T00:00:00Z')
const window: UsageWindow = { kind: 'five_hour', label: '5-hour', percent: 8, resetsAt: '2026-01-01T02:48:00Z' }

test('shared layout keeps the mark centered against the complete meter block', () => {
  for (const width of [320, 479, 480, 481]) {
    const layout = usageLayout(width)
    expect(layout.inline).toBe(width >= 480)
    expect(layout.mark.y + 6.5).toBe(layout.height / 2)
    expect(layout.meters[0]!.x - layout.mark.x - 13).toBe(13.5)
    expect(8 + layout.mark.x).toBe(13.5)
    const last = layout.meters[1]!
    expect(width - (last.x + layout.meterWidth)).toBe(5)
    expect(last.y + METER_HEIGHT).toBe(layout.height)
  }
})

test('glyph data uses tabular figures while labels retain proportional spacing', () => {
  expect(captionWidth('111', 15, true)).toBe(captionWidth('888', 15, true))
  expect(captionWidth('111')).not.toBe(captionWidth('888'))
  for (const text of ['5-hour', 'Weekly', '2.5%', '105%', '—']) {
    expect(captionGlyphs(text).every((glyph) => glyph.dots.length > 0)).toBe(true)
  }
})

test('boundary fixtures settle on whole dots at every adaptive count', () => {
  for (const width of [19, 73, 198, 199, 288, 584]) {
    for (const percent of [0, 1, 2.5, 8, 94, 99.5, 100, 105]) {
      const scene = meterGeometry({ ...window, percent }, width, NOW)
      const total = scene.dots.reduce((sum, dot) => sum + dot.fraction, 0)
      expect(scene.dots.every((dot) => dot.fraction === 0 || dot.fraction === 1)).toBe(true)
      expect(Math.abs(total - (Math.min(percent, 100) / 100) * scene.dots.length) <= 1).toBe(true)
      if (percent === 0) expect(total).toBe(0)
      if (percent >= 100) expect(total).toBe(scene.dots.length)
      expect(scene.fillEnd <= width).toBe(true)
      const end = entranceFrame(960)
      expect(scene.fillEnd * end.fill).toBe(scene.fillEnd)
      expect(entranceFrame(1000)).toEqual(end)
    }
  }
})

test('entrance moves whole meters before the fixed-value fill sweep', () => {
  expect(entranceFrame(0).markOpacity).toBe(0)
  expect(entranceFrame(100).meters[0]!.opacity).toBe(0)
  expect(entranceFrame(240).markScale).toBe(1)
  expect(entranceFrame(240).meters[1]!.opacity).toBe(0)
  expect(entranceFrame(460).meters[0]).toEqual({ opacity: 1, rise: 0 })
  expect(entranceFrame(600).meters[1]).toEqual({ opacity: 1, rise: 0 })
  expect(entranceFrame(600).fill).toBe(0)
  expect(entranceFrame(780).fill > 0 && entranceFrame(780).fill < 1).toBe(true)
  expect(entranceFrame(959).done).toBe(false)
  expect(entranceFrame(960).done).toBe(true)
  for (const time of [0, 40, 80]) {
    const frame = entranceFrame(time, true)
    expect(frame.fill).toBe(1)
    expect(frame.markScale).toBe(1)
    expect(frame.meters[0]!.rise).toBe(0)
    expect(frame.meters[0]!.opacity).toBe(frame.meters[1]!.opacity)
  }
})

test('right-aligned number changes roll only changed digits with bounded stagger', () => {
  expect(rollingColumns('9%', '10%').map((column) => column.changed)).toEqual([true, true, false])
  expect(rollingColumns('99%', '100%').map((column) => column.changed)).toEqual([true, true, true, false])
  expect(rollingColumns('99.5%', '99.4%').map((column) => column.changed)).toEqual([false, false, false, true, false])
  expect(rollingColumns('—', '0%').map((column) => column.changed)).toEqual([true, false])
  expect(rollingColumns('99%', '99%').some((column) => column.changed)).toBe(false)
  expect(rollingColumns('100%', '9%').every((column) => column.delay <= 24)).toBe(true)
})

test('entrance claims once and invalidates callbacks on interruption', () => {
  const owner = new EntranceOwner()
  expect(owner.claim(NOW, true, true)).toBe(false)
  owner.load(true)
  expect(owner.claim(NOW, false, true)).toBe(false)
  expect(owner.claim(NOW, true, false)).toBe(false)
  expect(owner.claim(NOW, true, true)).toBe(true)
  const generation = owner.state.generation
  expect(owner.claim(NOW, true, true)).toBe(false)
  expect(owner.accepts(generation)).toBe(true)
  owner.cancel()
  expect(owner.accepts(generation)).toBe(false)
  expect(owner.claim(NOW, true, true)).toBe(false)
  owner.load(false)
  expect(owner.claim(NOW, true, true)).toBe(false)
})

test('reset glyphs and extended pace strokes remain centered without clipping', () => {
  const inkCenter = (text: string, size: number, baseline: number) => {
    const ys = captionGlyphs(text, size).flatMap((glyph) => glyph.dots.map((dot) => dot[1]))
    return baseline - (((Math.min(...ys) + Math.max(...ys) + 100) / 2) * size) / 1000
  }
  expect(
    Math.abs(
      inkCenter('5-hour', 14, PRESENTATION.captionBaseline) -
        inkCenter('Starts when a message is sent', 12, PRESENTATION.resetBaseline),
    ) < 1e-9,
  ).toBe(true)
  const geometry = meterGeometry(window, 288, NOW)
  const marker = geometry.marker!
  expect(marker.y).toBe(geometry.dots[0]!.y)
  expect(marker.height).toBe(18)
  expect(marker.y - marker.height / 2 >= PRESENTATION.captionBaseline).toBe(true)
  expect(marker.y + marker.height / 2 <= METER_HEIGHT).toBe(true)
})

test('rendered neutral pace line has continuous symmetric bounds around the dots', () => {
  const svg = meterSvg(window, 288, NOW)
  const path = svg.match(/class="marker" d="([^"]+)"/)![1]!
  const segments = [...path.matchAll(/M[\d.]+ ([\d.]+)v([\d.]+)/g)].map((match) => ({
    y: Number(match[1]),
    height: Number(match[2]),
  }))
  expect(segments.length).toBe(1)
  const center = meterGeometry(window, 288, NOW).dots[0]!.y
  expect(segments[0]!.height).toBe(18)
  expect(segments[0]!.y + segments[0]!.height / 2).toBe(center)
})

test('Label reveal aligns complete window rows and bounds the fixed-pitch track', () => {
  for (const width of [192, 320, 479, 480, 481, 960]) {
    const layout = usageLayout(width, 'inline')
    expect(layout.mark.y + 6.5).toBe(layout.height / 2)
    expect(layout.meterHeight).toBe(18)
    expect(layout.stackedGap).toBe(1)
    for (const label of ['5-hour', 'Weekly']) {
      for (const percent of [0, 1, 2.5, 8, 44, 94, 99.5, 100, 105]) {
        const scene = meterGeometry(
          { ...window, label, percent },
          layout.meterWidth,
          NOW,
          'inline',
        )
        expect(scene.trackX > scene.labelWidth).toBe(true)
        expect(scene.trackX + scene.trackWidth < scene.percentX).toBe(true)
        expect(scene.dots.length).toBe(
          Math.max(0, Math.floor((scene.trackWidth - 7) / 11) + 1) < 2
            ? 0
            : Math.floor((scene.trackWidth - 7) / 11) + 1,
        )
        expect(
          scene.dots.every(
            (dot) =>
              dot.y === 9 && dot.x - 3.5 >= scene.trackX && dot.x + 3.5 <= scene.percentX,
          ),
        ).toBe(true)
        const total = scene.dots.reduce((sum, dot) => sum + dot.fraction, 0)
        expect(scene.dots.every((dot) => dot.fraction === 0 || dot.fraction === 1)).toBe(true)
        expect(Math.abs(total - (Math.min(percent, 100) / 100) * scene.dots.length) <= 1).toBe(
          true,
        )
        if (scene.marker) expect(scene.marker.y).toBe(9)
      }
    }
  }
})

test('Label reveal limits reset overlays to the track and retains native interaction motion', () => {
  const svg = meterSvg(window, 288, NOW, false, true, { layout: 'inline', previousFillEnd: 100, fillElapsed: 100 })
  const scene = meterGeometry(window, 288, NOW, 'inline')
  expect(svg).toContain(`x="${scene.trackX}" width="${scene.trackWidth}" height="18"`)
  expect(svg).toContain('.meter:has(.label:hover) .reset')
  expect(svg).toContain('.meter:has(.pace:hover) .pace-detail')
  expect(svg).toContain('transition-duration:280ms;transition-delay:300ms')
  expect(svg).toContain(
    '.meter:has(.pace:hover) .reset.detail-reveal{opacity:0;clip-path:inset(0 100% 0 0);transition-duration:280ms;transition-delay:300ms;animation:none}',
  )
  expect(svg).toContain('.meter:has(.pace:focus) .reset{opacity:0;clip-path:inset(0 100% 0 0);transition:none;animation:none}')
  expect(svg).toContain('transition:opacity 180ms')
  expect(svg).toContain('prefers-reduced-motion:reduce')
  expect(svg).toContain(`transform-origin:${scene.trackX}px 0`)
  expect(svg).not.toContain('<title>')
})

test('both layouts keep seven-pixel dots and four-pixel gaps at every width', () => {
  for (const variant of ['inline', 'current'] as const) {
    for (let width = 100; width <= 1000; width++) {
      const scene = meterGeometry(window, width, NOW, variant)
      const dots = scene.dots
      for (let index = 1; index < dots.length; index++) {
        expect(
          Math.abs(dots[index]!.x - dots[index - 1]!.x - PRESENTATION.dotPitch) < 1e-9,
        ).toBe(true)
        expect(
          Math.abs(
            dots[index]!.x - dots[index - 1]!.x - PRESENTATION.dotSize - PRESENTATION.dotGap,
          ) < 1e-9,
        ).toBe(true)
      }
      if (dots.length > 0) {
        const end = dots[dots.length - 1]!.x + PRESENTATION.dotSize / 2
        expect(end <= scene.trackX + scene.trackWidth).toBe(true)
        expect(scene.trackX + scene.trackWidth - end < PRESENTATION.dotPitch).toBe(true)
        expect(
          scene.marker!.x >= dots[0]!.x && scene.marker!.x <= dots[dots.length - 1]!.x,
        ).toBe(true)
      }
    }
  }
})

test('inline captions center their visible glyphs on the mark and fit the compact row', () => {
  for (const label of ['5-hour', 'Weekly', '0%', '100%']) {
    const scene = meterGeometry({ ...window, label }, 288, NOW, 'inline')
    const ys = captionGlyphs(label, 14).flatMap((glyph) => glyph.dots.map((dot) => dot[1]))
    const center =
      scene.baseline - (((Math.min(...ys) + Math.max(...ys) + 100) / 2) * 14) / 1000
    expect(center).toBe(scene.height / 2)
    expect(scene.trackY).toBe(center)
    expect(scene.marker!.y - scene.marker!.height / 2 >= 0).toBe(true)
    expect(scene.marker!.y + scene.marker!.height / 2 <= scene.height).toBe(true)
  }
})

test('pace details sit beside the marker and switch sides near the end', () => {
  for (const elapsed of [0.05, 0.4, 0.55, 0.9, 0.99]) {
    const reading = { ...window, resetsAt: new Date(NOW + (1 - elapsed) * 5 * 3600000).toISOString() }
    const scene = meterGeometry(reading, 500, NOW, 'inline')
    const marker = scene.marker!
    const text = `Even pace: ${Math.round(marker.value * 100)}%`
    const textWidth = captionWidth(text, 12, false, 0.025)
    const x = scene.trackX + scene.trackWidth - marker.x >= marker.x - scene.trackX ? marker.x + 6 : marker.x - 6 - textWidth
    const svg = meterSvg(reading, 500, NOW, false, true, { layout: 'inline' })
    expect(svg).toContain(captionSvg(text, x, scene.height / 2 + 3.6, 12, false, 0.025))
    expect(x >= scene.trackX).toBe(true)
    expect(x + textWidth <= scene.trackX + scene.trackWidth).toBe(true)
  }
})

test('resting dots are whole while the fill front can pass through circles in motion', () => {
  for (const percent of [0, 1, 2.5, 8, 44.5, 98, 100]) {
    const reading = { ...window, percent }
    const steady = meterSvg(reading, 288, NOW)
    expect(steady).not.toContain('id="dot-')
    const scene = meterGeometry(reading, 288, NOW)
    const moving = meterSvg(reading, 288, NOW, false, true, { previousFillEnd: 0, fillElapsed: 100 })
    expect(moving).toContain('clip-path="url(#fill-front)"')
    expect(moving).toContain('@keyframes preview-fill')
    if (scene.dots.some((dot) => dot.fraction > 0)) {
      const last = [...scene.dots].reverse().find((dot) => dot.fraction === 1)!
      expect(scene.fillEnd).toBe(last.x + 3.5)
    }
  }
})


test('inline dot tracks share their starting position across both window labels', () => {
  for (const width of [192, 320, 639, 640, 960]) {
    const five = meterGeometry({ ...window, label: '5-hour' }, width, NOW, 'inline')
    const weekly = meterGeometry({ ...window, label: 'Weekly' }, width, NOW, 'inline')
    expect(five.trackX).toBe(weekly.trackX)
    expect(five.dots[0]?.x).toBe(weekly.dots[0]?.x)
    expect(five.marker?.x).toBe(weekly.marker?.x)
  }
})

test('desktop primary glyph size stays consistent during rolls and reset reveals', () => {
  for (const layout of ['inline', 'current'] as const) {
    const svg = meterSvg(window, 288, NOW, true, true, {
      layout,
      previousPercent: '9%',
      rollElapsed: 100,
    })
    expect(svg).toContain('scale(0.014 -0.014)')
    expect(svg).not.toContain('scale(0.015 -0.015)')
    expect(svg).toContain('scale(0.012 -0.012)')
    expect(svg).toContain('class="roll-old"')
    expect(svg).toContain('class="roll-new"')
  }
})

test('desktop warning dots use the smaller radius and never split at the pace line', () => {
  const scene = meterGeometry({ ...window, percent: 94 }, 288, NOW, 'inline')
  const svg = meterSvg({ ...window, percent: 94 }, 288, NOW, false, true, {
    layout: 'inline',
    warning: 'red',
    previousFillEnd: 0,
    fillElapsed: 100,
  })
  const red = [
    ...svg.matchAll(/<circle class="over-used" cx="([\d.]+)" cy="[\d.]+" r="([\d.]+)"/g),
  ]
  expect(red.length > 0).toBe(true)
  expect(
    red.every(
      (circle) => Number(circle[2]) === 3.5 && Number(circle[1]) - 3.5 >= scene.marker!.x,
    ),
  ).toBe(true)
  expect(scene.fillEnd).toBe(scene.dots.filter((dot) => dot.fraction === 1).at(-1)!.x + 3.5)
})

test('compact inline bars retain meaningful tracks at the stacking boundary', () => {
  for (const width of [480, 488, 520, 560, 639]) {
    const layout = usageLayout(width, 'inline')
    expect(layout.inline).toBe(true)
    for (const percent of [0, 99.5, 105]) {
      const scene = meterGeometry(
        { ...window, label: 'Weekly', percent },
        layout.meterWidth,
        NOW,
        'inline',
      )
      expect(scene.dots.length >= 5).toBe(true)
      expect(scene.marker).not.toBe(null)
    }
  }
  const stacked = usageLayout(479, 'inline')
  expect(stacked.inline).toBe(false)
  expect(stacked.height).toBe(37)
})

test('desktop orange and red fills share the quieter opacity without dimming text', () => {
  for (const warning of [undefined, 'red', 'pulse'] as const) {
    const svg = meterSvg({ ...window, percent: 94 }, 288, NOW, false, true, {
      layout: 'inline',
      warning,
    })
    expect(svg).toContain('.sweep{opacity:0.85}')
    expect(svg).toContain('.ink{fill:#252525}')
  }
})

test('desktop percentages show whole numbers without rounding into exhaustion', () => {
  for (const [percent, expected] of [
    [0, '0%'],
    [2.5, '2%'],
    [99.5, '99%'],
    [99.999, '99%'],
    [100, '100%'],
    [105.9, '105%'],
    [null, '—'],
  ] as const) {
    const reading = { ...window, percent }
    expect(desktopPercentage(reading)).toBe(expected)
    expect(meterGeometry(reading, 288, NOW, 'inline').percent).toBe(expected)
    expect(reading.percent).toBe(percent)
  }
})

test('desktop exhaustion uses steady red regardless of preview warning', () => {
  for (const warning of [undefined, 'red', 'pulse'] as const) {
    const svg = meterSvg({ ...window, percent: 100 }, 288, NOW, false, true, {
      layout: 'inline',
      warning,
    })
    expect(svg).toContain('.used{fill:#ed4545}')
    expect(svg).not.toContain('@keyframes debug-pulse')
    expect(svg).toContain('stroke="#ed4545"')
    expect(svg).toContain('5-hour: 100%.')
  }
})


test('layered desktop meters draw the marker shadow only in the base layer', () => {
  const source = meterSvg(window, 288, NOW, false, true, { layout: 'inline' })
  const base = meterLayer(source, 'base')
  const interaction = meterLayer(source, 'interaction')
  expect(base).toContain('class="marker-shadow"')
  expect(base).not.toContain('.data,.caption-ink,.marker-shadow{display:none}')
  expect(interaction).toContain('.data,.caption-ink,.marker-shadow{display:none}')
  expect(interaction).toContain('class="pace" tabindex="0"')
  expect(interaction).toContain('.pace:focus .marker{opacity:1')
})
