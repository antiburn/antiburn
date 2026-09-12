import { afterEach, describe, expect, it, vi } from "vitest"

import { LIVE_CYCLE_MS, anchorLiveAnimation, installLivePhase, livePhase } from "./livePhase"

/** A stand-in for the part of `Animation` the module reads and writes. */
function fakeAnimation(
  animationName: string,
  timelineMs: number,
  startTime: number | null = null,
) {
  return {
    animationName,
    startTime,
    timeline: { currentTime: timelineMs },
  } as unknown as Animation & { animationName: string }
}

/** The point of the cycle an animation shows at the given timeline time. */
function progress(animation: Animation, timelineMs: number): number {
  return livePhase(timelineMs - Number(animation.startTime))
}

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
  // jsdom declares no `getAnimations`, so the stub needs removing by hand.
  delete (document as Partial<Document>).getAnimations
})

describe("livePhase", () => {
  it("repeats every cycle and never reports a negative point", () => {
    expect(livePhase(10_000)).toBe(2000)
    expect(livePhase(10_000 + LIVE_CYCLE_MS)).toBe(2000)
    expect(livePhase(-1000)).toBe(3000)
  })
})

describe("anchorLiveAnimation", () => {
  it("puts an animation at the point of the cycle the wall clock holds", () => {
    const animation = fakeAnimation("led-sweep", 500)
    expect(anchorLiveAnimation(animation, 10_000)).toBe(true)
    expect(progress(animation, 500)).toBe(livePhase(10_000))
  })

  it("agrees between two animations that start at different moments", () => {
    // Two windows, each with its own timeline origin, and one wall clock.
    const early = fakeAnimation("activity-row-title-shimmer", 120)
    const late = fakeAnimation("led-sweep", 900_000)
    anchorLiveAnimation(early, 1_234_567)
    anchorLiveAnimation(late, 1_234_567)
    expect(progress(early, 120)).toBe(progress(late, 900_000))
  })

  it("leaves an animation that already holds the phase", () => {
    const animation = fakeAnimation("led-sweep", 500)
    anchorLiveAnimation(animation, 10_000)
    const anchored = animation.startTime
    // A whole cycle later the same phase comes around, so nothing moves.
    expect(anchorLiveAnimation(animation, 10_000 + LIVE_CYCLE_MS)).toBe(false)
    expect(animation.startTime).toBe(anchored)
  })

  it("corrects an animation that a render or a pause moved", () => {
    const animation = fakeAnimation("led-sweep", 500)
    anchorLiveAnimation(animation, 10_000)
    // A paused window holds its animation while the clock runs on.
    animation.startTime = Number(animation.startTime) + 700
    expect(anchorLiveAnimation(animation, 10_000)).toBe(true)
    expect(progress(animation, 500)).toBe(livePhase(10_000))
  })

  it("reports no move for an animation without a timeline", () => {
    const animation = { startTime: null, timeline: null } as unknown as Animation
    expect(anchorLiveAnimation(animation, 10_000)).toBe(false)
  })
})

describe("installLivePhase", () => {
  /** Fires the event the browser sends when a CSS animation starts. */
  function startAnimation(doc: Document, animationName: string) {
    const event = new Event("animationstart", { bubbles: true })
    Object.defineProperty(event, "animationName", { value: animationName })
    doc.dispatchEvent(event)
  }

  /**
   * Runs the animation frame the module waits for.
   *
   * The anchor must compare the animation clock with the wall clock at one
   * moment, and the two agree only during a frame.
   */
  function flushFrame() {
    vi.advanceTimersByTime(20)
  }

  it("anchors an animation on the frame after it starts", () => {
    vi.useFakeTimers()
    const animation = fakeAnimation("led-sweep", 500)
    document.getAnimations = vi.fn(() => [animation])
    const stop = installLivePhase(document)
    flushFrame()
    animation.startTime = null
    startAnimation(document, "led-sweep")
    flushFrame()
    expect(animation.startTime).not.toBeNull()
    stop()
  })

  it("ignores an animation that shows no live session", () => {
    vi.useFakeTimers()
    const animation = fakeAnimation("hud-detail-in", 500)
    document.getAnimations = vi.fn(() => [animation])
    const stop = installLivePhase(document)
    flushFrame()
    startAnimation(document, "hud-detail-in")
    flushFrame()
    expect(animation.startTime).toBeNull()
    stop()
  })

  it("corrects an animation that drifts, once each cycle", () => {
    vi.useFakeTimers()
    const animation = fakeAnimation("led-sweep", 500)
    document.getAnimations = vi.fn(() => [animation])
    const stop = installLivePhase(document)
    flushFrame()
    const anchored = Number(animation.startTime)
    animation.startTime = anchored + 900
    vi.advanceTimersByTime(LIVE_CYCLE_MS + 40)
    expect(Number(animation.startTime)).not.toBe(anchored + 900)
    stop()
  })

  it("runs no timer while no live animation is on screen", () => {
    vi.useFakeTimers()
    document.getAnimations = vi.fn(() => [])
    const stop = installLivePhase(document)
    flushFrame()
    expect(vi.getTimerCount()).toBe(0)
    stop()
  })

  it("stops its timer and its listeners on teardown", () => {
    vi.useFakeTimers()
    const animation = fakeAnimation("led-sweep", 500)
    document.getAnimations = vi.fn(() => [animation])
    const stop = installLivePhase(document)
    flushFrame()
    expect(vi.getTimerCount()).toBe(1)
    stop()
    expect(vi.getTimerCount()).toBe(0)
    animation.startTime = null
    startAnimation(document, "led-sweep")
    flushFrame()
    expect(animation.startTime).toBeNull()
  })
})
