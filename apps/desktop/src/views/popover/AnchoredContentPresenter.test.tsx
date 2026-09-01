import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { StrictMode, type ReactNode } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { AnchoredContentPresenter, type AnchoredPresentation } from "./AnchoredContentPresenter"

class FakeResizeObserver {
  static instances: FakeResizeObserver[] = []
  readonly callback: ResizeObserverCallback
  readonly disconnect = vi.fn()
  readonly observe = vi.fn()

  constructor(callback: ResizeObserverCallback) {
    this.callback = callback
    FakeResizeObserver.instances.push(this)
  }

  trigger(): void {
    this.callback([], this as unknown as ResizeObserver)
  }
}

const heights = new Map<number, number>()
let frames: FrameRequestCallback[] = []

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((settle, fail) => {
    resolve = settle
    reject = fail
  })
  return { promise, reject, resolve }
}

function content(generation: number, label: string): AnchoredPresentation<string> {
  return { generation, value: label }
}

function flushFrames(): void {
  const pending = frames
  frames = []
  pending.forEach((callback) => callback(0))
}

function presenter({
  requestedGeneration,
  presented,
  candidate,
  busy = true,
  retargetCommitRequired = false,
  initialPresentationCommitRequired = false,
  commitRetarget = vi.fn(async () => true),
  acknowledge = vi.fn(async () => true),
  onPromote = vi.fn(),
  onDiscard = vi.fn(),
  renderContent = (label) => <div>{label}</div>,
}: {
  requestedGeneration: number
  presented: AnchoredPresentation<string> | null
  candidate: AnchoredPresentation<string> | null
  busy?: boolean
  retargetCommitRequired?: boolean
  initialPresentationCommitRequired?: boolean
  commitRetarget?: (generation: number, height: number | null) => Promise<boolean>
  acknowledge?: (generation: number, height: number) => Promise<boolean>
  onPromote?: (generation: number) => void
  onDiscard?: (generation: number) => void
  renderContent?: (label: string) => ReactNode
}) {
  return (
    <AnchoredContentPresenter
      requestedGeneration={requestedGeneration}
      presented={presented}
      candidate={candidate}
      coldLoading={presented == null}
      busy={busy}
      loading={<div data-testid="cold-loading">Loading</div>}
      retargetCommitRequired={retargetCommitRequired}
      initialPresentationCommitRequired={initialPresentationCommitRequired}
      commitRetarget={commitRetarget}
      acknowledge={acknowledge}
      onPromote={onPromote}
      onDiscard={onDiscard}
      renderContent={renderContent}
    />
  )
}

beforeEach(() => {
  frames = []
  heights.clear()
  FakeResizeObserver.instances = []
  vi.stubGlobal("ResizeObserver", FakeResizeObserver)
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.push(callback)
    return frames.length
  })
  vi.stubGlobal("cancelAnimationFrame", vi.fn())
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
    this: HTMLElement,
  ) {
    const generation = Number(this.parentElement?.dataset.generation)
    return { height: heights.get(generation) ?? 320 } as DOMRect
  })
})

afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

