/*
 * LED smoke for the Enhance Done step. A layered value-noise field drifts up
 * and sways. Each cell of a dot grid lights by the smoke density there, so
 * the field reads like an LED panel. Unlit dots stay faintly visible. The
 * dot colour comes from the canvas's CSS `color`.
 */

const PITCH = 7 // CSS pixels between dot centres
const RADIUS = 2.4
const FRAME_MS = 40
const RISE = 0.9 // grid cells per second
const SCALE = 0.085 // noise frequency per grid cell

function hash(x: number, y: number) {
  let h = Math.imul(x, 374761393) + Math.imul(y, 668265263)
  h = Math.imul(h ^ (h >>> 13), 1274126177)
  return ((h ^ (h >>> 16)) >>> 0) / 4294967295
}

function noise(x: number, y: number) {
  const x0 = Math.floor(x)
  const y0 = Math.floor(y)
  const tx = x - x0
  const ty = y - y0
  const sx = tx * tx * (3 - 2 * tx)
  const sy = ty * ty * (3 - 2 * ty)
  const top = hash(x0, y0) + (hash(x0 + 1, y0) - hash(x0, y0)) * sx
  const bottom = hash(x0, y0 + 1) + (hash(x0 + 1, y0 + 1) - hash(x0, y0 + 1)) * sx
  return top + (bottom - top) * sy
}

function fbm(x: number, y: number) {
  return noise(x, y) * 0.55 + noise(x * 2.1, y * 2.1) * 0.3 + noise(x * 4.3, y * 4.3) * 0.15
}

/** Draws LED smoke into `canvas` until the returned function runs. It
 *  returns null when there is no 2D context or ResizeObserver. With reduced
 *  motion, it draws one still frame. */
export function startSmoke(canvas: HTMLCanvasElement): (() => void) | null {
  if (typeof ResizeObserver === "undefined") return null
  const context = canvas.getContext("2d")
  if (!context) return null
  const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches
  let columns = 0
  let rows = 0
  let frame = 0
  let last = 0
  const start = performance.now()

  function draw(now: number) {
    const time = (now - start) / 1000
    const scale = window.devicePixelRatio || 1
    context.setTransform(scale, 0, 0, scale, 0, 0)
    context.clearRect(0, 0, canvas.clientWidth, canvas.clientHeight)
    context.fillStyle = getComputedStyle(canvas).color
    for (let row = 0; row < rows; row++) {
      // Smoke is densest at the bottom and thins as it rises.
      const rise = 0.35 + 0.65 * (row / Math.max(rows - 1, 1))
      const sway = Math.sin(row * 0.18 + time * 0.7) * 1.6
      for (let column = 0; column < columns; column++) {
        const n = fbm((column + sway) * SCALE, (row + time * RISE * 4) * SCALE)
        const t = Math.min(Math.max((n - 0.42) / 0.33, 0), 1)
        const lit = t * t * (3 - 2 * t) * rise
        const x = column * PITCH + PITCH / 2
        const y = row * PITCH + PITCH / 2
        context.globalAlpha = 0.12 + lit * 0.6
        context.beginPath()
        context.arc(x, y, RADIUS * (0.55 + lit * 0.45), 0, Math.PI * 2)
        context.fill()
      }
    }
    context.globalAlpha = 1
  }

  function tick(now: number) {
    frame = requestAnimationFrame(tick)
    if (now - last < FRAME_MS) return
    last = now
    draw(now)
  }

  function resize() {
    const scale = window.devicePixelRatio || 1
    canvas.width = Math.round(canvas.clientWidth * scale)
    canvas.height = Math.round(canvas.clientHeight * scale)
    columns = Math.ceil(canvas.clientWidth / PITCH)
    rows = Math.ceil(canvas.clientHeight / PITCH)
    draw(performance.now())
  }

  const observer = new ResizeObserver(resize)
  observer.observe(canvas)
  if (!still) frame = requestAnimationFrame(tick)
  return () => {
    cancelAnimationFrame(frame)
    observer.disconnect()
  }
}
