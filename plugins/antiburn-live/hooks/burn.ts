import { bezier } from './usage-presentation'

export const BURN_ANIMATION_ENABLED = false

export const BURN_POLICY = {
  windowMs: 5_000,
  bucketMs: 500,
  minObservationMs: 2_000,
  entryTps: 60,
  exitTps: 36,
  holdMs: 8_000,
  staleMs: 8_000,
  maxRequests: 64,
  maxBuckets: 12,
  maxCalibrationRequests: 8,
  defaultCharsPerToken: 4,
  minCharsPerToken: 1,
  maxCharsPerToken: 12,
  maxCharsPerChunk: 1_000_000,
  maxCharsPerRequest: 100_000_000,
} as const

export const BURN_IGNITION = {
  emberEnd: 180,
  rippleStart: 100,
  rippleEnd: 500,
  flameStart: 220,
  duration: 620,
} as const

export type BurnSource = 'idle' | 'stream-estimate' | 'completed-request-average'

export type BurnSnapshot = {
  active: boolean
  estimatedTps: number
  measuredTps: number | null
  source: BurnSource
}

type RequestActivity = {
  startedAt: number
  lastAt: number
  chars: number
}

type ActivityBucket = {
  startedAt: number
  firstAt: number
  lastAt: number
  chars: number
}

type CompletionMeasurement = {
  at: number
  durationMs: number
  tps: number
}

function finiteRate(value: number): number {
  return Number.isFinite(value) && value >= 0 ? value : 0
}

function validId(id: string): boolean {
  return typeof id === 'string' && id.length > 0
}

export class BurnActivity {
  private requests = new Map<string, RequestActivity>()
  private buckets: ActivityBucket[] = []
  private calibrations: number[] = []
  private completion: CompletionMeasurement | null = null
  private lastClock: number | null = null
  private lastChunkAt: number | null = null
  private activeUntil = Number.NEGATIVE_INFINITY
  private activeSource: BurnSource = 'idle'

  reset(): void {
    this.clearState()
    this.lastClock = null
  }

  start(id: string, now: number): void {
    if (!this.acceptTime(now) || !validId(id)) return
    if (this.requests.has(id)) return
    while (this.requests.size >= BURN_POLICY.maxRequests) {
      const oldest = this.requests.keys().next().value
      if (oldest === undefined) break
      this.requests.delete(oldest)
    }
    this.requests.set(id, { startedAt: now, lastAt: now, chars: 0 })
  }

  chunk(id: string, chars: number, now: number): void {
    if (!this.acceptTime(now) || !validId(id) || !Number.isFinite(chars) || chars <= 0) return
    let request = this.requests.get(id)
    if (request === undefined) {
      this.start(id, now)
      request = this.requests.get(id)
      if (request === undefined) return
    }

    const amount = Math.min(BURN_POLICY.maxCharsPerChunk, Math.floor(chars))
    if (amount <= 0) return
    request.chars = Math.min(BURN_POLICY.maxCharsPerRequest, request.chars + amount)
    request.lastAt = now
    this.lastChunkAt = now

    const bucketStart = Math.floor(now / BURN_POLICY.bucketMs) * BURN_POLICY.bucketMs
    const bucket = this.buckets[this.buckets.length - 1]
    if (bucket?.startedAt === bucketStart) {
      bucket.chars = Math.min(BURN_POLICY.maxCharsPerRequest, bucket.chars + amount)
      bucket.lastAt = now
    } else {
      this.buckets.push({ startedAt: bucketStart, firstAt: now, lastAt: now, chars: amount })
    }
    this.prune(now)
  }

