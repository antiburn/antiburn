import { renderHook } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { useExternalSubscription } from "./useExternalSubscription"

describe("useExternalSubscription", () => {
  it("runs subscribe once on mount and its cleanup once on unmount", () => {
    const cleanup = vi.fn()
    const subscribe = vi.fn(() => cleanup)

    const { unmount } = renderHook(() => useExternalSubscription(subscribe))

    expect(subscribe).toHaveBeenCalledTimes(1)
    expect(cleanup).not.toHaveBeenCalled()

    unmount()

    expect(cleanup).toHaveBeenCalledTimes(1)
  })

  it("never re-renders the component, since the snapshot never changes", () => {
    const renders = vi.fn()
    renderHook(() => {
      renders()
      useExternalSubscription(() => () => {})
    })

    expect(renders).toHaveBeenCalledTimes(1)
  })

  it("tears down and re-runs subscribe when its identity changes between renders", () => {
    const firstCleanup = vi.fn()
    const secondCleanup = vi.fn()
    const firstSubscribe = vi.fn(() => firstCleanup)
    const secondSubscribe = vi.fn(() => secondCleanup)

    const { rerender } = renderHook(({ subscribe }) => useExternalSubscription(subscribe), {
      initialProps: { subscribe: firstSubscribe },
    })
    expect(firstSubscribe).toHaveBeenCalledTimes(1)

    rerender({ subscribe: secondSubscribe })

    // A fresh subscribe function is a signal to resubscribe — this is the
    // "keyed subscription" case the module doc calls out, not a bug.
    expect(firstCleanup).toHaveBeenCalledTimes(1)
    expect(secondSubscribe).toHaveBeenCalledTimes(1)
    expect(secondCleanup).not.toHaveBeenCalled()
  })

  it("does not resubscribe across renders when subscribe stays referentially stable", () => {
    const cleanup = vi.fn()
    const subscribe = vi.fn(() => cleanup)

    const { rerender } = renderHook(() => useExternalSubscription(subscribe))
    rerender()
    rerender()

    expect(subscribe).toHaveBeenCalledTimes(1)
    expect(cleanup).not.toHaveBeenCalled()
  })
})
