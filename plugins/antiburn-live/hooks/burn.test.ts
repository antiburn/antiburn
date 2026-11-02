import { expect, test } from 'claude-code/testing'
import {
  BURN_GEOMETRY,
  BURN_HEIGHT,
  BURN_IGNITION,
  BURN_POLICY,
  BURN_WIDTH,
  BurnActivity,
  burnAnimationSvg,
  burnFrame,
  burnMarkSvg,
} from './burn'

function feedHeavy(activity: BurnActivity, id: string, start = 0): void {
  activity.start(id, start)
  for (let offset = 0; offset <= BURN_POLICY.minObservationMs; offset += BURN_POLICY.bucketMs) {
    activity.chunk(id, 500, start + offset)
  }
}

const BASE64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'

function decodeBase64(value: string): Uint8Array {
  const bytes: number[] = []
  for (let index = 0; index < value.length; index += 4) {
    const a = BASE64.indexOf(value[index]!)
    const b = BASE64.indexOf(value[index + 1]!)
    const c = value[index + 2] === '=' ? 0 : BASE64.indexOf(value[index + 2]!)
    const d = value[index + 3] === '=' ? 0 : BASE64.indexOf(value[index + 3]!)
    const packed = (a << 18) | (b << 12) | (c << 6) | d
    bytes.push((packed >>> 16) & 255)
    if (value[index + 2] !== '=') bytes.push((packed >>> 8) & 255)
    if (value[index + 3] !== '=') bytes.push(packed & 255)
  }
  return Uint8Array.from(bytes)
}

test('stream estimates require an observation interval and cool when the signal is stale', () => {
  const activity = new BurnActivity()
  activity.start('request', 0)
  activity.chunk('request', 2_000, 0)
  expect(activity.snapshot(BURN_POLICY.minObservationMs - 1).active).toBe(false)
  activity.chunk('request', 500, BURN_POLICY.minObservationMs)
  const burning = activity.snapshot(BURN_POLICY.minObservationMs)
  expect(burning.active).toBe(true)
  expect(burning.estimatedTps >= BURN_POLICY.entryTps).toBe(true)
  expect(burning.source).toBe('stream-estimate')

  const stale = activity.snapshot(BURN_POLICY.minObservationMs + BURN_POLICY.staleMs + 1)
  expect(stale.active).toBe(false)
  expect(stale.estimatedTps).toBe(0)
})

test('completion reports a request average, calibrates later estimates, and ends the final active request', () => {
  const activity = new BurnActivity()
  activity.start('measured', 0)
  activity.chunk('measured', 400, 1_000)
  activity.complete('measured', 200, 4_000)
  const completed = activity.snapshot(4_000)
  expect(completed.active).toBe(false)
  expect(completed.measuredTps).toBe(50)
  expect(completed.source).toBe('completed-request-average')

  activity.start('calibrated', 4_000)
  activity.chunk('calibrated', 400, 4_000)
  const calibrated = activity.snapshot(6_000)
  expect(Math.abs(calibrated.estimatedTps - 100) < 0.000_001).toBe(true)
})

test('duplicate and invalid completions cannot replace a valid measurement', () => {
  const activity = new BurnActivity()
  activity.start('one', 0)
  activity.chunk('one', 400, 1_000)
  activity.complete('one', 80, 2_000)
  activity.complete('one', 8_000, 2_001)
  expect(activity.snapshot(2_001).measuredTps).toBe(40)

  activity.start('two', 2_001)
  activity.complete('two', Number.NaN, 3_001)
  expect(activity.snapshot(3_001).measuredTps).toBe(40)
  activity.start('three', 3_001)
  activity.complete('three', -1, 4_001)
  expect(activity.snapshot(4_001).measuredTps).toBe(40)
})

test('concurrent requests aggregate chunks while completion rates stay request scoped', () => {
  const activity = new BurnActivity()
  activity.start('a', 0)
  activity.start('b', 0)
  activity.chunk('a', 500, 0)
  activity.chunk('b', 500, 1_000)
  activity.chunk('b', 500, 2_000)
  activity.complete('a', 200, 2_000)
  const concurrent = activity.snapshot(2_000)
  expect(concurrent.active).toBe(true)
  expect(concurrent.measuredTps).toBe(100)
  expect(concurrent.source).toBe('completed-request-average')
  activity.end('b', 2_001)
  expect(activity.snapshot(2_001).active).toBe(false)
})