  complete(id: string, outputTokens: number | null, now: number): void {
    if (!this.acceptTime(now) || !validId(id)) return
    const request = this.requests.get(id)
    if (request === undefined) return
    this.requests.delete(id)

    const durationMs = now - request.startedAt
    if (outputTokens !== null && Number.isFinite(outputTokens) && outputTokens >= 0 && durationMs > 0) {
      const tps = finiteRate(outputTokens / (durationMs / 1_000))
      this.completion = { at: now, durationMs, tps }
      if (outputTokens > 0 && request.chars > 0) {
        const charsPerToken = Math.min(BURN_POLICY.maxCharsPerToken, Math.max(BURN_POLICY.minCharsPerToken, request.chars / outputTokens))
        this.calibrations.push(charsPerToken)
        if (this.calibrations.length > BURN_POLICY.maxCalibrationRequests) this.calibrations.shift()
      }
      if (this.requests.size > 0 && durationMs >= BURN_POLICY.minObservationMs && tps >= BURN_POLICY.entryTps) {
        this.activeUntil = now + BURN_POLICY.holdMs
        this.activeSource = 'completed-request-average'
      }
    }

    if (this.requests.size === 0) this.clearEpisode()
  }

  end(id: string, now: number): void {
    if (!this.acceptTime(now) || !validId(id)) return
    this.requests.delete(id)
    if (this.requests.size === 0) this.clearEpisode()
  }

  snapshot(now: number): BurnSnapshot {
    if (!this.acceptTime(now)) return this.snapshotAt(this.lastClock ?? 0)
    return this.snapshotAt(now)
  }

  private snapshotAt(now: number): BurnSnapshot {
    this.prune(now)
    const charsPerToken =
      this.calibrations.length === 0
        ? BURN_POLICY.defaultCharsPerToken
        : this.calibrations.reduce((sum, value) => sum + value, 0) / this.calibrations.length
    const first = this.buckets[0]
    const recentChars = this.buckets.reduce((sum, bucket) => sum + bucket.chars, 0)
    const observedMs = first === undefined ? 0 : Math.max(0, now - first.firstAt)
    const estimateDurationMs = Math.max(BURN_POLICY.minObservationMs, observedMs)
    const estimatedTps = finiteRate((recentChars / charsPerToken) * (1_000 / estimateDurationMs))
    const measurement = this.completion !== null && now - this.completion.at <= BURN_POLICY.staleMs ? this.completion : null

    const estimateEnters = observedMs >= BURN_POLICY.minObservationMs && estimatedTps >= BURN_POLICY.entryTps
    const measurementEnters =
      measurement !== null && measurement.durationMs >= BURN_POLICY.minObservationMs && measurement.tps >= BURN_POLICY.entryTps
    const estimateSustains = observedMs >= BURN_POLICY.minObservationMs && estimatedTps >= BURN_POLICY.exitTps
    const measurementSustains =
      measurement !== null && measurement.durationMs >= BURN_POLICY.minObservationMs && measurement.tps >= BURN_POLICY.exitTps

    if (this.requests.size > 0 && (measurementEnters || estimateEnters)) {
      this.activeUntil = now + BURN_POLICY.holdMs
      this.activeSource = measurementEnters ? 'completed-request-average' : 'stream-estimate'
    } else if (this.requests.size > 0 && this.activeSource !== 'idle' && (measurementSustains || estimateSustains)) {
      this.activeUntil = now + BURN_POLICY.holdMs
    }

    const latestSignalAt = Math.max(this.lastChunkAt ?? Number.NEGATIVE_INFINITY, measurement?.at ?? Number.NEGATIVE_INFINITY)
    const active =
      this.requests.size > 0 && this.activeSource !== 'idle' && now <= this.activeUntil && now - latestSignalAt <= BURN_POLICY.staleMs

    if (!active && (this.requests.size === 0 || now - latestSignalAt > BURN_POLICY.staleMs)) this.clearActiveLatch()

    const source: BurnSource = active
      ? this.activeSource
      : measurement !== null && (this.lastChunkAt === null || measurement.at >= this.lastChunkAt)
        ? 'completed-request-average'
        : estimatedTps > 0
          ? 'stream-estimate'
          : 'idle'
    return { active, estimatedTps, measuredTps: measurement?.tps ?? null, source }
  }

  private acceptTime(now: number): boolean {
    if (!Number.isFinite(now)) return false
    if (this.lastClock !== null && now < this.lastClock) this.clearState()
    this.lastClock = now
    this.prune(now)
    return true
  }

