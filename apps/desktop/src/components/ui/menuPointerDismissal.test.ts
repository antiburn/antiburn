import { afterEach, describe, expect, it, vi } from "vitest"
import { observeMenuPointerExit } from "./menuPointerDismissal"

function setup(pointerOpened = true) {
  vi.useFakeTimers()
  const content = document.createElement("div")
  const trigger = document.createElement("button")
  content.style.setProperty("--space-sm", "8px")
  document.body.append(content, trigger)
  vi.spyOn(content, "getBoundingClientRect").mockReturnValue(new DOMRect(100, 100, 200, 300))
  vi.spyOn(trigger, "getBoundingClientRect").mockReturnValue(new DOMRect(240, 70, 60, 22))
  const dismiss = vi.fn()
  const cleanup = observeMenuPointerExit({
    content,
    trigger: () => trigger,
    pointerOpened,
    onDismiss: dismiss,
  })
  return { content, trigger, dismiss, cleanup }
}

function move(x: number, y: number, pointerType = "mouse") {
  document.dispatchEvent(
    new PointerEvent("pointermove", { clientX: x, clientY: y, pointerType }),
  )
}

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
  document.body.replaceChildren()
})

describe("menu pointer dismissal", () => {
  it("allows all four edges and the trigger gap, then closes after the grace delay", () => {
    const { dismiss, cleanup } = setup()
    try {
      for (const [x, y] of [
        [93, 200],
        [307, 200],
        [200, 93],
        [200, 407],
        [270, 96],
        [270, 75],
      ]) {
        move(x!, y!)
        vi.advanceTimersByTime(400)
        expect(dismiss).not.toHaveBeenCalled()
      }
      move(320, 200)
      vi.advanceTimersByTime(299)
      expect(dismiss).not.toHaveBeenCalled()
      move(330, 200)
      vi.advanceTimersByTime(1)
      expect(dismiss).toHaveBeenCalledOnce()
    } finally {
      cleanup()
    }
  })

  it("cancels pending dismissal when the pointer returns", () => {
    const { dismiss, cleanup } = setup()
    try {
      move(400, 200)
      vi.advanceTimersByTime(200)
      move(200, 200)
      vi.advanceTimersByTime(400)
      expect(dismiss).not.toHaveBeenCalled()
      move(400, 200)
      cleanup()
      vi.advanceTimersByTime(400)
      expect(dismiss).not.toHaveBeenCalled()
    } finally {
      cleanup()
    }
  })

  it("keeps keyboard and touch interaction open until mouse use resumes inside", () => {
    const { dismiss, cleanup } = setup(false)
    try {
      move(400, 200)
      vi.advanceTimersByTime(400)
      expect(dismiss).not.toHaveBeenCalled()
      move(200, 200)
      move(400, 200)
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown" }))
      vi.advanceTimersByTime(400)
      expect(dismiss).not.toHaveBeenCalled()
      move(200, 200, "touch")
      move(400, 200, "touch")
      vi.advanceTimersByTime(400)
      expect(dismiss).not.toHaveBeenCalled()
      move(200, 200)
      move(400, 200)
      vi.advanceTimersByTime(300)
      expect(dismiss).toHaveBeenCalledOnce()
    } finally {
      cleanup()
    }
  })

  it("keeps an associated portaled tooltip in the grace area", () => {
    const { content, dismiss, cleanup } = setup()
    const tooltip = document.createElement("div")
    tooltip.setAttribute("data-radix-popper-content-wrapper", "")
    tooltip.innerHTML = '<span id="menu-tip" role="tooltip">Details</span>'
    document.body.append(tooltip)
    const item = document.createElement("div")
    item.setAttribute("aria-describedby", "menu-tip")
    content.append(item)
    vi.spyOn(tooltip, "getBoundingClientRect").mockReturnValue(new DOMRect(310, 150, 180, 60))
    try {
      move(350, 170)
      vi.advanceTimersByTime(400)
      expect(dismiss).not.toHaveBeenCalled()
    } finally {
      cleanup()
    }
  })

  it("dismisses after the pointer leaves the window", () => {
    const { dismiss, cleanup } = setup()
    try {
      document.dispatchEvent(
        new PointerEvent("pointerout", { pointerType: "mouse", relatedTarget: null }),
      )
      vi.advanceTimersByTime(300)
      expect(dismiss).toHaveBeenCalledOnce()
    } finally {
      cleanup()
    }
  })
})
