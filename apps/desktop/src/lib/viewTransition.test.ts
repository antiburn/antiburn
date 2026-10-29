import { afterEach, describe, expect, it, vi } from "vitest"

import { withViewTransition } from "./viewTransition"

afterEach(() => vi.unstubAllGlobals())

function environment(visibility = "visible", reducedMotion = false) {
  const start = vi.fn()
  vi.stubGlobal("document", { visibilityState: visibility, startViewTransition: start })
  vi.stubGlobal("window", { matchMedia: () => ({ matches: reducedMotion }) })
  return start
}

describe("withViewTransition", () => {
  it("applies a hidden document update without starting a transition", async () => {
    const start = environment("hidden")
    const update = vi.fn()
    await withViewTransition(update)
    expect(update).toHaveBeenCalledExactlyOnceWith()
    expect(start).not.toHaveBeenCalled()
  })

  it("applies reduced-motion updates directly", async () => {
    const start = environment("visible", true)
    const update = vi.fn()
    await withViewTransition(update)
    expect(update).toHaveBeenCalledOnce()
    expect(start).not.toHaveBeenCalled()
  })

  it("completes an applied update when the browser skips its animation", async () => {
    const start = environment()
    const update = vi.fn()
    start.mockImplementation((callback: () => void) => {
      callback()
      return {
        updateCallbackDone: Promise.resolve(),
        finished: Promise.reject(new DOMException("Document hidden", "InvalidStateError")),
      }
    })
    await expect(withViewTransition(update)).resolves.toBeUndefined()
    expect(update).toHaveBeenCalledOnce()
  })

  it("preserves update errors when the animation also rejects", async () => {
    const start = environment()
    const failure = new Error("Update failed")
    start.mockReturnValue({
      updateCallbackDone: Promise.reject(failure),
      finished: Promise.reject(failure),
    })
    await expect(withViewTransition(vi.fn())).rejects.toBe(failure)
  })

  it("waits for a successful visual transition", async () => {
    const start = environment()
    let finish!: () => void
    start.mockImplementation((callback: () => void) => {
      callback()
      return {
        updateCallbackDone: Promise.resolve(),
        finished: new Promise<void>((resolve) => {
          finish = resolve
        }),
      }
    })
    const completed = vi.fn()
    const result = withViewTransition(vi.fn()).then(completed)
    await Promise.resolve()
    expect(completed).not.toHaveBeenCalled()
    finish()
    await result
    expect(completed).toHaveBeenCalledOnce()
  })
})
