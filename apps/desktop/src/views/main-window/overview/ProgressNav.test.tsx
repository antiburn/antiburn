import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { checkAvailabilityStore, emptyCheckAvailability } from "../../../lib/checkAvailability"
import { ProgressNav } from "./ProgressNav"
import type * as ProgressStore from "./overviewProgressStore"
import type { FlowStep, OverviewProgress } from "./overviewProgressStore"

let snapshot: OverviewProgress
const openProgressStep = vi.fn()
const closeProgressStep = vi.fn()
const rewindTo = vi.fn()

const openChecks = vi.fn()

// The modal's own IPC-heavy settings are covered by each step component's
// own tests; here a stand-in proves the modal renders them for the open
// step, without pulling Tauri-backed sessions into this file.
vi.mock("./stepSettings/StepSettings", () => ({
  StepSettings: ({ step }: { step: string }) => (
    <div data-testid="step-settings">
      {step}
      <input aria-label={`${step} setting`} />
    </div>
  ),
}))

vi.mock("./overviewProgressStore", async () => {
  const actual = await vi.importActual<typeof ProgressStore>("./overviewProgressStore")
  return {
    ...actual,
    subscribeOverviewProgress: () => () => undefined,
    overviewProgress: () => snapshot,
    openProgressStep: (step: "agents" | "sessions" | "checks" | "fixes") =>
      openProgressStep(step),
    closeProgressStep: () => closeProgressStep(),
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
    openStep: null,
    openStepControl: null,
    openStepControlRevision: 0,
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
    const { container } = render(<ProgressNav onOpenChecks={openChecks} />)
    expect(container).toBeEmptyDOMElement()
  })

  it("docks the Agents row after welcome and its card are complete", () => {
    snapshot = progress("welcome", "firstRun")
    const { container, rerender } = render(<ProgressNav onOpenChecks={openChecks} />)
    expect(container).toBeEmptyDOMElement()

    snapshot = progress("agents", "firstRun")
    rerender(<ProgressNav onOpenChecks={openChecks} />)
    expect(container).toBeEmptyDOMElement()

    snapshot = progress("limits", "firstRun")
    rerender(<ProgressNav onOpenChecks={openChecks} />)
    expect(screen.getByRole("button", { name: /Agents/ })).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /^Sessions/ })).toBeNull()
  })

  it("adds a row as each step docks, in order", () => {
    snapshot = progress("checks", "firstRun")
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(screen.getByRole("button", { name: /Agents/ })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /^Sessions/ })).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /^Checks/ })).toBeNull()
    expect(screen.queryByRole("button", { name: /^Fixes/ })).toBeNull()
  })

  it("shows all four rows once steady", () => {
    snapshot = progress("done", "steady")
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(screen.getByRole("button", { name: /Agents/ })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /^Sessions/ })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /^Checks/ })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /^Fixes/ })).toBeInTheDocument()
  })

  it("counts only selected checks that can currently run", () => {
    const selected = emptyCheckAvailability.checks.map((check) => ({
      ...check,
      enabled: check.id !== "ignoredInstructions",
    }))
    act(() =>
      checkAvailabilityStore.set({
        ...emptyCheckAvailability,
        revision: 1,
        checks: selected,
      }),
    )
    snapshot = progress("done", "steady")
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(navValue(/^Checks/)).toBe("9")
    act(() =>
      checkAvailabilityStore.set({
        ...emptyCheckAvailability,
        revision: 2,
        configured: true,
        checks: selected.map((check) => ({ ...check, enabled: true })),
      }),
    )
    expect(navValue(/^Checks/)).toBe("13")
    act(() =>
      checkAvailabilityStore.set({
        ...emptyCheckAvailability,
        revision: 3,
        configured: true,
        checks: selected.map((check) => ({
          ...check,
          enabled: check.id === "unusedSkills" ? false : check.enabled,
        })),
      }),
    )
    expect(navValue(/^Checks/)).toBe("11")
  })

  it("shows the failing count on the Fixes row", () => {
    snapshot = progress("done", "steady", {
      checks: { done: true, windowSessions: 0, pendingEvidence: 0, deferredEvidence: 0 },
      failingCount: 3,
    })
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(navValue(/^Fixes/)).toBe("3")
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
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(navValue(/^Sessions/)).toBe("114")
    expect(navPulsing(/^Sessions/)).toBe(true)
  })

  it("shows the combined completed figure, not pulsing, once the 30-day read is done and the history pass is idle", () => {
    snapshot = sessionsProgress()
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(navValue(/^Sessions/)).toBe("114")
    expect(navPulsing(/^Sessions/)).toBe(false)
  })

  it("climbs with the history pass's completed count, and pulses, while reading", () => {
    snapshot = sessionsProgress(
      { displayCompleted: 432 },
      { state: "reading", completed: 318, total: 318 },
    )
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(navValue(/^Sessions/)).toBe("432")
    expect(navPulsing(/^Sessions/)).toBe(true)
  })

  it("pulses while looking for older sessions", () => {
    snapshot = sessionsProgress({}, { state: "looking", completed: 0, total: 0 })
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(navPulsing(/^Sessions/)).toBe(true)
  })

  it("stops pulsing once the history pass is done", () => {
    snapshot = sessionsProgress(
      { displayCompleted: 432 },
      { state: "done", completed: 318, total: 318 },
    )
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(navValue(/^Sessions/)).toBe("432")
    expect(navPulsing(/^Sessions/)).toBe(false)
  })
})

