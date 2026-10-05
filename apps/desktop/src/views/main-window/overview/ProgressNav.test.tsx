import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { checksConfiguredStore } from "../../../lib/checkAvailability"
import { ProgressNav } from "./ProgressNav"
import type { FlowStep, OverviewProgress, ProgressStepKey } from "./overviewProgressStore"

let snapshot: OverviewProgress
const openProgressStep = vi.fn()
const closeProgressStep = vi.fn()
const rewindTo = vi.fn()

const FLOW_ORDER: readonly FlowStep[] = [
  "limits",
  "agents",
  "sessions",
  "checks",
  "fixes",
  "done",
]
const STEP_DOCKED_AT: Record<ProgressStepKey, FlowStep> = {
  agents: "sessions",
  sessions: "checks",
  checks: "fixes",
  fixes: "done",
}

// Mirrors `stepDocked` in `overviewProgressStore.ts` exactly: a pure index
// comparison, safe to inline rather than load the real store module.
function stepDocked(flow: FlowStep, step: ProgressStepKey): boolean {
  return FLOW_ORDER.indexOf(flow) >= FLOW_ORDER.indexOf(STEP_DOCKED_AT[step])
}

const openChecks = vi.fn()

vi.mock("./overviewProgressStore", () => ({
  subscribeOverviewProgress: () => () => undefined,
  overviewProgress: () => snapshot,
  openProgressStep: (step: ProgressStepKey) => openProgressStep(step),
  closeProgressStep: () => closeProgressStep(),
  rewindTo: (step: ProgressStepKey) => rewindTo(step),
  stepDocked,
  progressStepTransitionName: (step: string) => `progress-step-${step}`,
  enableNonRepoFolders: vi.fn(),
  fixesFound: (progress: OverviewProgress) =>
    progress.checks.windowSessions > 0 && progress.failingCount > 0,
  firstFailingCheck: (progress: OverviewProgress) =>
    progress.categories.find((category) => category.status === "needsFix")?.id,
}))

afterEach(() => vi.clearAllMocks())

function progress(
  flow: FlowStep,
  mode: OverviewProgress["mode"],
  overrides: Partial<OverviewProgress> = {},
): OverviewProgress {
  return {
    mode,
    flow,
    openStep: null,
    stepShown: true,
    agents: {
      done: true,
      rows: [{ agent: "claude-code", label: "Claude Code", sessions: 7, done: true }],
    },
    sessions: {
      done: true,
      completed: 7,
      total: 7,
      gate: null,
      includeNonRepoFolders: false,
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

  it("renders no rows before the first step has docked", () => {
    snapshot = progress("agents", "firstRun")
    const { container } = render(<ProgressNav onOpenChecks={openChecks} />)
    expect(container).toBeEmptyDOMElement()
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

  it("counts the checks that run, with Ignored Instructions only once it is set up", () => {
    snapshot = progress("done", "steady")
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(screen.getByRole("button", { name: /^Checks/ })).toHaveTextContent(/^Checks9$/)
    act(() => checksConfiguredStore.set(true))
    expect(screen.getByRole("button", { name: /^Checks/ })).toHaveTextContent(/^Checks10$/)
    act(() => checksConfiguredStore.set(false))
  })

  it("shows 'No recent sessions' on the Fixes row when the window has none", () => {
    snapshot = progress("done", "steady", {
      checks: { done: true, windowSessions: 0, pendingEvidence: 0, deferredEvidence: 0 },
    })
    render(<ProgressNav onOpenChecks={openChecks} />)
    expect(screen.getByRole("button", { name: /^Fixes/ })).toHaveTextContent(
      "No recent sessions",
    )
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
    expect(screen.getByRole("dialog", { name: "Agents" })).toBeInTheDocument()
    expect(screen.getByRole("heading", { name: "Finding agents" })).toBeInTheDocument()
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
