import { act, fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { Nudge } from "../lib/ipc"
import { NudgeView } from "./NudgeView"

/**
 * The notification window.
 *
 * Everything here is state a reader never sees directly: whether the
 * auto-dismiss timer is paused, which CTA an incidental click resolves to, and
 * whether the shell is told exactly once. The shell is stubbed at the module
 * boundary, as everywhere else in this app.
 */

const invoke = vi.hoisted(() => vi.fn())
/** Shell event handlers the view subscribed to, by event name. */
const listeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler)
    return () => listeners.delete(name)
  }),
}))

const usageMilestone: Nudge = {
  id: "usage-milestone-1",
  kind: "usageMilestone",
  tone: "info",
  title: "75% of your weekly limit used",
  subtitle: "The provider reported this usage milestone.",
  description: "The percentage comes from the provider's current usage reading.",
  recommendations: ["Review recent usage", "Check the provider dashboard"],
  actions: [
    { id: "notification_settings", label: "Settings", primary: false },
    { id: "dismiss", label: "Dismiss", primary: false },
    { id: "view_session", label: "View session", primary: true },
  ],
  timeoutMs: 10_000,
}

/**
 * A payload with no `recommendations` at all: the wire contract omits an empty
 * array (Rust `skip_serializing_if`), so it arrives with the field absent rather
 * than as `[]`.
 */
const scanFailureNoRecs = {
  id: "scan-failure-1",
  kind: "scanFailure",
  tone: "warning",
  title: "Scan failed",
  subtitle: "Could not read ~/.claude/projects.",
  description:
    "Review scan folders and folder access, then rescan. Everything already indexed is unaffected.",
  actions: [
    { id: "notification_settings", label: "Settings", primary: false },
    { id: "dismiss", label: "Not now", primary: false },
    { id: "review_sources", label: "Review sources", primary: true },
  ],
  timeoutMs: 10_000,
} as unknown as Nudge

/** The only primary CTA is a dismiss, so a body click has nothing to run. */
const diskSpaceLow: Nudge = {
  id: "disk-space-low-1",
  kind: "diskSpaceLow",
  tone: "warning",
  title: "Free space is running low",
  subtitle: "4 GB left on the startup volume.",
  description: "Free space dropped below your warning threshold.",
  actions: [
    { id: "notification_settings", label: "Settings", primary: false },
    { id: "dismiss", label: "Got it", primary: true },
  ],
  timeoutMs: 10_000,
}

/** The settings preview emits no CTAs at all. */
const testNudge: Nudge = {
  id: "test-1",
  kind: "test",
  tone: "info",
  title: "Notifications are working",
  subtitle: "This sample uses your current notification settings.",
  description: "Future notifications use this same layout.",
  actions: [
    { id: "notification_settings", label: "Settings", primary: false },
    { id: "dismiss", label: "Dismiss", primary: true },
  ],
  timeoutMs: 10_000,
}

/** A generic nudge can still ask only to surface the app. */
const actionlessNudge: Nudge = {
  ...testNudge,
  id: "actionless-1",
  actions: [],
}

/** A CTA carrying the session it is about, echoed back to the shell on click. */
const targeted: Nudge = {
  ...usageMilestone,
  id: "usage-milestone-targeted",
  actions: [
    { id: "dismiss", label: "Dismiss", primary: false },
    {
      id: "view_session",
      label: "View session",
      primary: true,
      target: { type: "session", agent: "claude", sessionId: "session-9", environment: null },
    },
  ],
}

const updateAvailable: Nudge = {
  id: "update-2.4.0",
  kind: "updateAvailable",
  tone: "info",
  title: "antiburn update released",
  subtitle: "New version 2.4.0 is available.",
  description:
    "Select Install to download, verify, and install it. Open About to follow progress.",
  actions: [
    { id: "notification_settings", label: "Settings", primary: false },
    { id: "dismiss", label: "Dismiss", primary: false },
    {
      id: "install",
      label: "Install",
      primary: true,
      target: { type: "update", expectedVersion: "2.4.0" },
    },
  ],
  timeoutMs: 10_000,
}

/** Push a nudge at the view, the way the crate's `nudge:show` event does. */
function showNudge(payload: Nudge) {
  act(() => listeners.get("nudge:show")?.({ payload }))
}