  private prune(now: number): void {
    const cutoff = now - BURN_POLICY.windowMs
    while (this.buckets.length > 0 && this.buckets[0]!.lastAt < cutoff) this.buckets.shift()
    while (this.buckets.length > BURN_POLICY.maxBuckets) this.buckets.shift()
    if (this.completion !== null && now - this.completion.at > BURN_POLICY.staleMs) this.completion = null
  }

  private clearActiveLatch(): void {
    this.activeUntil = Number.NEGATIVE_INFINITY
    this.activeSource = 'idle'
  }

  private clearEpisode(): void {
    this.buckets = []
    this.lastChunkAt = null
    this.clearActiveLatch()
  }

  private clearState(): void {
    this.requests.clear()
    this.buckets = []
    this.calibrations = []
    this.completion = null
    this.lastChunkAt = null
    this.clearActiveLatch()
  }
}

export const BURN_WIDTH = 14
export const BURN_HEIGHT = 30

export const BURN_MARK_TOP = 16
const MARK_TOP = BURN_MARK_TOP
const MARK_SOURCE_X = 6.2
const MARK_SOURCE_Y = 5.9
const MARK_SOURCE_SIZE = 32.4
const MARK_SCALE = BURN_WIDTH / MARK_SOURCE_SIZE
const DOT_RADIUS = 3.2 * MARK_SCALE
const DOT_PITCH = 6.4 * MARK_SCALE
const BRAND = '#FF5914'
const FLAME = '#cf4f17'
const HOLE_TRANSMISSION = 115 / 255
const HIGHLIGHT = '#ffd08a'
const MARK_DOTS = [
  [16, 9.3],
  [22.4, 9.3],
  [28.8, 9.3],
  [35.2, 15.7],
  [16, 22.1],
  [22.4, 22.1],
  [28.8, 22.1],
  [35.2, 22.1],
  [9.6, 28.5],
  [35.2, 28.5],
  [16, 34.9],
  [22.4, 34.9],
  [35.2, 34.9],
] as const

const markPoint = ([x, y]: readonly [number, number]): readonly [number, number] => [
  (x - MARK_SOURCE_X) * MARK_SCALE,
  MARK_TOP + (y - MARK_SOURCE_Y) * MARK_SCALE,
]

const MARK_POINTS = MARK_DOTS.map(markPoint)
const FLAME_ROOTS = [markPoint([16, 34.9]), markPoint([35.2, 34.9])] as const

// Dim flames in the counters and open edges to keep the foreground letter clear.
const PROTECTED_SPACES = [
  [6.2, 5.9, 6.6, 19.2],
  [12.8, 12.5, 19.2, 6.4],
  [12.8, 25.3, 19.2, 6.4],
  [25.6, 31.7, 6.4, 6.6],
  [32, 5.9, 6.6, 6.6],
].map(([x, y, width, height]) => ({
  x: (x! - MARK_SOURCE_X) * MARK_SCALE,
  y: MARK_TOP + (y! - MARK_SOURCE_Y) * MARK_SCALE,
  width: width! * MARK_SCALE,
  height: height! * MARK_SCALE,
}))

function protectedSpace(x: number, y: number): boolean {
  return PROTECTED_SPACES.some((space) => x >= space.x && x < space.x + space.width && y >= space.y && y < space.y + space.height)
}

function flameMask(): string {
  return (
    '<defs><mask id="burn-flame-space" maskUnits="userSpaceOnUse" x="0" y="0" width="14" height="30">' +
    '<rect width="14" height="30" fill="white"/>' +
    PROTECTED_SPACES.map(
      (space) =>
        `<rect x="${number(space.x)}" y="${number(space.y)}" width="${number(space.width)}" height="${number(space.height)}" fill="#737373"/>`,
    ).join('') +
    '</mask></defs>'
  )
}

export const BURN_GEOMETRY = {
  width: BURN_WIDTH,
  height: BURN_HEIGHT,
  markTop: MARK_TOP,
  markSize: BURN_WIDTH,
  dotRadius: DOT_RADIUS,
  dotPitch: DOT_PITCH,
  flameRoots: FLAME_ROOTS,
} as const

type DotSample = {
  x: number
  y: number
  alpha: number
}

type Scene = {
  flames: DotSample[]
  highlights: DotSample[]
}

