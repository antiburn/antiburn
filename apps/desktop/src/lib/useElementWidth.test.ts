// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { RefObject } from "react"

import { useElementWidth } from "./useElementWidth"

/**
 * jsdom has no `ResizeObserver`. This fake keeps a registry of observed
 * elements so a test can trigger a resize by calling the stashed callback
 * directly, the same way the browser would call it after layout.
 */
class FakeResizeObserver {
  static instances: FakeResizeObserver[] = []
  observed: Element[] = []
  callback: ResizeObserverCallback
  constructor(callback: ResizeObserverCallback) {
    this.callback = callback
    FakeResizeObserver.instances.push(this)
  }
  observe(element: Element): void {
    this.observed.push(element)
  }
  unobserve(element: Element): void {
    this.observed = this.observed.filter((e) => e !== element)
  }
  disconnect(): void {
    this.observed = []
  }
  trigger(): void {
    this.callback([], this as unknown as ResizeObserver)
  }
}

function elementWithWidth(width: number): HTMLElement {
  const element = document.createElement("div")
  element.getBoundingClientRect = () => ({ width }) as DOMRect
  return element
}

beforeEach(() => {
  FakeResizeObserver.instances = []
  vi.stubGlobal("ResizeObserver", FakeResizeObserver)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("useElementWidth", () => {
  it("measures the element as soon as the ref attaches, with no separate kick needed", () => {
    const ref = { current: elementWithWidth(120) } as RefObject<HTMLElement | null>

    const { result } = renderHook(() => useElementWidth(ref))

    expect(result.current).toBe(120)
    expect(FakeResizeObserver.instances[0]?.observed).toEqual([ref.current])
  })

  it("re-renders with the new width when the observer reports a resize", () => {
    const ref = { current: elementWithWidth(120) } as RefObject<HTMLElement | null>
    const { result } = renderHook(() => useElementWidth(ref))
    expect(result.current).toBe(120)

    ref.current!.getBoundingClientRect = () => ({ width: 240 }) as DOMRect
    act(() => {
      FakeResizeObserver.instances[0]?.trigger()
    })

    expect(result.current).toBe(240)
  })

  it("disconnects the observer on unmount", () => {
    const ref = { current: elementWithWidth(120) } as RefObject<HTMLElement | null>
    const { unmount } = renderHook(() => useElementWidth(ref))
    const observer = FakeResizeObserver.instances[0]!
    const disconnect = vi.spyOn(observer, "disconnect")

    unmount()

    expect(disconnect).toHaveBeenCalledTimes(1)
  })

  it("reports 0 without an element, and without crashing when ResizeObserver is unavailable", () => {
    vi.unstubAllGlobals()
    const ref = { current: null } as RefObject<HTMLElement | null>

    const { result } = renderHook(() => useElementWidth(ref))

    expect(result.current).toBe(0)
  })
})
