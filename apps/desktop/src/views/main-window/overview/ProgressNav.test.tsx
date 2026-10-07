import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { checksConfiguredStore } from "../../../lib/checkAvailability"
import { ProgressNav } from "./ProgressNav"
import type * as ProgressStore from "./overviewProgressStore"
import type { FlowStep, OverviewProgress } from "./overviewProgressStore"

let snapshot: OverviewProgress
const rewindTo = vi.fn()

const openSettings = vi.fn()
const openFixes = vi.fn()

function nav() {
  return <ProgressNav onOpenSettings={openSettings} onOpenFixes={openFixes} />
}

vi.mock("./overviewProgressStore", async () => {
  const actual = await vi.importActual<typeof ProgressStore>("./overviewProgressStore")
  return {
    ...actual,
    subscribeOverviewProgress: () => () => undefined,
    overviewProgress: () => snapshot,
    rewindTo: (step: "agents" | "sessions" | "checks" | "fixes") => rewindTo(step),
  }
})

afterEach(() => vi.clearAllMocks())

/** A nav row's value, read from CountUp's accessible target value rather
 *  than the number it shows mid-count. */
function navValue(label: RegExp): string {
  const row = screen.getByRole("button", { name: label })
  return row.querySelector("[data-count-up-value]")?.textContent ?? ""
}

/** Whether a nav row's value is in its pulsing (still-working) state. */
function navPulsing(label: RegExp): boolean {
  const row = screen.getByRole("button", { name: label })
  return row.querySelector(".font-mono")?.classList.contains("animate-pulse") ?? false
}

function progress(
  flow: FlowStep,
  mode: OverviewProgress["mode"],
  overrides: Partial<OverviewProgress> = {},
): OverviewProgress {
  return {
    mode,
    flow,
    actionPending: false,
    actionError: null,
    stepShown: true,
    agents: {
      done: true,
      rows: [{ agent: "claude-code", label: "Claude Code", sessions: 7, done: true }],
    },
    sessions: {
      done: true,
      completed: 7,
      total: 7,
      displayCompleted: 7,
      displayTotal: 7,
      deferred: [],
    },
    checks: { done: true, windowSessions: 7, pendingEvidence: 0, deferredEvidence: 0 },
    categories: [],
    failingCount: 0,
    history: null,
    ...overrides,
  }
}

describe("ProgressNav's row visibility", () => {
  it("renders nothing while the first-run decision is still pending", () => {
    snapshot = progress("limits", "pending")
    const { container } = render(nav())
    expect(container).toBeEmptyDOMElement()
  })

  it("docks the Agents row after welcome and its card are complete", () => {
    snapshot = progress("welcome", "firstRun")
    const { container, rerender } = render(nav())
    expect(container).toBeEmptyDOMElement()

    snapshot = progress("agents", "firstRun")
    rerender(nav())
    expect(container).toBeEmptyDOMElement()

    snapshot = progress("limits", "firstRun")
    rerender(nav())
    expect(screen.getByRole("button", { name: /Agents/ })).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /^Sessions/ })).toBeNull()
  })

  it("adds a row as each step docks, in order", () => {
    snapshot = progress("checks", "firstRun")
    render(nav())
    expect(screen.getByRole("button", { name: /Agents/ })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /^Sessions/ })).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /^Checks/ })).toBeNull()
    expect(screen.queryByRole("button", { name: /^To fix/ })).toBeNull()
  })

  it("shows all four rows once steady", () => {
    snapshot = progress("done", "steady")
    render(nav())
    expect(screen.getByRole("button", { name: /Agents/ })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /^Sessions/ })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /^Checks/ })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /^To fix/ })).toBeInTheDocument()
  })

  it("counts the checks that run, with Ignored Instructions only once it is set up", () => {
    snapshot = progress("done", "steady")
    render(nav())
    expect(navValue(/^Checks/)).toBe("9")
    act(() => checksConfiguredStore.set(true))
    expect(navValue(/^Checks/)).toBe("10")
    act(() => checksConfiguredStore.set(false))
  })

  it("shows the failing count on the To fix row", () => {
    snapshot = progress("done", "steady", {
      checks: { done: true, windowSessions: 0, pendingEvidence: 0, deferredEvidence: 0 },
      failingCount: 3,
    })
    render(nav())
    expect(navValue(/^To fix/)).toBe("3")
  })
})