type RippleEpisode = {
  start: number
  duration: number
  bend: number
  tilt: number
  width: number
  strength: number
}

function buildRippleCycle(): { episodes: RippleEpisode[]; duration: number } {
  let seed = 7_143
  let cursor = 200
  const episodes: RippleEpisode[] = []
  const random = () => {
    seed = (seed * 1_664_525 + 1_013_904_223) >>> 0
    return seed / 4_294_967_296
  }
  for (let index = 0; index < 16; index += 1) {
    const duration = 1_100 + random() * 700
    episodes.push({
      start: cursor,
      duration,
      bend: random() * Math.PI * 2,
      tilt: (random() - 0.5) * 1.1,
      width: 2.6 + random() * 1.2,
      strength: 0.8 + random() * 0.2,
    })
    cursor += duration + 350 + random() * 650
  }
  return { episodes, duration: cursor }
}

const RIPPLE_CYCLE = buildRippleCycle()

function clampStrength(value: number): number {
  if (!Number.isFinite(value)) return 0
  return Math.min(1, Math.max(0, value))
}

function elapsed(value: number): number {
  return Number.isFinite(value) && value > 0 ? value : 0
}

function rippleAt(elapsedMs: number): RippleEpisode | null {
  const cycleAt = elapsed(elapsedMs) % RIPPLE_CYCLE.duration
  for (const episode of RIPPLE_CYCLE.episodes) {
    if (cycleAt >= episode.start && cycleAt <= episode.start + episode.duration) return episode
  }
  return null
}

const FLAME_POINTS: readonly (readonly [number, number])[] = (() => {
  const points: [number, number][] = []
  const firstX = FLAME_ROOTS[0][0] - DOT_PITCH
  const baseY = FLAME_ROOTS[0][1]
  for (let y = baseY; y - DOT_RADIUS >= 0; y -= DOT_PITCH) {
    for (let x = firstX; x + DOT_RADIUS <= BURN_WIDTH; x += DOT_PITCH) {
      if (x - DOT_RADIUS >= 0 && y >= MARK_TOP - DOT_PITCH * 2 + DOT_RADIUS) points.push([x, y])
    }
  }
  return points
})()

function flameNoise(time: number, seed: number): number {
  const hash = (step: number) => {
    let value = Math.imul(step ^ seed, 0x45d9f3b)
    value = Math.imul(value ^ (value >>> 16), 0x45d9f3b)
    return ((value ^ (value >>> 16)) >>> 0) / 4_294_967_295
  }
  const step = Math.floor(time)
  const fraction = time - step
  const blend = fraction * fraction * (3 - 2 * fraction)
  return (hash(step) * (1 - blend) + hash(step + 1) * blend) * 2 - 1
}