/** The crate's native cursor sample (macOS), which bypasses pointer events. */
function nativeHover(hovered: boolean) {
  act(() => listeners.get("nudge:hover")?.({ payload: hovered }))
}

const callsTo = (command: string) =>
  invoke.mock.calls.filter((call) => call[0] === command).length

/** The `hovered` argument of every `nudge_set_hovered` call, in order. */
const hoverCommandArgs = () =>
  invoke.mock.calls
    .filter((call) => call[0] === "nudge_set_hovered")
    .map((call) => (call[1] as { hovered: boolean }).hovered)

function notificationCard(container: HTMLElement): HTMLElement {
  const wrapper = container.firstElementChild as HTMLElement
  return wrapper.firstElementChild as HTMLElement
}

function finishAnimation(element: Element) {
  fireEvent.animationEnd(element)
  // jsdom lacks style.animation, so React registers its vendor-prefixed
  // fallback listener in tests. Real browsers use the unprefixed event.
  fireEvent(element, new Event("webkitAnimationEnd", { bubbles: true }))
}

beforeEach(() => {
  invoke.mockReset()
  invoke.mockResolvedValue(null)
  listeners.clear()
})

describe("NudgeView", () => {
  it("keeps content rendered while the entrance animation is pending", () => {
    const { container } = render(<NudgeView />)
    showNudge(usageMilestone)

    expect(notificationCard(container).className).toContain("animate-nudge-in")
    expect(screen.getByText(usageMilestone.title)).toBeInTheDocument()
    expect(screen.getByText(usageMilestone.subtitle).className).toContain("line-clamp-2")
  })

  it("settles a completed entrance and cancels its watchdog", () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<NudgeView />)
      showNudge(usageMilestone)
      const card = notificationCard(container)
      const timerCountWhileEntering = vi.getTimerCount()

      act(() => {
        finishAnimation(card)
      })

      expect(card.className).not.toContain("animate-nudge-in")
      expect(vi.getTimerCount()).toBe(timerCountWhileEntering - 1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("recovers a stalled entrance after 350ms", () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<NudgeView />)
      showNudge(usageMilestone)
      const card = notificationCard(container)

      act(() => {
        vi.advanceTimersByTime(350)
      })

      expect(card.className).not.toContain("animate-nudge-in")
    } finally {
      vi.useRealTimers()
    }
  })

  it("restarts the entrance watchdog when a nudge is replaced", () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<NudgeView />)
      showNudge(usageMilestone)

      act(() => {
        vi.advanceTimersByTime(300)
      })
      showNudge(scanFailureNoRecs)

      act(() => {
        vi.advanceTimersByTime(50)
      })
      expect(notificationCard(container).className).toContain("animate-nudge-in")

      act(() => {
        vi.advanceTimersByTime(300)
      })
      expect(notificationCard(container).className).not.toContain("animate-nudge-in")
    } finally {
      vi.useRealTimers()
    }
  })

  it("ignores animation events bubbled from notification content", () => {
    const { container } = render(<NudgeView />)
    showNudge(usageMilestone)
    const card = notificationCard(container)

    act(() => {
      finishAnimation(screen.getByText(usageMilestone.title))
    })

    expect(card.className).toContain("animate-nudge-in")
  })

  it("renders the title and subtitle while collapsed, but hides expanded detail and actions", () => {
    render(<NudgeView />)
    showNudge(usageMilestone)

    expect(screen.getByText(usageMilestone.title)).toBeInTheDocument()
    expect(screen.getByText(usageMilestone.subtitle)).toBeInTheDocument()
    expect(screen.queryByText(usageMilestone.description)).not.toBeInTheDocument()
    expect(screen.queryByText("Review recent usage")).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "View session" })).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Dismiss" })).not.toBeInTheDocument()
  })

  it("expands on hover to reveal the description, recommendations, and action bar", () => {
    const { container } = render(<NudgeView />)
    showNudge(usageMilestone)
    const wrapper = container.firstElementChild as HTMLElement

    act(() => {
      fireEvent.mouseEnter(wrapper)
    })
    expect(screen.getByText(usageMilestone.subtitle).className).not.toContain("line-clamp-2")
    expect(screen.getByText(usageMilestone.description)).toBeInTheDocument()
    expect(screen.getByText("Review recent usage")).toBeInTheDocument()
    expect(screen.getByText("Check the provider dashboard")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Settings" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "View session" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Dismiss" })).toBeInTheDocument()

    act(() => {
      fireEvent.mouseLeave(wrapper)
    })
    expect(screen.queryByText(usageMilestone.description)).not.toBeInTheDocument()
    expect(screen.queryByText("Review recent usage")).not.toBeInTheDocument()
  })

  it("expands a nudge that has no recommendations field without crashing", () => {
    const { container } = render(<NudgeView />)
    showNudge(scanFailureNoRecs)

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement)
    })

    expect(screen.getByText("Scan failed")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Review sources" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Not now" })).toBeInTheDocument()
  })

  it("reveals on show, then animates a resize on expand so the notification grows in place", () => {
    const { container } = render(<NudgeView />)
    showNudge(usageMilestone)
    // Sized + shown once on first measurement; no resize yet.
    expect(callsTo("nudge_reveal")).toBe(1)
    expect(callsTo("nudge_resize")).toBe(0)

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement)
    })

    // Expanding remeasures the same nudge → resize (animated), not another reveal.
    expect(callsTo("nudge_reveal")).toBe(1)
    expect(callsTo("nudge_resize")).toBeGreaterThanOrEqual(1)
  })

  it("reveals once, ignoring a re-delivered event with the same id", () => {
    render(<NudgeView />)
    showNudge(usageMilestone)
    // Re-delivery of the same id: the crate re-emits its pending payload when
    // the webview reports that its listener is attached.
    showNudge(usageMilestone)

    expect(callsTo("nudge_reveal")).toBe(1)
  })

  it("asks the crate to re-deliver anything emitted before the listener attached", async () => {
    render(<NudgeView />)

    // Resolved on the microtask after `listen` settles, so let it run.
    await act(async () => {})

    expect(callsTo("nudge_ready")).toBe(1)
  })

  it("collapses for the next nudge after an action click, even if the pointer never left", () => {
    const { container } = render(<NudgeView />)
    showNudge(usageMilestone)

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement)
    })
    expect(screen.getByRole("button", { name: "View session" })).toBeInTheDocument()

    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "View session" }))
    })

    // A new nudge replaces it under the still-resting cursor — no `mouseLeave`
    // ever fires — so it should start collapsed rather than inherit the
    // outgoing nudge's expanded state.
    showNudge(scanFailureNoRecs)

    expect(screen.getByText("Scan failed")).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Review sources" })).not.toBeInTheDocument()
  })

  it("sends a CTA target through to the shell", () => {
    const { container } = render(<NudgeView />)
    showNudge(targeted)

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement)
    })
    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "View session" }))
    })
    showNudge(scanFailureNoRecs)

    expect(invoke).toHaveBeenCalledWith("nudge_action", {
      kind: "usageMilestone",
      actionId: "view_session",
      target: { type: "session", agent: "claude", sessionId: "session-9", environment: null },
    })
  })

  it("sends the notification settings action directly to the shell", () => {
    const { container } = render(<NudgeView />)
    showNudge(usageMilestone)

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement)
    })
    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    })
    showNudge(scanFailureNoRecs)

    expect(invoke).toHaveBeenCalledWith("nudge_action", {
      kind: "usageMilestone",
      actionId: "notification_settings",
      target: undefined,
    })
  })

  it("runs the primary CTA when the notification body is clicked", () => {
    render(<NudgeView />)
    showNudge(usageMilestone)

    act(() => {
      fireEvent.click(screen.getByText(usageMilestone.title))
    })
    showNudge(scanFailureNoRecs)

    expect(invoke).toHaveBeenCalledWith("nudge_action", {
      kind: "usageMilestone",
      actionId: "view_session",
      target: undefined,
    })
  })

  it("opens About without installing when the update notification body is clicked", () => {
    render(<NudgeView />)
    showNudge(updateAvailable)

    act(() => {
      fireEvent.click(screen.getByText(updateAvailable.title))
    })
    showNudge(usageMilestone)

    expect(invoke).toHaveBeenCalledWith("nudge_action", {
      kind: "updateAvailable",
      actionId: "open_app",
      target: undefined,
    })
  })

  it("sends the exact update target when the Install button is clicked", () => {
    const { container } = render(<NudgeView />)
    showNudge(updateAvailable)

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement)
    })
    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "Install" }))
    })
    showNudge(usageMilestone)

    expect(invoke).toHaveBeenCalledWith("nudge_action", {
      kind: "updateAvailable",
      actionId: "install",
      target: { type: "update", expectedVersion: "2.4.0" },
    })
  })

  it("clicks through the body even while collapsed — no hover needed", () => {
    render(<NudgeView />)
    showNudge(scanFailureNoRecs)

    // No `mouseEnter`: the action bar isn't even rendered, yet the body still
    // resolves the nudge's primary CTA.
    expect(screen.queryByRole("button", { name: "Review sources" })).not.toBeInTheDocument()
    act(() => {
      fireEvent.click(screen.getByText("Scan failed"))
    })
    showNudge(usageMilestone)

    expect(invoke).toHaveBeenCalledWith("nudge_action", {
      kind: "scanFailure",
      actionId: "review_sources",
      target: undefined,
    })
  })

  it("just opens the app when the body is clicked on a nudge with no actions", () => {
    render(<NudgeView />)
    showNudge(actionlessNudge)

    act(() => {
      fireEvent.click(screen.getByText(actionlessNudge.title))
    })
    showNudge(usageMilestone)

    expect(invoke).toHaveBeenCalledWith("nudge_action", {
      kind: "test",
      actionId: "open_app",
      target: undefined,
    })
  })

  it("opens notification settings from the body when dismiss is primary", () => {
    render(<NudgeView />)
    showNudge(diskSpaceLow)

    act(() => {
      fireEvent.click(screen.getByText(diskSpaceLow.title))
    })
    showNudge(usageMilestone)

    expect(invoke).toHaveBeenCalledWith("nudge_action", {
      kind: "diskSpaceLow",
      actionId: "notification_settings",
      target: undefined,
    })
  })

  it("does not double-fire when a CTA button is clicked", () => {
    const { container } = render(<NudgeView />)
    showNudge(usageMilestone)

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement)
    })
    act(() => {
      // A second click during the exit animation must not arm a second action.
      fireEvent.click(screen.getByRole("button", { name: "View session" }))
      fireEvent.click(screen.getByRole("button", { name: "View session" }))
    })
    showNudge(scanFailureNoRecs)

    // The action bar is a sibling of the clickable body, so the button's click
    // never bubbles into it either.
    expect(callsTo("nudge_action")).toBe(1)
    expect(invoke).toHaveBeenCalledWith("nudge_action", {
      kind: "usageMilestone",
      actionId: "view_session",
      target: undefined,
    })
  })

  it("does not run an action when the close button is clicked", () => {
    const { container } = render(<NudgeView />)
    showNudge(usageMilestone)

    act(() => {
      fireEvent.mouseEnter(container.firstElementChild as HTMLElement)
    })
    act(() => {
      // A rapid double-click must not dismiss twice.
      fireEvent.click(screen.getByRole("button", { name: "Close" }))
      fireEvent.click(screen.getByRole("button", { name: "Close" }))
    })
    showNudge(scanFailureNoRecs)

    expect(callsTo("nudge_dismiss")).toBe(1)
    expect(callsTo("nudge_action")).toBe(0)
  })

  it("dismisses itself when the auto-dismiss timer elapses", () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<NudgeView />)
      showNudge(usageMilestone)

      act(() => {
        vi.advanceTimersByTime(usageMilestone.timeoutMs!)
      })
      act(() => {
        finishAnimation(notificationCard(container))
      })

      expect(callsTo("nudge_dismiss")).toBe(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("does not re-arm the auto-dismiss timer when the mouse leaves after an exit is armed", () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<NudgeView />)
      showNudge(usageMilestone)
      const wrapper = container.firstElementChild as HTMLElement

      // Hover pauses the timer with leftover time, close arms the exit, and the
      // subsequent mouse-leave must not resume() the dead nudge's timer.
      act(() => {
        fireEvent.mouseEnter(wrapper)
      })
      act(() => {
        fireEvent.click(screen.getByRole("button", { name: "Close" }))
      })
      act(() => {
        fireEvent.mouseLeave(wrapper)
        vi.advanceTimersByTime(usageMilestone.timeoutMs! * 2)
      })
      act(() => {
        finishAnimation(notificationCard(container))
      })

      expect(callsTo("nudge_dismiss")).toBe(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("does not re-arm the auto-dismiss timer when the mouse leaves during a timeout exit", () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<NudgeView />)
      showNudge(usageMilestone)
      const wrapper = container.firstElementChild as HTMLElement

      // The timeout fires while the pointer rests on the card, arming the exit.
      // Leaving mid-slide-out must not resume() the dead nudge's timer — that
      // would dismiss the same nudge a second time.
      act(() => {
        fireEvent.mouseEnter(wrapper)
      })
      act(() => {
        fireEvent.mouseLeave(wrapper)
        vi.advanceTimersByTime(usageMilestone.timeoutMs!)
      })
      act(() => {
        fireEvent.mouseEnter(wrapper)
      })
      act(() => {
        fireEvent.mouseLeave(wrapper)
        vi.advanceTimersByTime(usageMilestone.timeoutMs! * 2)
      })
      act(() => {
        finishAnimation(notificationCard(container))
      })

      expect(callsTo("nudge_dismiss")).toBe(1)
    } finally {
      vi.useRealTimers()
    }
  })
})