describe("ProgressNav's Sessions row and the history pass", () => {
  function sessionsProgress(
    overrides: Partial<OverviewProgress["sessions"]> = {},
    history: OverviewProgress["history"] = null,
  ): OverviewProgress {
    return progress("done", "steady", {
      sessions: {
        done: true,
        completed: 114,
        total: 114,
        displayCompleted: 114,
        displayTotal: 114,
        deferred: [],
        ...overrides,
      },
      history,
    })
  }

  it("shows the 30-day total, pulsing, before the 30-day read is done", () => {
    snapshot = sessionsProgress({
      done: false,
      completed: 50,
      total: 114,
      displayCompleted: 50,
      displayTotal: 114,
    })
    render(nav())
    expect(navValue(/^Sessions/)).toBe("114")
    expect(navPulsing(/^Sessions/)).toBe(true)
  })

  it("shows the combined completed figure, not pulsing, once the 30-day read is done and the history pass is idle", () => {
    snapshot = sessionsProgress()
    render(nav())
    expect(navValue(/^Sessions/)).toBe("114")
    expect(navPulsing(/^Sessions/)).toBe(false)
  })

  it("climbs with the history pass's completed count, and pulses, while reading", () => {
    snapshot = sessionsProgress(
      { displayCompleted: 432 },
      { state: "reading", completed: 318, total: 318 },
    )
    render(nav())
    expect(navValue(/^Sessions/)).toBe("432")
    expect(navPulsing(/^Sessions/)).toBe(true)
  })

  it("pulses while looking for older sessions", () => {
    snapshot = sessionsProgress({}, { state: "looking", completed: 0, total: 0 })
    render(nav())
    expect(navPulsing(/^Sessions/)).toBe(true)
  })

  it("stops pulsing once the history pass is done", () => {
    snapshot = sessionsProgress(
      { displayCompleted: 432 },
      { state: "done", completed: 318, total: 318 },
    )
    render(nav())
    expect(navValue(/^Sessions/)).toBe("432")
    expect(navPulsing(/^Sessions/)).toBe(false)
  })
})

describe("ProgressNav's rewind", () => {
  it("takes the first run back to a pill's step instead of opening Settings", () => {
    snapshot = progress("fixes", "firstRun")
    render(nav())
    fireEvent.click(screen.getByRole("button", { name: /^Sessions/ }))
    expect(rewindTo).toHaveBeenCalledWith("sessions")
    expect(openSettings).not.toHaveBeenCalled()
  })

  it("opens Settings again once the first run is done", () => {
    snapshot = progress("done", "firstRun")
    render(nav())
    fireEvent.click(screen.getByRole("button", { name: /^Sessions/ }))
    expect(openSettings).toHaveBeenCalledWith("sessions")
    expect(rewindTo).not.toHaveBeenCalled()
  })
})

describe("ProgressNav's steady destinations", () => {
  it.each([
    [/Agents/, "agents"],
    [/^Sessions/, "sessions"],
    [/^Checks/, "checks"],
  ] as const)("opens the %s pill's Settings pane", (label, step) => {
    snapshot = progress("done", "steady")
    render(nav())
    fireEvent.click(screen.getByRole("button", { name: label }))
    expect(openSettings).toHaveBeenCalledExactlyOnceWith(step)
    expect(openFixes).not.toHaveBeenCalled()
    expect(rewindTo).not.toHaveBeenCalled()
  })

  it("opens Checks from the To fix pill, with no check when none fails", () => {
    snapshot = progress("done", "steady")
    render(nav())
    fireEvent.click(screen.getByRole("button", { name: /^To fix/ }))
    expect(openFixes).toHaveBeenCalledExactlyOnceWith(undefined)
    expect(openSettings).not.toHaveBeenCalled()
  })
})