describe("AnchoredContentPresenter", () => {
  it("commits only the latest retarget shell after its first frame", () => {
    const commitRetarget = vi.fn(async () => true)
    const { rerender } = render(
      presenter({
        requestedGeneration: 2,
        presented: null,
        candidate: null,
        retargetCommitRequired: true,
        commitRetarget,
      }),
    )

    expect(commitRetarget).not.toHaveBeenCalled()

    rerender(
      presenter({
        requestedGeneration: 3,
        presented: null,
        candidate: null,
        retargetCommitRequired: true,
        commitRetarget,
      }),
    )
    act(flushFrames)
    expect(commitRetarget).not.toHaveBeenCalled()
    act(flushFrames)

    expect(commitRetarget).toHaveBeenCalledOnce()
    expect(commitRetarget).toHaveBeenCalledWith(3, null)
  })

  it("commits seeded content with its measured height", () => {
    heights.set(2, 224)
    const commitRetarget = vi.fn(async () => true)
    render(
      presenter({
        requestedGeneration: 2,
        presented: content(2, "B"),
        candidate: null,
        retargetCommitRequired: true,
        commitRetarget,
      }),
    )

    act(flushFrames)
    act(flushFrames)

    expect(commitRetarget).toHaveBeenCalledWith(2, 224)
  })

  it("acknowledges the first stable seeded measurement", () => {
    heights.set(2, 224)
    const acknowledge = vi.fn(async () => true)
    render(
      presenter({
        requestedGeneration: 2,
        presented: content(2, "B"),
        candidate: null,
        initialPresentationCommitRequired: true,
        acknowledge,
      }),
    )

    expect(acknowledge).toHaveBeenCalledOnce()
    expect(acknowledge).toHaveBeenCalledWith(2, 224)
  })

  it("reschedules the retarget barrier during the StrictMode mount cycle", () => {
    const commitRetarget = vi.fn(async () => true)
    render(
      <StrictMode>
        {presenter({
          requestedGeneration: 2,
          presented: null,
          candidate: null,
          retargetCommitRequired: true,
          commitRetarget,
        })}
      </StrictMode>,
    )

    act(flushFrames)
    act(flushFrames)

    expect(commitRetarget).toHaveBeenCalledOnce()
    expect(commitRetarget).toHaveBeenCalledWith(2, null)
  })

  it("keeps candidate content gated when native rejects the retarget", async () => {
    heights.set(2, 220)
    const commitRetarget = vi.fn(async () => false)
    const acknowledge = vi.fn(async () => true)
    render(
      presenter({
        requestedGeneration: 2,
        presented: null,
        candidate: content(2, "B"),
        retargetCommitRequired: true,
        commitRetarget,
        acknowledge,
      }),
    )

    act(flushFrames)
    await act(async () => flushFrames())

    expect(commitRetarget).toHaveBeenCalledWith(2, null)
    expect(acknowledge).not.toHaveBeenCalled()
    expect(screen.queryByText("B")).not.toBeInTheDocument()
    expect(screen.getByTestId("cold-loading")).toBeInTheDocument()
  })

  it("shows a cold skeleton directly and acknowledges a measured candidate once", async () => {
    heights.set(1, 196)
    const acknowledge = vi.fn(async () => true)
    const onPromote = vi.fn()
    const { rerender } = render(
      presenter({
        requestedGeneration: 1,
        presented: null,
        candidate: null,
        acknowledge,
        onPromote,
      }),
    )

    expect(screen.getByTestId("cold-loading")).toBeInTheDocument()
    expect(screen.getByTestId("anchored-content-presenter")).toHaveAttribute(
      "aria-busy",
      "true",
    )
    expect(acknowledge).not.toHaveBeenCalled()

    rerender(
      presenter({
        requestedGeneration: 1,
        presented: null,
        candidate: content(1, "First"),
        acknowledge,
        onPromote,
      }),
    )

    await waitFor(() => expect(acknowledge).toHaveBeenCalledWith(1, 196))
    expect(acknowledge).toHaveBeenCalledOnce()
    const staged = screen.getByText("First").closest("[data-slot-state]")
    expect(staged).toHaveAttribute("data-slot-state", "staged")
    expect(staged).toHaveAttribute("inert")
    expect(staged).toHaveAttribute("aria-hidden", "true")

    act(flushFrames)
    const incoming = screen.getByText("First").closest("[data-slot-state]")!
    expect(incoming).toHaveAttribute("data-slot-state", "incoming")
    fireEvent.transitionEnd(incoming, { propertyName: "opacity" })

    expect(onPromote).toHaveBeenCalledWith(1)
    const stable = screen.getByText("First").closest("[data-slot-state]")
    expect(stable).toHaveAttribute("data-slot-state", "stable")
    expect(stable).not.toHaveAttribute("inert")
    expect(stable).not.toHaveAttribute("aria-hidden")
    expect(screen.queryByTestId("cold-loading")).not.toBeInTheDocument()
  })

  it("keeps A stable and hides stale B before staging C", async () => {
    heights.set(2, 220)
    heights.set(3, 240)
    const acknowledge = vi.fn(async () => true)
    const onPromote = vi.fn()
    const { rerender } = render(
      presenter({
        requestedGeneration: 2,
        presented: content(1, "A"),
        candidate: content(2, "B"),
        acknowledge,
        onPromote,
      }),
    )

    await waitFor(() => expect(acknowledge).toHaveBeenCalledWith(2, 220))
    act(flushFrames)
    expect(screen.getByText("B").closest("[data-slot-state]")).toHaveAttribute(
      "data-slot-state",
      "incoming",
    )

    rerender(
      presenter({
        requestedGeneration: 3,
        presented: content(1, "A"),
        candidate: content(3, "C"),
        acknowledge,
        onPromote,
      }),
    )

    expect(screen.getByText("A").closest("[data-slot-state]")).toHaveAttribute(
      "data-slot-state",
      "outgoing",
    )
    const stale = screen.getByText("B").closest("[data-slot-state]")!
    expect(stale).toHaveAttribute("data-slot-state", "discarding")
    expect(screen.queryByText("C")).not.toBeInTheDocument()

    fireEvent.transitionEnd(stale, { propertyName: "opacity" })
    await waitFor(() => expect(acknowledge).toHaveBeenCalledWith(3, 240))

    expect(screen.queryByText("B")).not.toBeInTheDocument()
    expect(screen.getByText("A")).toBeInTheDocument()
    expect(screen.getByText("C").closest("[data-slot-state]")).toHaveAttribute(
      "data-slot-state",
      "staged",
    )
    expect(onPromote).not.toHaveBeenCalled()
  })

  it("promotes a current candidate when native already presented its generation", async () => {
    heights.set(2, 220)
    const acknowledge = vi.fn(async () => false)
    const onPromote = vi.fn()
    render(
      presenter({
        requestedGeneration: 2,
        presented: content(1, "A"),
        candidate: content(2, "B"),
        acknowledge,
        onPromote,
      }),
    )

    await waitFor(() => expect(acknowledge).toHaveBeenCalledWith(2, 220))
    act(flushFrames)

    const incoming = screen.getByText("B").closest("[data-slot-state]")!
    expect(incoming).toHaveAttribute("data-slot-state", "incoming")
    fireEvent.transitionEnd(incoming, { propertyName: "opacity" })

    expect(onPromote).toHaveBeenCalledWith(2)
    expect(screen.getByText("B").closest("[data-slot-state]")).toHaveAttribute(
      "data-slot-state",
      "stable",
    )
  })

  it("deduplicates stable observer reports and coalesces a genuine resize", async () => {
    heights.set(2, 220)
    const acknowledge = vi.fn(async () => true)
    const onPromote = vi.fn()
    const { rerender } = render(
      presenter({
        requestedGeneration: 2,
        presented: content(1, "A"),
        candidate: content(2, "B"),
        acknowledge,
        onPromote,
      }),
    )

    await waitFor(() => expect(acknowledge).toHaveBeenCalledWith(2, 220))
    act(flushFrames)
    fireEvent.transitionEnd(screen.getByText("B").closest("[data-slot-state]")!, {
      propertyName: "opacity",
    })
    rerender(
      presenter({
        requestedGeneration: 2,
        presented: content(2, "B"),
        candidate: null,
        busy: false,
        acknowledge,
        onPromote,
      }),
    )

    const stableObserver = FakeResizeObserver.instances.find((observer) =>
      observer.observe.mock.calls.some(
        ([node]) => (node as HTMLElement).parentElement?.dataset.generation === "2",
      ),
    )!
    act(() => {
      stableObserver.trigger()
      stableObserver.trigger()
      flushFrames()
    })
    expect(acknowledge).toHaveBeenCalledOnce()

    heights.set(2, 260)
    act(() => {
      stableObserver.trigger()
      stableObserver.trigger()
    })
    expect(frames).toHaveLength(1)
    act(flushFrames)

    expect(acknowledge).toHaveBeenCalledTimes(2)
    expect(acknowledge).toHaveBeenLastCalledWith(2, 260)
  })

  it("does no DOM or native work for an identical lifecycle echo", () => {
    heights.set(1, 180)
    const acknowledge = vi.fn(async () => true)
    const props = {
      requestedGeneration: 1,
      presented: content(1, "A"),
      candidate: null,
      busy: false,
      acknowledge,
    }
    const { rerender } = render(presenter(props))
    const stable = screen.getByText("A").closest("[data-slot-state]")

    rerender(presenter(props))

    expect(screen.getByText("A").closest("[data-slot-state]")).toBe(stable)
    expect(acknowledge).not.toHaveBeenCalled()
  })

  it("disconnects observers and cancels scheduled work when unmounted", async () => {
    heights.set(2, 220)
    const acknowledge = vi.fn(async () => true)
    const { unmount } = render(
      presenter({
        requestedGeneration: 2,
        presented: content(1, "A"),
        candidate: content(2, "B"),
        acknowledge,
      }),
    )
    await waitFor(() => expect(acknowledge).toHaveBeenCalledOnce())
    expect(frames).toHaveLength(1)

    unmount()

    expect(
      FakeResizeObserver.instances.every((observer) => observer.disconnect.mock.calls.length),
    ).toBe(true)
    expect(cancelAnimationFrame).toHaveBeenCalled()
  })

  it("ignores a rejected acknowledgement after unmount", async () => {
    heights.set(2, 220)
    const acknowledgement = deferred<boolean>()
    const acknowledge = vi.fn(() => acknowledgement.promise)
    const onDiscard = vi.fn()
    const onPromote = vi.fn()
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined)
    const { unmount } = render(
      presenter({
        requestedGeneration: 2,
        presented: content(1, "A"),
        candidate: content(2, "B"),
        acknowledge,
        onDiscard,
        onPromote,
      }),
    )

    await waitFor(() => expect(acknowledge).toHaveBeenCalledWith(2, 220))
    unmount()
    await act(async () => {
      acknowledgement.reject(new Error("stale"))
      await Promise.resolve()
    })

    expect(onDiscard).not.toHaveBeenCalled()
    expect(onPromote).not.toHaveBeenCalled()
    expect(consoleError).not.toHaveBeenCalled()
  })

  it("uses the documented transition class for staged content", async () => {
    heights.set(2, 220)
    render(
      presenter({
        requestedGeneration: 2,
        presented: content(1, "A"),
        candidate: content(2, "B"),
      }),
    )

    await waitFor(() => expect(screen.getByText("B")).toBeInTheDocument())
    expect(screen.getByText("B").closest("[data-slot-state]")).toHaveClass(
      "ui-anchored-content-transition",
      "opacity-0",
    )
  })

  it("exposes current content but prevents host actions", () => {
    const action = vi.fn()
    render(
      presenter({
        requestedGeneration: 1,
        presented: content(1, "Current"),
        candidate: null,
        busy: false,
        renderContent: () => <button onClick={action}>Toggle usage</button>,
      }),
    )

    const button = screen.getByRole("button", { name: "Toggle usage" })
    const stable = button.closest("[data-slot-state]")
    expect(stable).not.toHaveAttribute("inert")
    expect(stable).toHaveClass("pointer-events-none")

    fireEvent.click(button)
    expect(action).not.toHaveBeenCalled()
  })
})