test('abort cools midstream and a backward clock starts a clean generation', () => {
  const activity = new BurnActivity()
  feedHeavy(activity, 'aborted')
  expect(activity.snapshot(2_000).active).toBe(true)
  activity.end('aborted', 2_100)
  expect(activity.snapshot(2_100).active).toBe(false)

  feedHeavy(activity, 'old-clock', 10_000)
  expect(activity.snapshot(12_000).active).toBe(true)
  expect(activity.snapshot(9_000)).toEqual({ active: false, estimatedTps: 0, measuredTps: null, source: 'idle' })
  activity.complete('old-clock', 9_000, 9_001)
  expect(activity.snapshot(9_001).measuredTps).toBeNull()
})

test('bounded samples reject invalid inputs and keep all public statistics finite', () => {
  const activity = new BurnActivity()
  for (let index = 0; index < BURN_POLICY.maxRequests + 20; index += 1) {
    activity.start(`request-${index}`, index * BURN_POLICY.bucketMs)
    activity.chunk(`request-${index}`, Number.MAX_VALUE, index * BURN_POLICY.bucketMs)
  }
  activity.chunk('invalid', Number.NaN, 50_000)
  const snapshot = activity.snapshot(50_000)
  expect(Number.isFinite(snapshot.estimatedTps)).toBe(true)
  expect(snapshot.estimatedTps >= 0).toBe(true)
  expect(activity.snapshot(Number.NaN).estimatedTps).toBe(snapshot.estimatedTps)
})

test('static SVG keeps the source mark and rooted flames inside the fixed allocation', () => {
  const svg = burnMarkSvg(1_000)
  const animation = burnAnimationSvg(1_000)
  expect(svg).toContain(`width="${BURN_WIDTH}" height="${BURN_HEIGHT}"`)
  expect(svg).toContain(`viewBox="0 0 ${BURN_WIDTH} ${BURN_HEIGHT}"`)
  expect(svg).toContain('fill="#FF5914"')
  expect(svg).not.toContain('NaN')
  expect(svg).not.toContain('Infinity')
  for (const [x, y] of BURN_GEOMETRY.flameRoots) {
    expect(x - BURN_GEOMETRY.dotRadius >= 0).toBe(true)
    expect(x + BURN_GEOMETRY.dotRadius <= BURN_WIDTH).toBe(true)
    expect(y - BURN_GEOMETRY.dotRadius >= 0).toBe(true)
    expect(y + BURN_GEOMETRY.dotRadius <= BURN_HEIGHT).toBe(true)
    const root = `class="flame" cx="${Number(x.toFixed(3))}" cy="${Number(y.toFixed(3))}"`
    expect(svg).toContain(root)
    expect(animation).toContain(root)
  }
  expect((svg.match(/class="mark"/g) ?? []).length).toBe(1)
})

test('strength endpoints and reduced motion keep a crisp static mark', () => {
  const cool = burnMarkSvg(500, 0)
  expect(cool).not.toContain('class="flame"')
  expect(cool).not.toContain('class="heat"')
  expect((cool.match(/<circle/g) ?? []).length).toBe(13)

  const reduced = burnMarkSvg(500, 1, true)
  expect(reduced).not.toContain('class="flame"')
  expect((reduced.match(/class="heat"/g) ?? []).length).toBe(13)
})

test('raw frames use the same bounded geometry and change continuously', () => {
  const first = burnFrame(1_000)
  const next = burnFrame(1_083)
  expect(first.width).toBe(BURN_WIDTH)
  expect(first.height).toBe(BURN_HEIGHT)
  const firstBytes = decodeBase64(first.rgba)
  const nextBytes = decodeBase64(next.rgba)
  expect(firstBytes.length).toBe(BURN_WIDTH * BURN_HEIGHT * 4)
  expect(nextBytes.length).toBe(firstBytes.length)

  let difference = 0
  for (let index = 0; index < firstBytes.length; index += 1) difference += Math.abs(firstBytes[index]! - nextBytes[index]!)
  expect(difference > 0).toBe(true)
  expect(difference < 20_000).toBe(true)
})

