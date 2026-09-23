/*
 * LED confetti for the Enhance Done step. Particles burst up from the bottom
 * and fall back. Each one draws on a dot grid, so the burst reads like an LED
 * panel and not like paper. The colours come from the theme tokens.
 */

const PITCH = 6 // CSS pixels between dot centres
const RADIUS = 2.1
const COUNT = 140
const GRAVITY = 0.16
const DRAG = 0.985
const TOKENS = [
  "--color-brand-tint",
  "--color-burn-check-pass-fill",
  "--color-check-overthinking",
  "--color-check-cache",
  "--color-check-mcp",
]

type Particle = { x: number; y: number; vx: number; vy: number; color: string; life: number }

/** Bursts confetti on `canvas` and returns a function that stops it. It
 *  returns null when there is no 2D context or the reader reduces motion. */
export function burstConfetti(canvas: HTMLCanvasElement): (() => void) | null {
  const context = canvas.getContext("2d")
  if (!context) return null
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return null
  const style = getComputedStyle(canvas)
  const colors = TOKENS.map((token) => style.getPropertyValue(token).trim()).filter(Boolean)
  const scale = window.devicePixelRatio || 1
  const width = canvas.clientWidth
  const height = canvas.clientHeight
  canvas.width = Math.round(width * scale)
  canvas.height = Math.round(height * scale)
  context.setTransform(scale, 0, 0, scale, 0, 0)

  const particles: Particle[] = Array.from({ length: COUNT }, (_, index) => {
    // Two fountains, one on each side, aimed in to the middle.
    const left = index % 2 === 0
    const angle = (left ? -60 : -120) + (Math.random() - 0.5) * 40
    const speed = 7 + Math.random() * 7
    return {
      x: left ? width * 0.08 : width * 0.92,
      y: height,
      vx: Math.cos((angle * Math.PI) / 180) * speed,
      vy: Math.sin((angle * Math.PI) / 180) * speed,
      color: colors[index % colors.length] ?? "currentColor",
      life: 1,
    }
  })

  let frame = requestAnimationFrame(function tick() {
    context.clearRect(0, 0, width, height)
    let alive = 0
    for (const particle of particles) {
      particle.vx *= DRAG
      particle.vy = particle.vy * DRAG + GRAVITY
      particle.x += particle.vx
      particle.y += particle.vy
      if (particle.vy > 0) particle.life -= 0.008
      if (particle.life <= 0 || particle.y > height + PITCH) continue
      alive++
      // Snap to the dot grid for the LED look.
      const x = Math.round(particle.x / PITCH) * PITCH
      const y = Math.round(particle.y / PITCH) * PITCH
      context.globalAlpha = Math.min(particle.life * 1.5, 1)
      context.fillStyle = particle.color
      context.beginPath()
      context.arc(x, y, RADIUS, 0, Math.PI * 2)
      context.fill()
    }
    context.globalAlpha = 1
    frame = alive > 0 ? requestAnimationFrame(tick) : 0
  })
  return () => cancelAnimationFrame(frame)
}
