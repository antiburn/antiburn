import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { ProjectFolderActions } from "./ProjectFolderActions"

beforeEach(() => vi.useFakeTimers())
afterEach(() => vi.useRealTimers())

function setup(path = "/tmp/worktrees/issue-321") {
  const onOpen = vi.fn().mockResolvedValue(undefined)
  const onCopy = vi.fn().mockResolvedValue(undefined)
  const view = render(<ProjectFolderActions path={path} onOpen={onOpen} onCopy={onCopy} />)
  const trigger = screen.getByRole("button", { name: "Project folder" })
  return { ...view, trigger, onOpen, onCopy }
}

function focus(trigger: HTMLElement) {
  act(() => trigger.focus())
}

describe("ProjectFolderActions", () => {
  it("opens on delayed hover without moving focus and keeps the pointer transfer open", () => {
    const { trigger } = setup()
    fireEvent.pointerEnter(trigger, { pointerType: "mouse" })
    act(() => vi.advanceTimersByTime(299))
    expect(screen.queryByRole("dialog")).toBeNull()
    act(() => vi.advanceTimersByTime(1))
    const dialog = screen.getByRole("dialog")
    expect(document.activeElement).not.toBe(trigger)
    expect(dialog.parentElement).toBe(document.body)
    fireEvent.pointerLeave(trigger)
    act(() => vi.advanceTimersByTime(100))
    fireEvent.pointerEnter(dialog)
    act(() => vi.advanceTimersByTime(300))
    expect(screen.getByRole("dialog")).toBe(dialog)
    fireEvent.pointerLeave(dialog)
    act(() => vi.advanceTimersByTime(200))
    expect(screen.queryByRole("dialog")).toBeNull()
  })

  it("cancels brief hovers and does not toggle on mouse clicks", () => {
    const { trigger } = setup()
    fireEvent.pointerEnter(trigger)
    fireEvent.pointerLeave(trigger)
    fireEvent.click(trigger)
    act(() => vi.advanceTimersByTime(500))
    expect(screen.queryByRole("dialog")).toBeNull()
  })

  it("opens on focus, tabs to the actions, and returns focus on Escape", () => {
    const { trigger } = setup()
    focus(trigger)
    expect(screen.getByRole("dialog")).toBeVisible()
    fireEvent.keyDown(trigger, { key: "Tab" })
    expect(document.activeElement?.getAttribute("aria-label")).toMatch(/^Open in/)
    fireEvent.keyDown(document.activeElement!, { key: "Escape" })
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(document.activeElement).toBe(trigger)
  })

  it.each(["/tmp/日本語/project with spaces", String.raw`C:\Users\dev\worktrees\project`])(
    "shows the exact selectable path: %s",
    (path) => {
      const { trigger } = setup(path)
      focus(trigger)
      expect(document.querySelector(".project-folder-path")?.textContent).toBe(path)
    },
  )

  it("reports copy success only after resolution and clears the feedback", async () => {
    const { trigger, onCopy } = setup()
    let resolve!: () => void
    onCopy.mockImplementation(
      () =>
        new Promise<void>((done) => {
          resolve = done
        }),
    )
    focus(trigger)
    fireEvent.click(screen.getByRole("button", { name: "Copy path" }))
    fireEvent.click(screen.getByRole("button", { name: "Copy path" }))
    expect(onCopy).toHaveBeenCalledOnce()
    expect(screen.queryByRole("button", { name: "Path copied" })).toBeNull()
    await act(async () => resolve())
    expect(screen.getByRole("button", { name: "Path copied" })).toBeVisible()
    act(() => vi.advanceTimersByTime(2000))
    expect(screen.getByRole("button", { name: "Copy path" })).toBeVisible()
  })

  it("keeps failed open and copy actions available with inline errors", async () => {
    const { trigger, onCopy, onOpen } = setup()
    onOpen.mockRejectedValue(new Error("missing"))
    onCopy.mockRejectedValue(new Error("denied"))
    focus(trigger)
    await act(async () => fireEvent.click(screen.getByRole("button", { name: /^Open in/ })))
    expect(screen.getByRole("alert")).toHaveTextContent("Couldn’t open this folder")
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Copy path" })))
    expect(screen.getByRole("alert")).toHaveTextContent("Couldn’t copy the path")
    expect(screen.queryByRole("button", { name: "Path copied" })).toBeNull()
  })

  it("dismisses for an outside press or surrounding scroll", () => {
    const { trigger } = setup()
    focus(trigger)
    fireEvent.pointerDown(document.body)
    expect(screen.queryByRole("dialog")).toBeNull()
    fireEvent.blur(trigger)
    fireEvent.focus(trigger)
    fireEvent.scroll(document)
    expect(screen.queryByRole("dialog")).toBeNull()
  })

  it("cancels timers and removes its portal when unmounted", () => {
    const { trigger, unmount } = setup()
    fireEvent.pointerEnter(trigger)
    unmount()
    act(() => vi.runAllTimers())
    expect(screen.queryByRole("dialog")).toBeNull()
  })

  it("repositions growing feedback above the trigger and disconnects on dismissal", () => {
    let resize!: ResizeObserverCallback
    const disconnect = vi.fn()
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(callback: ResizeObserverCallback) {
          resize = callback
        }
        observe() {}
        disconnect = disconnect
      },
    )
    try {
      const { trigger } = setup()
      vi.spyOn(trigger, "getBoundingClientRect").mockReturnValue({
        top: window.innerHeight - 160,
        bottom: window.innerHeight - 140,
        right: 420,
      } as DOMRect)
      focus(trigger)
      const dialog = screen.getByRole("dialog")
      vi.spyOn(dialog, "getBoundingClientRect").mockReturnValue({
        width: 390,
        height: 200,
      } as DOMRect)
      act(() => resize([], {} as ResizeObserver))
      expect(dialog.style.top).toBe(`${window.innerHeight - 368}px`)
      fireEvent.keyDown(trigger, { key: "Escape" })
      expect(disconnect).toHaveBeenCalledOnce()
    } finally {
      vi.unstubAllGlobals()
    }
  })
})
