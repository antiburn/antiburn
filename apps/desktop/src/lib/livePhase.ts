/**
 * One phase for every animation that shows a live session.
 *
 * The title shimmer in the session list and the sweep on the meters run the
 * same cycle. They must also hold the same point of that cycle, in one window
 * and between the popover and the HUD. A CSS animation starts when the browser
 * applies it, so each element starts its own cycle at its own moment.
 *
 * A negative `animation-delay` corrects only the start. A later render writes a
 * new delay while the animation keeps its first start time, and the phase
 * jumps. A window that stops painting also holds its animations while the clock
 * runs on.
 *
 * This module therefore owns the phase. It sets the start time of each live
 * animation from the wall clock. It sets the start time again when an animation
 * starts, when the window comes back, and once each cycle. The stylesheets
 * declare no delay, so no render moves an animation.
 */

/** The length of the cycle every live animation shares. */
export const LIVE_CYCLE_MS = 4000

/**
 * The keyframes that show a live session.
 *
 * `--activity-row-shimmer-cycle` in session-rows.css and `--led-sweep-cycle` in
 * hud.css give these animations their duration. Keep the three numbers equal.
 */
const LIVE_ANIMATIONS = new Set(["activity-row-title-shimmer", "led-sweep"])

/** The largest phase error this module accepts. It is one frame at 60 Hz. */
const TOLERANCE_MS = 16

/** The animation API reports a time as a number or as a unit value. */
function toMs(value: unknown): number | null {
  if (typeof value === "number") return Number.isFinite(value) ? value : null
  if (typeof value === "object" && value !== null && "value" in value) {
    const ms = Number((value as { value: unknown }).value)
    return Number.isFinite(ms) ? ms : null
  }
  return null
}

/** The point of the cycle that a time falls on. */
export function livePhase(time: number = Date.now()): number {
  return ((time % LIVE_CYCLE_MS) + LIVE_CYCLE_MS) % LIVE_CYCLE_MS
}

/** The distance between two phases, in the shorter direction around the cycle. */
function phaseDistance(a: number, b: number): number {
  const offset = livePhase(a - b)
  return Math.min(offset, LIVE_CYCLE_MS - offset)
}

/**
 * Puts one animation at the phase of the wall clock.
 *
 * The animation keeps the start time it has when that time is already in
 * phase. Returns true when the animation moves.
 */
export function anchorLiveAnimation(animation: Animation, now: number = Date.now()): boolean {
  const timeline = toMs(animation.timeline?.currentTime)
  if (timeline === null) return false
  // The stylesheets declare no delay, so the progress is the time since the
  // start time. A start time this far back therefore holds the wall clock's
  // phase.
  const target = timeline - livePhase(now)
  const start = toMs(animation.startTime)
  if (start !== null && phaseDistance(start, target) <= TOLERANCE_MS) return false
  animation.startTime = target
  return true
}

/** The name of a CSS animation. Older engines do not report one. */
function animationName(animation: Animation): string | undefined {
  return (animation as Animation & { animationName?: string }).animationName
}

/** Every live animation in the document, including those on pseudo-elements. */
function liveAnimations(doc: Document): Animation[] {
  if (typeof doc.getAnimations !== "function") return []
  return doc.getAnimations().filter((animation) => {
    const name = animationName(animation)
    return name !== undefined && LIVE_ANIMATIONS.has(name)
  })
}

/**
 * Holds every live animation at the phase of the wall clock.
 *
 * Each window must run this once, before the first render. It returns a
 * teardown function, which is safe to call twice.
 */
export function installLivePhase(doc: Document = document): () => void {
  const view = doc.defaultView
  let tick: ReturnType<typeof setInterval> | undefined
  let frame: number | undefined

  const stopTick = () => {
    if (tick === undefined) return
    clearInterval(tick)
    tick = undefined
  }

  // The tick catches a window that held its animations while it painted
  // nothing. It runs only while a live animation is on a visible screen.
  const anchorAll = () => {
    const animations = liveAnimations(doc)
    const now = Date.now()
    for (const animation of animations) anchorLiveAnimation(animation, now)
    if (animations.length === 0 || doc.hidden) {
      stopTick()
      return
    }
    if (tick === undefined) tick = setInterval(sync, LIVE_CYCLE_MS)
  }

  /*
   * A timeline reports the time of the last frame, and the wall clock reports
   * now. The two agree during an animation frame, and a window that paints
   * rarely puts them far apart. The anchor must compare equal times, so it
   * waits for a frame. A window that paints nothing therefore corrects itself
   * on the frame that follows its return.
   */
  const sync = () => {
    if (frame !== undefined) return
    if (!view?.requestAnimationFrame) {
      anchorAll()
      return
    }
    frame = view.requestAnimationFrame(() => {
      frame = undefined
      anchorAll()
    })
  }

  const onAnimationEvent = (event: Event) => {
    const name = (event as AnimationEvent).animationName
    if (typeof name === "string" && LIVE_ANIMATIONS.has(name)) sync()
  }

  // The capture phase reaches this listener even when a component stops the
  // event below the document.
  doc.addEventListener("animationstart", onAnimationEvent, true)
  // A canceled animation leaves the document. The sync then parks the tick.
  doc.addEventListener("animationcancel", onAnimationEvent, true)
  doc.addEventListener("visibilitychange", sync)
  view?.addEventListener("focus", sync)
  view?.addEventListener("pageshow", sync)
  sync()

  return () => {
    stopTick()
    if (frame !== undefined) {
      view?.cancelAnimationFrame(frame)
      frame = undefined
    }
    doc.removeEventListener("animationstart", onAnimationEvent, true)
    doc.removeEventListener("animationcancel", onAnimationEvent, true)
    doc.removeEventListener("visibilitychange", sync)
    view?.removeEventListener("focus", sync)
    view?.removeEventListener("pageshow", sync)
  }
}