function sceneAt(elapsedMs: number, rawStrength: number, reduced: boolean, ignitionMs = elapsedMs): Scene {
  const strength = clampStrength(rawStrength)
  const time = elapsed(elapsedMs) / 1_000
  const ignition = elapsed(ignitionMs)
  const flameProgress = bezier(
    clampStrength((ignition - BURN_IGNITION.flameStart) / (BURN_IGNITION.duration - BURN_IGNITION.flameStart)),
    0.32,
    0.72,
    0,
    1,
  )
  const steadyProgress = clampStrength((ignition - BURN_IGNITION.rippleEnd) / (BURN_IGNITION.duration - BURN_IGNITION.rippleEnd))
  const flames = FLAME_POINTS.map(([x, y]) => {
    if (reduced || strength === 0 || flameProgress === 0) return { x, y, alpha: 0 }
    const up = FLAME_ROOTS[0][1] - y
    let density = 0
    for (let index = 0; index < FLAME_ROOTS.length; index += 1) {
      const seed = 7_143 + index * 101
      const height = DOT_PITCH + (18 + 3 * flameNoise(time * 1.3, seed) - DOT_PITCH) * flameProgress
      const rise = up / height
      if (rise >= 1) continue
      const bend = 3.6 * flameNoise(time * 1.7, seed + 11) * rise
      const wave = 1.2 * flameNoise(time * 2.1 - rise * 2.4, seed + 23) * rise
      const center = FLAME_ROOTS[index]![0] + bend + wave
      const width = (3.4 + 0.8 * flameNoise(time * 1.5, seed + 37)) * Math.pow(Math.max(0, 1 - rise), 0.45)
      if (width > 0) density = Math.max(density, Math.max(0, 1 - Math.abs(x - center) / width))
    }
    const flicker = 0.88 + 0.12 * flameNoise(time * 3.3 - up * 0.21, Math.round(x * 41) + 313)
    const alpha = Math.min(1, density * 2.2) * flicker * strength * flameProgress
    return { x, y, alpha: clampStrength(alpha) }
  })

  const episode = reduced ? null : rippleAt(elapsedMs)
  const highlights = MARK_POINTS.map(([x, y]) => {
    if (reduced) return { x, y, alpha: strength * 0.195 }
    if (strength === 0) return { x, y, alpha: 0 }
    const ember =
      y >= MARK_TOP + 9
        ? 0.45 * clampStrength(ignition / BURN_IGNITION.emberEnd) * (1 - clampStrength((ignition - BURN_IGNITION.emberEnd) / 180))
        : 0
    const catchProgress = clampStrength((ignition - BURN_IGNITION.rippleStart) / (BURN_IGNITION.rippleEnd - BURN_IGNITION.rippleStart))
    const catchCrest = MARK_TOP + 18 - catchProgress * 22 + (x - BURN_WIDTH / 2) * 0.15
    const catchHeat = Math.exp(-Math.pow((y - catchCrest) / 2.8, 2)) * Math.pow(Math.sin(Math.PI * catchProgress), 0.45) * 0.85
    const ignitionHeat = Math.max(ember, catchHeat)
    if (episode === null) return { x, y, alpha: clampStrength(strength * ignitionHeat) }
    const progress = ((elapsed(elapsedMs) % RIPPLE_CYCLE.duration) - episode.start) / episode.duration
    const wind = flameNoise(time * 1.7, 7_154) * (1 - x / BURN_WIDTH) + flameNoise(time * 1.7, 7_255) * (x / BURN_WIDTH)
    const crest =
      MARK_TOP +
      18 -
      progress * 22 +
      (x - BURN_WIDTH / 2) * episode.tilt +
      1.4 * wind +
      0.9 * Math.sin((x - BURN_WIDTH / 2) * 0.65 + progress * 3 + episode.bend)
    const heat = Math.exp(-Math.pow((y - crest) / episode.width, 2))
    const envelope = Math.pow(Math.sin(Math.PI * progress), 0.45)
    return { x, y, alpha: clampStrength(strength * Math.max(ignitionHeat, heat * envelope * episode.strength * 0.85 * steadyProgress)) }
  })
  return { flames, highlights }
}

function number(value: number): string {
  return Number(value.toFixed(3)).toString()
}

function markCircles(fill = BRAND): string {
  return MARK_POINTS.map(([x, y]) => `<circle cx="${number(x)}" cy="${number(y)}" r="${number(DOT_RADIUS)}" fill="${fill}"/>`).join('')
}

function staticSceneSvg(scene: Scene): string {
  const flames = scene.flames
    .filter((dot) => dot.alpha > 0.001)
    .map(
      (dot) =>
        `<circle class="flame" cx="${number(dot.x)}" cy="${number(dot.y)}" r="${number(DOT_RADIUS)}" fill="${FLAME}" fill-opacity="${number(dot.alpha)}"/>`,
    )
    .join('')
  const highlights = scene.highlights
    .filter((dot) => dot.alpha > 0.001)
    .map(
      (dot) =>
        `<circle class="heat" cx="${number(dot.x)}" cy="${number(dot.y)}" r="${number(DOT_RADIUS)}" fill="${HIGHLIGHT}" fill-opacity="${number(dot.alpha)}"/>`,
    )
    .join('')
  return flameMask() + `<g mask="url(#burn-flame-space)">${flames}</g>` + `<g class="mark">${markCircles()}${highlights}</g>`
}

export function burnMarkSvg(elapsedMs: number, strength = 1, reduced = false): string {
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${BURN_WIDTH}" height="${BURN_HEIGHT}" viewBox="0 0 ${BURN_WIDTH} ${BURN_HEIGHT}" overflow="hidden">` +
    staticSceneSvg(sceneAt(elapsedMs, strength, reduced)) +
    '</svg>'
  )
}