describe("ProgressNav's Sessions step card", () => {
  function openSessions(history: OverviewProgress["history"]): void {
    snapshot = progress("done", "steady", {
      openStep: "sessions",
      sessions: {
        done: true,
        completed: 114,
        total: 114,
        displayCompleted: 432,
        displayTotal: 432,
        deferred: [],
      },
      history,
    })
    render(<ProgressNav onOpenChecks={openChecks} />)
  }

  it("shows the combined completed/total on the progress bar", () => {
    openSessions({ state: "reading", completed: 318, total: 400 })
    const values = screen.getByRole("dialog").querySelectorAll("[data-count-up-value]")
    expect(Array.from(values, (node) => node.textContent).join("/")).toBe("432/432")
  })

  it("shows a pending footnote", () => {
    openSessions({ state: "pending", completed: 0, total: 0 })
    expect(screen.getByText("Older sessions are read once checks finish.")).toBeInTheDocument()
  })

  it("shows a looking footnote", () => {
    openSessions({ state: "looking", completed: 0, total: 0 })
    expect(screen.getByText("Looking for older sessions…")).toBeInTheDocument()
  })

  it("shows a reading footnote with the history pass's own numbers, not the combined ones", () => {
    openSessions({ state: "reading", completed: 318, total: 400 })
    expect(screen.getByText("Reading older sessions · 318 of 400")).toBeInTheDocument()
  })

  it("shows no footnote once the history pass is done", () => {
    openSessions({ state: "done", completed: 400, total: 400 })
    expect(
      screen.queryByText(
        /Reading older sessions|Looking for older sessions|Older sessions are read/,
      ),
    ).toBeNull()
  })

  it("shows no footnote when there is no history to report", () => {
    openSessions(null)
    expect(screen.queryByText(/older session/i)).toBeNull()
  })
})

describe("ProgressNav's Fixes step card", () => {
  it("no longer shows a history fine-print line", () => {
    snapshot = progress("done", "steady", {
      openStep: "fixes",
      history: { state: "reading", completed: 100, total: 200 },
    })
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(screen.queryByText(/Reading older history/)).toBeNull()
  })
})

describe("ProgressNav's rewind", () => {
  it("takes the first run back to a row's step instead of opening its modal", () => {
    snapshot = progress("fixes", "firstRun")
    render(<ProgressNav onOpenChecks={openChecks} />)
    const row = screen.getByRole("button", { name: /^Sessions/ })
    expect(row).not.toHaveAttribute("aria-haspopup")
    fireEvent.click(row)
    expect(rewindTo).toHaveBeenCalledWith("sessions")
    expect(openProgressStep).not.toHaveBeenCalled()
  })

  it("opens the modal again once the first run is done", () => {
    snapshot = progress("done", "firstRun")
    render(<ProgressNav onOpenChecks={openChecks} />)
    fireEvent.click(screen.getByRole("button", { name: /^Sessions/ }))
    expect(openProgressStep).toHaveBeenCalledWith("sessions")
    expect(rewindTo).not.toHaveBeenCalled()
  })
})