test('animated SVG is finite, compact, seeded by elapsed time, and has a reduced-motion fallback', () => {
  const entering = burnAnimationSvg(0)
  const active = burnAnimationSvg(1_000)
  const shifted = burnAnimationSvg(2_000)
  const cooling = burnAnimationSvg(1_000, true)
  const debug = burnAnimationSvg(1_000, false, 15_000)
  const minimum = burnAnimationSvg(1_000, false, 1)
  const maximum = burnAnimationSvg(1_000, false, 20_000)
  const invalid = burnAnimationSvg(1_000, false, Number.NaN)
  expect(active).toContain('dur="8s"')
  expect(cooling).toContain('dur="0.8s"')
  expect(debug).toContain('dur="15s"')
  expect(minimum).toContain('dur="0.8s"')
  expect(maximum).toContain('dur="15s"')
  expect(invalid).toContain('dur="8s"')
  expect(active).toContain('repeatCount="1"')
  expect(active).not.toContain('indefinite')
  expect(active).toContain('prefers-reduced-motion:reduce')
  expect(active).toContain('<g class="reduced"')
  expect(active).toContain('class="reduced" fill="#ffd08a" fill-opacity="0.195"><animate attributeName="fill-opacity"')
  expect(debug).toContain('0.195;0.195')
  expect(debug).toContain(';0" calcMode="discrete" dur="15s"')
  expect(entering).toContain('fill-opacity="0"><animate')
  expect(active.length < 200_000).toBe(true)
  expect(debug.length < 200_000).toBe(true)
  expect(cooling.length < active.length).toBe(true)
  expect(shifted).not.toBe(active)
})

test('taller flames show through the holes at lower contrast and leave the foreground opaque', () => {
  const holes = [
    [1, 20],
    [4, 20],
    [7, 20],
    [7, 26],
    [10, 28],
    [12, 17],
  ] as const
  let throughHoles = false
  let aboveMark = false
  for (let time = 0; time <= 15000; time += 83) {
    const pixels = decodeBase64(burnFrame(time).rgba)
    for (const [x, y] of holes) {
      const offset = (y * BURN_WIDTH + x) * 4
      const alpha = pixels[offset + 3]!
      expect(alpha <= 115).toBe(true)
      if (alpha > 5) {
        throughHoles = true
        expect(pixels[offset]! < 230).toBe(true)
      }
    }
    for (let y = 0; y < BURN_GEOMETRY.markTop - BURN_GEOMETRY.dotPitch; y += 1) {
      for (let x = 0; x < BURN_WIDTH; x += 1) {
        if (pixels[(y * BURN_WIDTH + x) * 4 + 3]! > 20) aboveMark = true
      }
    }
    const root = BURN_GEOMETRY.flameRoots[0]!
    expect(pixels[(Math.floor(root[1]) * BURN_WIDTH + Math.floor(root[0])) * 4 + 3]).toBe(255)
  }
  expect(throughHoles).toBe(true)
  expect(aboveMark).toBe(true)
  for (const svg of [burnMarkSvg(1000), burnAnimationSvg(1000), burnAnimationSvg(1000, true)]) {
    expect(svg).toContain('mask="url(#burn-flame-space)"')
    expect(svg).toContain('fill="#737373"')
    expect(svg.indexOf('class="flame"') < svg.indexOf('class="mark"')).toBe(true)
  }
})

test('flame tongues vary across all five columns above the mark', () => {
  const columns = new Set<string>()
  const silhouettes = new Set<string>()
  let widest = 0
  for (let time = 0; time < 15000; time += 83) {
    const visible = new Set<string>()
    for (const match of burnMarkSvg(time).matchAll(/class="flame" cx="([\d.]+)" cy="([\d.]+)"[^>]*fill-opacity="([\d.]+)"/g)) {
      if (Number(match[2]) >= BURN_GEOMETRY.markTop || Number(match[3]) <= 0.2) continue
      columns.add(match[1]!)
      visible.add(match[1]!)
    }
    widest = Math.max(widest, visible.size)
    silhouettes.add([...visible].sort().join(','))
  }
  expect(columns.size).toBe(5)
  expect(widest >= 3).toBe(true)
  expect(silhouettes.size >= 8).toBe(true)
  expect(burnMarkSvg(3456)).toBe(burnMarkSvg(3456))
})