function animatedCircle(
  className: string,
  point: readonly [number, number],
  fill: string,
  values: readonly number[],
  durationMs: number,
): string {
  const opacity = values.map(number).join(';')
  return (
    `<circle class="${className}" cx="${number(point[0])}" cy="${number(point[1])}" r="${number(DOT_RADIUS)}" fill="${fill}" fill-opacity="${number(values[0] ?? 0)}">` +
    `<animate attributeName="fill-opacity" values="${opacity}" calcMode="discrete" dur="${number(durationMs / 1_000)}s" begin="0s" repeatCount="1" fill="freeze"/>` +
    '</circle>'
  )
}

export function burnAnimationSvg(elapsedMs: number, cooling = false, leaseMs = 8_000, coolingStrength = 1): string {
  const boundedLeaseMs = Number.isFinite(leaseMs) ? Math.min(15_000, Math.max(800, Math.round(leaseMs))) : 8_000
  const durationMs = cooling ? 800 : boundedLeaseMs
  const frameCount = Math.ceil(durationMs / (1_000 / 12)) + 1
  const strengths = Array.from({ length: frameCount }, (_, index) => {
    const progress = index / (frameCount - 1)
    const leaseStrength = Math.min(1, Math.max(0, (durationMs * (1 - progress)) / 800))
    const entranceStrength = Math.min(1, (elapsed(elapsedMs) + progress * durationMs) / 220)
    return cooling ? clampStrength(coolingStrength) * (1 - progress) : Math.min(leaseStrength, entranceStrength)
  })
  const coolingScene = cooling ? sceneAt(elapsedMs, clampStrength(coolingStrength), false) : undefined
  const scenes = strengths.map((strength, index) => {
    if (coolingScene) {
      const fade = 1 - index / (frameCount - 1)
      return {
        flames: coolingScene.flames.map((dot) => ({ ...dot, alpha: dot.alpha * fade })),
        highlights: coolingScene.highlights.map((dot) => ({ ...dot, alpha: dot.alpha * fade })),
      }
    }
    return sceneAt(elapsedMs + (index / (frameCount - 1)) * durationMs, strength, false)
  })
  const flames = FLAME_POINTS.map((point, index) =>
    animatedCircle(
      'flame',
      point,
      FLAME,
      scenes.map((scene) => scene.flames[index]!.alpha),
      durationMs,
    ),
  ).join('')
  const highlights = MARK_POINTS.map((point, index) =>
    animatedCircle(
      'heat',
      point,
      HIGHLIGHT,
      scenes.map((scene) => scene.highlights[index]!.alpha),
      durationMs,
    ),
  ).join('')
  const reducedOpacity = strengths.map((strength) => strength * 0.195)
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${BURN_WIDTH}" height="${BURN_HEIGHT}" viewBox="0 0 ${BURN_WIDTH} ${BURN_HEIGHT}" overflow="hidden">` +
    '<style>.reduced{display:none}@media(prefers-reduced-motion:reduce){.motion{display:none}.reduced{display:inline}}</style>' +
    flameMask() +
    `<g class="motion" mask="url(#burn-flame-space)">${flames}</g><g class="mark">${markCircles()}<g class="motion">${highlights}</g>` +
    `<g class="reduced" fill="${HIGHLIGHT}" fill-opacity="${number(reducedOpacity[0] ?? 0)}">` +
    `<animate attributeName="fill-opacity" values="${reducedOpacity.map(number).join(';')}" calcMode="discrete" dur="${number(durationMs / 1_000)}s" begin="0s" repeatCount="1" fill="freeze"/>` +
    MARK_POINTS.map(([x, y]) => `<circle cx="${number(x)}" cy="${number(y)}" r="${number(DOT_RADIUS)}"/>`).join('') +
    '</g></g>' +
    '</svg>'
  )
}

const BASE64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'

