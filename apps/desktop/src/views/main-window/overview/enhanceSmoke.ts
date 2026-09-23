/*
 * LED smoke for the Enhance Done step. A layered value-noise field drifts up
 * and sways. Each cell of a dot grid lights by the smoke density there, so
 * the field reads like an LED panel. Unlit dots stay faintly visible. The
 * dot colour comes from the canvas's CSS `color`.
 *
 * The pointer plays with the smoke: dots near it glow, the smoke bulges
 * away from it, and a click sends a ring out across the panel. The canvas
 * takes no pointer events of its own, so its parent element supplies them.
 */

const PITCH = 7 // CSS pixels between dot centres
const RADIUS = 2.4
const FRAME_MS = 40
const RISE = 0.9 // grid cells per second
const SCALE = 0.085 // noise frequency per grid cell
const GLOW = 5 // one sigma of the pointer glow, in grid cells
const PUSH = 2.5 // how far the pointer pushes the smoke, in grid cells
const RING_MS = 900 // a click ring lives this long
const RING_CELLS = 30 // a click ring travels this far

interface Point {
  x: number
  y: number
}

interface Ring extends Point {
  at: number
}

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
 *  motion, it draws one still frame and ignores the pointer. */
export function startSmoke(canvas: HTMLCanvasElement): (() => void) | null {
  if (typeof ResizeObserver === "undefined") return null
  const context = canvas.getContext("2d")
  if (!context) return null
  const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches
  const surface = canvas.parentElement ?? canvas
  let columns = 0
  let rows = 0
  let frame = 0
  let last = 0
  let heat = new Float32Array(0)
  let target: Point | null = null
  const pointer = { x: 0, y: 0, strength: 0 }
  let rings: Ring[] = []
  const start = performance.now()

  function toCell(event: PointerEvent): Point {
    const rect = canvas.getBoundingClientRect()
    return { x: (event.clientX - rect.left) / PITCH, y: (event.clientY - rect.top) / PITCH }
  }

  function onMove(event: PointerEvent) {
    target = toCell(event)
  }

  function onLeave() {
    target = null
  }

  function onDown(event: PointerEvent) {
    rings.push({ ...toCell(event), at: performance.now() })
  }

  /** Adds heat to every cell within `reach` of a point. */
  function warm(cx: number, cy: number, reach: number, amount: (distance: number) => number) {
    const left = Math.max(0, Math.floor(cx - reach))
    const right = Math.min(columns - 1, Math.ceil(cx + reach))
    const top = Math.max(0, Math.floor(cy - reach))
    const bottom = Math.min(rows - 1, Math.ceil(cy + reach))
    for (let row = top; row <= bottom; row++) {
      for (let column = left; column <= right; column++) {
        const index = row * columns + column
        heat[index] = (heat[index] ?? 0) + amount(Math.hypot(column - cx, row - cy))
      }
    }
  }

  function draw(now: number) {
    const time = (now - start) / 1000
    // The glow follows the pointer with a lag and fades after it leaves.
    if (target) {
      pointer.x += (target.x - pointer.x) * 0.35
      pointer.y += (target.y - pointer.y) * 0.35
      pointer.strength = Math.min(1, pointer.strength + 0.15)
    } else {
      pointer.strength = Math.max(0, pointer.strength - 0.06)
    }
    rings = rings.filter((ring) => now - ring.at < RING_MS)
    heat.fill(0)
    if (pointer.strength > 0) {
      warm(pointer.x, pointer.y, GLOW * 3, (distance) => {
        return pointer.strength * Math.exp(-(distance * distance) / (2 * GLOW * GLOW))
      })
    }
    for (const ring of rings) {
      const age = (now - ring.at) / RING_MS
      const radius = age * RING_CELLS
      warm(ring.x, ring.y, radius + 3, (distance) => {
        return (1 - age) * Math.exp(-((distance - radius) * (distance - radius)) / 2.5)
      })
    }
    const scale = window.devicePixelRatio || 1
    context.setTransform(scale, 0, 0, scale, 0, 0)
    context.clearRect(0, 0, canvas.clientWidth, canvas.clientHeight)
    context.fillStyle = getComputedStyle(canvas).color
    const spread = 2 * (GLOW * 1.6) * (GLOW * 1.6)
    for (let row = 0; row < rows; row++) {
      // Smoke is densest at the bottom and thins as it rises.
      const rise = 0.35 + 0.65 * (row / Math.max(rows - 1, 1))
      const sway = Math.sin(row * 0.18 + time * 0.7) * 1.6
      for (let column = 0; column < columns; column++) {
        let x = column + sway
        let y = row + time * RISE * 4
        // The smoke bulges away from the pointer.
        if (pointer.strength > 0) {
          const dx = column - pointer.x
          const dy = row - pointer.y
          const d2 = dx * dx + dy * dy
          const push =
            (PUSH * pointer.strength * Math.exp(-d2 / spread)) / Math.max(Math.sqrt(d2), 0.5)
          x -= dx * push
          y -= dy * push
        }
        const n = fbm(x * SCALE, y * SCALE)
        const t = Math.min(Math.max((n - 0.42) / 0.33, 0), 1)
        const lit = t * t * (3 - 2 * t) * rise
        const glow = Math.min(heat[row * columns + column] ?? 0, 1)
        context.globalAlpha = Math.min(0.12 + lit * 0.6 + glow * 0.5, 1)
        context.beginPath()
        context.arc(
          column * PITCH + PITCH / 2,
          row * PITCH + PITCH / 2,
          RADIUS * (0.55 + lit * 0.45 + glow * 0.5),
          0,
          Math.PI * 2,
        )
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
    heat = new Float32Array(columns * rows)
    draw(performance.now())
  }

  const observer = new ResizeObserver(resize)
  observer.observe(canvas)
  if (!still) {
    frame = requestAnimationFrame(tick)
    surface.addEventListener("pointermove", onMove)
    surface.addEventListener("pointerleave", onLeave)
    surface.addEventListener("pointerdown", onDown)
  }
  return () => {
    cancelAnimationFrame(frame)
    observer.disconnect()
    surface.removeEventListener("pointermove", onMove)
    surface.removeEventListener("pointerleave", onLeave)
    surface.removeEventListener("pointerdown", onDown)
  }
}