/**
 * The native hover signal.
 *
 * The notification never takes macOS key-window status on show, and AppKit
 * routes mouse-moved events to the key window — so while another antiburn
 * window is key, this window gets no pointer events at all. The crate samples
 * the cursor instead and reports crossings as `nudge:hover`; everything hover
 * drives must work from that signal alone.
 */
describe("NudgeView — native hover signal", () => {
  beforeEach(() => {
    invoke.mockReset()
    invoke.mockResolvedValue(null)
    listeners.clear()
  })

  it("expands and pauses auto-dismiss with no pointer events at all", () => {
    vi.useFakeTimers()
    try {
      render(<NudgeView />)
      showNudge(usageMilestone)

      nativeHover(true)
      expect(screen.getByText("Review recent usage")).toBeInTheDocument()
      expect(screen.getByRole("button", { name: "View session" })).toBeInTheDocument()
      expect(screen.getByRole("button", { name: "Close" }).className).toContain("opacity-100")

      // Auto-dismiss is paused for as long as the cursor rests on it.
      act(() => {
        vi.advanceTimersByTime(usageMilestone.timeoutMs! * 3)
      })
      expect(callsTo("nudge_dismiss")).toBe(0)

      nativeHover(false)
      expect(screen.queryByText("Review recent usage")).not.toBeInTheDocument()

      act(() => {
        vi.advanceTimersByTime(usageMilestone.timeoutMs!)
      })
      expect(callsTo("nudge_dismiss")).toBe(0)
      // The dismiss is deferred to the end of the exit animation.
      expect(document.querySelector(".animate-nudge-out")).not.toBeNull()
    } finally {
      vi.useRealTimers()
    }
  })

  it("does not ask for key-window status, but still releases it on leave", () => {
    render(<NudgeView />)
    showNudge(usageMilestone)

    nativeHover(true)
    // Acquiring key here would take it from the window the user is actually
    // working in — and buys nothing, since the pointer events CSS `:hover`
    // needs are exactly what isn't arriving.
    expect(hoverCommandArgs()).toEqual([])

    nativeHover(false)
    expect(hoverCommandArgs()).toEqual([false])
  })

  it("de-duplicates against the pointer, whichever edge lands first", () => {
    const { container } = render(<NudgeView />)
    showNudge(usageMilestone)
    const wrapper = container.firstElementChild as HTMLElement

    // Native sample first, pointer second: the pointer enter still requests key
    // (it proves mouse events are flowing) but must not re-expand or re-pause
    // anything.
    nativeHover(true)
    act(() => {
      fireEvent.mouseEnter(wrapper)
    })
    expect(hoverCommandArgs()).toEqual([true])
    expect(screen.getByRole("button", { name: "View session" })).toBeInTheDocument()

    // Pointer leave collapses once; the native sample catching up is a no-op.
    act(() => {
      fireEvent.mouseLeave(wrapper)
    })
    nativeHover(false)
    expect(hoverCommandArgs()).toEqual([true, false])
    expect(screen.queryByText("Review recent usage")).not.toBeInTheDocument()
  })
})
