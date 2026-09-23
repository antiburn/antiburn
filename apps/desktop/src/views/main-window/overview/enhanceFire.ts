/*
 * The dot-matrix fire from the install banner (`install.sh`, "fire banner"),
 * ported to a canvas. Heat enters along the bottom row and rises. Each step
 * moves heat up along one wandering path, and a slow wind leans the field.
 */

// The install banner's ramp: a purple ember through the brand orange to a
// hot tint. Each stop is [position, red, green, blue].
const RAMP: readonly (readonly [number, number, number, number])[] = [
  [0.0, 60, 18, 70],
  [0.16, 60, 18, 70],
  [0.34, 150, 40, 60],
  [0.54, 232, 80, 34],
  [0.74, 255, 106, 44],
  [0.89, 255, 170, 90],
  [1.0, 255, 225, 180],
]

const PITCH = 7 // CSS pixels between dot centres
const RADIUS = 2
const FRAME_MS = 50 // the install banner's frame delay
const WARM = 40 // steps computed before the first still frame
const BASE = 1.16
const GUST = 0.05
const CAP = 0.9

function rampColor(heat: number) {
  const h = Math.min(heat / CAP, 1)
  for (let i = 1; i < RAMP.length; i++) {
    const [p1, r1, g1, b1] = RAMP[i]
    const [p0, r0, g0, b0] = RAMP[i - 1]
    if (h <= p1) {
      const t = (h - p0) / (p1 - p0)
      return `rgb(${r0 + (r1 - r0) * t} ${g0 + (g1 - g0) * t} ${b0 + (b1 - b0) * t})`
    }
  }
  const [, r, g, b] = RAMP[RAMP.length - 1]
  return `rgb(${r} ${g} ${b})`
}

export type FireHandle = {
  play: () => void
  pause: () => void
  dispose: () => void
}

/** Draws the fire into `canvas`. It shows a still frame until `play`. The
 *  canvas's `data-fuel` attribute (0 to 1) sets the flame strength. */
export function startFire(canvas: HTMLCanvasElement): FireHandle {
  // With no ResizeObserver or 2D context (for example in tests), the card
  // shows no fire.
  if (typeof ResizeObserver === "undefined") return { play() {}, pause() {}, dispose() {} }
  const context = canvas.getContext("2d")
  if (!context) return { play() {}, pause() {}, dispose() {} }
  let width = 0
  let height = 0
  let heat = new Float32Array(0)
  let burner = new Float32Array(0)
  let wind = 0
  let frame = 0
  let last = 0

  function decay() {
    // Flames reach about three quarters of the card height.
    return 1.1 / Math.max(height * 0.75, 1)
  }

  function step() {
    const fuel = Number(canvas.dataset.fuel ?? 1)
    wind = wind * 0.9 + (Math.random() - 0.5) * 0.16
    if (Math.random() < GUST) wind += (Math.random() - 0.5) * 0.55
    wind = Math.max(-0.75, Math.min(0.75, wind))
    const drift = wind > 0.25 ? 1 : wind < -0.25 ? -1 : 0
    const loss = decay()
    for (let y = 0; y < height - 1; y++) {
      const below = (y + 1) * width
      for (let x = 0; x < width; x++) {
        let source = x - drift + Math.floor(Math.random() * 3) - 1
        source = Math.max(0, Math.min(width - 1, source))
        const value =
          heat[below + source] * 0.9 +
          heat[below + x] * 0.1 -
          loss * (0.45 + Math.random() * 0.75)
        heat[y * width + x] = value > 0 ? value : 0
      }
    }
    const next = burner.map((value) => value * 0.82 + (0.72 + Math.random() * 0.5) * 0.18)
    const bottom = (height - 1) * width
    for (let x = 0; x < width; x++) {
      const left = next[Math.max(x - 1, 0)]
      const right = next[Math.min(x + 1, width - 1)]
      burner[x] = next[x] * 0.72 + (left + right) * 0.14
      heat[bottom + x] = fuel * BASE * burner[x]
    }
  }

  function draw() {
    const scale = window.devicePixelRatio || 1
    context.setTransform(scale, 0, 0, scale, 0, 0)
    context.clearRect(0, 0, canvas.clientWidth, canvas.clientHeight)
    for (let y = 0; y < height; y++) {
      for (let x = 0; x < width; x++) {
        const value = heat[y * width + x]
        if (value < 0.04) continue
        // Cool cells fade out instead of going black, so the fire sits on
        // either theme.
        context.globalAlpha = Math.min(value / 0.3, 1)
        context.fillStyle = rampColor(value)
        context.beginPath()
        context.arc(x * PITCH + PITCH / 2, y * PITCH + PITCH / 2, RADIUS, 0, Math.PI * 2)
        context.fill()
      }
    }
    context.globalAlpha = 1
  }

  function resize() {
    const scale = window.devicePixelRatio || 1
    canvas.width = Math.round(canvas.clientWidth * scale)
    canvas.height = Math.round(canvas.clientHeight * scale)
    width = Math.ceil(canvas.clientWidth / PITCH)
    height = Math.ceil(canvas.clientHeight / PITCH)
    heat = new Float32Array(width * height)
    burner = Float32Array.from({ length: width }, () => 0.85 + Math.random() * 0.3)
    for (let i = 0; i < WARM; i++) step()
    draw()
  }

  function tick(now: number) {
    frame = requestAnimationFrame(tick)
    if (now - last < FRAME_MS) return
    last = now
    step()
    draw()
  }

  const observer = new ResizeObserver(resize)
  observer.observe(canvas)

  return {
    play() {
      if (frame !== 0) return
      if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return
      frame = requestAnimationFrame(tick)
    },
    pause() {
      cancelAnimationFrame(frame)
      frame = 0
    },
    dispose() {
      cancelAnimationFrame(frame)
      observer.disconnect()
    },
  }
}