describe("ProgressNav's modal", () => {
  it("releases a compact navigation owner before opening the modal", () => {
    const onActivate = vi.fn()
    snapshot = progress("done", "steady")
    render(<ProgressNav onActivate={onActivate} onOpenChecks={openChecks} />)

    fireEvent.click(screen.getByRole("button", { name: /^Checks/ }))

    expect(onActivate).toHaveBeenCalledOnce()
    expect(openProgressStep).toHaveBeenCalledWith("checks")
  })

  it("closes the fixes modal on Enhance and opens the first check that needs a fix", () => {
    snapshot = progress("done", "steady", {
      checks: { done: true, windowSessions: 5, pendingEvidence: 0, deferredEvidence: 0 },
      failingCount: 2,
      categories: [
        { id: "cacheChurn", label: "Excess cache rehydration", status: "passing" },
        { id: "unusedSkills", label: "Unused skills", status: "needsFix" },
        { id: "unusedMcpServers", label: "Unused MCP servers", status: "needsFix" },
      ],
      openStep: "fixes",
    })
    render(<ProgressNav onOpenChecks={openChecks} />)
    fireEvent.click(screen.getByRole("button", { name: "Enhance" }))
    expect(closeProgressStep).toHaveBeenCalled()
    expect(openChecks).toHaveBeenCalledWith("unusedSkills")
  })

  it("opens a step's modal on click, and shows it as selected", () => {
    snapshot = progress("done", "steady")
    render(<ProgressNav onOpenChecks={openChecks} />)
    const row = screen.getByRole("button", { name: /Agents/ })
    fireEvent.click(row)
    expect(openProgressStep).toHaveBeenCalledWith("agents")
  })

  it("renders the open step's card as a labelled dialog", () => {
    snapshot = progress("done", "steady", { openStep: "agents" })
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(screen.getByRole("dialog", { name: "Agents" })).toHaveClass("bg-surface-overlay")
    expect(screen.getByRole("heading", { name: "Agents", level: 2 })).toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "Finding agents" })).toBeNull()
  })

  it("keeps keyboard focus inside the modal's inputs and buttons", () => {
    snapshot = progress("done", "steady", { openStep: "checks" })
    render(<ProgressNav onOpenChecks={openChecks} />)
    const dialog = screen.getByRole("dialog")
    const input = screen.getByRole("textbox", { name: "checks setting" })
    const close = screen.getByRole("button", { name: "Close" })

    input.focus()
    fireEvent.keyDown(dialog, { key: "Tab", shiftKey: true })
    expect(close).toHaveFocus()
    fireEvent.keyDown(dialog, { key: "Tab" })
    expect(input).toHaveFocus()
  })

  it("shows the step's own settings below the summary, open by default", () => {
    snapshot = progress("done", "steady", { openStep: "sessions" })
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(screen.getByTestId("step-settings")).toHaveTextContent("sessions")
  })

  it("closes on Escape and returns focus to the row that opened it", () => {
    snapshot = progress("done", "steady", { openStep: "agents" })
    render(<ProgressNav onOpenChecks={openChecks} />)
    const row = screen.getByRole("button", { name: /Agents/ })
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" })
    expect(closeProgressStep).toHaveBeenCalledTimes(1)
    return vi.waitFor(() => expect(document.activeElement).toBe(row))
  })

  it("closes on a backdrop click", () => {
    snapshot = progress("done", "steady", { openStep: "agents" })
    render(<ProgressNav onOpenChecks={openChecks} />)
    const dialog = screen.getByRole("dialog")
    fireEvent.mouseDown(dialog.parentElement!)
    expect(closeProgressStep).toHaveBeenCalledTimes(1)
  })

  it("renders no modal when no step is open", () => {
    snapshot = progress("done", "steady", { openStep: null })
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(screen.queryByRole("dialog")).toBeNull()
  })
})