test('warm ripples cross the foreground with a clear gradient and varied paths', () => {
  const patterns = new Set<string>()
  let strongest = 0
  let gradientFrames = 0
  const rows = new Set<string>()
  for (let time = 0; time < 15000; time += 83) {
    const dots = [...burnMarkSvg(time).matchAll(/class="heat" cx="([\d.]+)" cy="([\d.]+)"[^>]*fill-opacity="([\d.]+)"/g)]
    const bright = dots.filter((dot) => Number(dot[3]) > 0.4)
    if (bright.length > 0 && dots.some((dot) => Number(dot[3]) < 0.15)) gradientFrames += 1
    for (const dot of bright) rows.add(dot[2]!)
    for (const dot of dots) strongest = Math.max(strongest, Number(dot[3]))
    patterns.add(bright.map((dot) => `${dot[1]},${dot[2]}`).join(';'))
    expect(dots.every((dot) => Number(dot[3]) <= 0.85)).toBe(true)
  }
  expect(strongest > 0.65).toBe(true)
  expect(rows.size).toBe(5)
  expect(patterns.size > 12).toBe(true)
  expect(gradientFrames > 20).toBe(true)
})

test('Ember catch warms the base, carries heat upward, then grows complete flame dots', () => {
  const heat = (time: number) => [...burnMarkSvg(time).matchAll(/class="heat" cx="([\d.]+)" cy="([\d.]+)"[^>]*fill-opacity="([\d.]+)"/g)]
  const center = (time: number) => {
    const dots = heat(time)
    return dots.reduce((sum, dot) => sum + Number(dot[2]) * Number(dot[3]), 0) / dots.reduce((sum, dot) => sum + Number(dot[3]), 0)
  }
  expect(burnMarkSvg(0)).not.toContain('class="flame"')
  expect(burnMarkSvg(150)).not.toContain('class="flame"')
  expect(heat(150).length > 0).toBe(true)
  expect(center(150) > center(400)).toBe(true)
  const flameTop = (time: number) =>
    Math.min(...[...burnMarkSvg(time).matchAll(/class="flame" cx="[\d.]+" cy="([\d.]+)"/g)].map((dot) => Number(dot[1])))
  expect(flameTop(BURN_IGNITION.duration) < flameTop(300)).toBe(true)
  expect(burnMarkSvg(8000)).toContain('class="flame"')
  expect(burnMarkSvg(150, 1, true)).not.toContain('class="flame"')
  expect(heat(150).length < 13).toBe(true)
  for (const match of burnMarkSvg(500).matchAll(/class="flame" cx="([\d.]+)" cy="([\d.]+)" r="([\d.]+)"/g)) {
    expect(Number(match[1]) - Number(match[3]) >= 0).toBe(true)
    expect(Number(match[1]) + Number(match[3]) <= BURN_WIDTH).toBe(true)
    expect(Number(match[2]) - Number(match[3]) >= 0).toBe(true)
    expect(Number(match[2]) + Number(match[3]) <= BURN_HEIGHT).toBe(true)
  }
})

test('cooling scales the current intensity and cannot rekindle an expired lease', () => {
  const full = burnAnimationSvg(4000, true, 8000, 1)
  const quarter = burnAnimationSvg(4000, true, 8000, 0.25)
  const expired = burnAnimationSvg(4000, true, 8000, 0)
  const values = (svg: string) =>
    [...svg.matchAll(/<animate attributeName="fill-opacity" values="([^"]+)"/g)].map((match) => match[1]!.split(';').map(Number))
  const fullValues = values(full)
  const quarterValues = values(quarter)
  const expiredValues = values(expired)
  for (let index = 0; index < fullValues.length; index++) {
    expect(quarterValues[index]![0]! <= fullValues[index]![0]! * 0.25 + 0.001).toBe(true)
    expect(quarterValues[index]!.every((value, frame, all) => frame === 0 || value <= all[frame - 1]!)).toBe(true)
    expect(expiredValues[index]!.every((value) => value === 0)).toBe(true)
  }
})