function base64(bytes: Uint8Array): string {
  let result = ''
  for (let index = 0; index < bytes.length; index += 3) {
    const a = bytes[index]!
    const b = bytes[index + 1]
    const c = bytes[index + 2]
    const value = (a << 16) | ((b ?? 0) << 8) | (c ?? 0)
    result += BASE64[(value >>> 18) & 63]
    result += BASE64[(value >>> 12) & 63]
    result += b === undefined ? '=' : BASE64[(value >>> 6) & 63]
    result += c === undefined ? '=' : BASE64[value & 63]
  }
  return result
}

function rgb(hex: string): readonly [number, number, number] {
  return [Number.parseInt(hex.slice(1, 3), 16), Number.parseInt(hex.slice(3, 5), 16), Number.parseInt(hex.slice(5, 7), 16)]
}

function renderRgba(scene: Scene): Uint8Array {
  const supersample = 4
  const width = BURN_WIDTH * supersample
  const height = BURN_HEIGHT * supersample
  const pixels = new Float64Array(width * height * 4)
  const paint = (x: number, y: number, radius: number, color: readonly [number, number, number], alpha: number, mask = false) => {
    if (alpha <= 0) return
    const left = Math.max(0, Math.floor((x - radius) * supersample))
    const right = Math.min(width - 1, Math.ceil((x + radius) * supersample))
    const top = Math.max(0, Math.floor((y - radius) * supersample))
    const bottom = Math.min(height - 1, Math.ceil((y + radius) * supersample))
    const radiusSquared = radius * radius
    for (let py = top; py <= bottom; py += 1) {
      for (let px = left; px <= right; px += 1) {
        const sampleX = (px + 0.5) / supersample
        const sampleY = (py + 0.5) / supersample
        if ((sampleX - x) ** 2 + (sampleY - y) ** 2 > radiusSquared) continue
        const offset = (py * width + px) * 4
        const coverage = mask && protectedSpace(sampleX, sampleY) ? alpha * HOLE_TRANSMISSION : alpha
        const behind = 1 - coverage
        pixels[offset] = color[0] * coverage + pixels[offset]! * behind
        pixels[offset + 1] = color[1] * coverage + pixels[offset + 1]! * behind
        pixels[offset + 2] = color[2] * coverage + pixels[offset + 2]! * behind
        pixels[offset + 3] = coverage + pixels[offset + 3]! * behind
      }
    }
  }

  const flame = rgb(FLAME)
  const brand = rgb(BRAND)
  const highlight = rgb(HIGHLIGHT)
  for (const dot of scene.flames) paint(dot.x, dot.y, DOT_RADIUS, flame, dot.alpha, true)
  for (const [x, y] of MARK_POINTS) paint(x, y, DOT_RADIUS, brand, 1)
  for (const dot of scene.highlights) paint(dot.x, dot.y, DOT_RADIUS, highlight, dot.alpha)

  const output = new Uint8Array(BURN_WIDTH * BURN_HEIGHT * 4)
  for (let y = 0; y < BURN_HEIGHT; y += 1) {
    for (let x = 0; x < BURN_WIDTH; x += 1) {
      let red = 0
      let green = 0
      let blue = 0
      let alpha = 0
      for (let sy = 0; sy < supersample; sy += 1) {
        for (let sx = 0; sx < supersample; sx += 1) {
          const source = ((y * supersample + sy) * width + x * supersample + sx) * 4
          red += pixels[source]!
          green += pixels[source + 1]!
          blue += pixels[source + 2]!
          alpha += pixels[source + 3]!
        }
      }
      const samples = supersample * supersample
      const target = (y * BURN_WIDTH + x) * 4
      output[target] = alpha === 0 ? 0 : Math.round(red / alpha)
      output[target + 1] = alpha === 0 ? 0 : Math.round(green / alpha)
      output[target + 2] = alpha === 0 ? 0 : Math.round(blue / alpha)
      output[target + 3] = Math.round((alpha / samples) * 255)
    }
  }
  return output
}

export function burnFrame(elapsedMs: number, strength = 1, reduced = false): { rgba: string; width: number; height: number } {
  return {
    rgba: base64(renderRgba(sceneAt(elapsedMs, strength, reduced))),
    width: BURN_WIDTH,
    height: BURN_HEIGHT,
  }
}
