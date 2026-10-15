import { fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { OverviewFixes } from "./OverviewFixes"
import type { OverviewProgress } from "./overviewProgressStore"

let snapshot: OverviewProgress

vi.mock("./overviewProgressStore", () => ({
  subscribeOverviewProgress: () => () => undefined,
  overviewProgress: () => snapshot,
}))

afterEach(() => vi.clearAllMocks())

function progress(overrides: Partial<OverviewProgress> = {}): OverviewProgress {
  return {
    mode: "steady",
    flow: "done",
    stepShown: true,
    actionPending: false,
    actionError: null,
    agents: { done: true, rows: [] },
    sessions: {
      done: true,
      completed: 0,
      total: 0,
      displayCompleted: 0,
      displayTotal: 0,
      deferred: [],
    },
    checks: { done: true, windowSessions: 0, pendingEvidence: 0, deferredEvidence: 0 },
    categories: [],
    failingCount: 0,
    history: null,
    ...overrides,
  }
}

describe("OverviewFixes", () => {
  it("opens the check when its row is clicked", () => {
    snapshot = progress({
      categories: [
        {
          id: "modelOverthinking",
          label: "Model overthinking",
          status: "needsFix",
          estimatedBurnBasisPoints: null,
          finding: 0,
          clean: 0,
          agents: [],
        },
      ],
    })
    const onOpenCheck = vi.fn()
    render(<OverviewFixes onOpenCheck={onOpenCheck} />)
    fireEvent.click(screen.getByRole("button", { name: /Model overthinking/ }))
    expect(onOpenCheck).toHaveBeenCalledWith("modelOverthinking")
  })

  it("opens all checks from the section heading", () => {
    snapshot = progress()
    const onOpenCheck = vi.fn()
    render(<OverviewFixes onOpenCheck={onOpenCheck} />)
    expect(screen.getByRole("heading", { name: "Config checks" })).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "All checks" }))
    expect(onOpenCheck).toHaveBeenCalledWith(undefined)
  })

  it("lists every category with its label and status", () => {
    snapshot = progress({
      categories: [
        {
          id: "modelOverthinking",
          label: "Model overthinking",
          status: "needsFix",
          estimatedBurnBasisPoints: null,
          finding: 0,
          clean: 0,
          agents: [],
        },
        {
          id: "cacheChurn",
          label: "Cache churn",
          status: "passing",
          estimatedBurnBasisPoints: null,
          finding: 0,
          clean: 0,
          agents: [],
        },
        {
          id: "oldModelUsage",
          label: "Old model usage",
          status: "awaitingVerification",
          estimatedBurnBasisPoints: null,
          finding: 0,
          clean: 0,
          agents: [],
        },
        {
          id: "overuseOfFastMode",
          label: "Overuse of fast mode",
          status: "notChecked",
          estimatedBurnBasisPoints: null,
          finding: 0,
          clean: 0,
          agents: [],
        },
      ],
    })
    render(<OverviewFixes onOpenCheck={() => {}} />)
    expect(screen.getByText("Model overthinking").closest("li")).toHaveTextContent("0 failed")
    expect(screen.getByText("Cache churn").closest("li")).toHaveTextContent("Passed")
    expect(screen.getByText("Old model usage").closest("li")).toHaveTextContent(
      "Awaiting verification",
    )
    expect(screen.getByText("Overuse of fast mode").closest("li")).toHaveTextContent(
      "Not checked",
    )
    // Failing checks lead the grid; passing checks come last.
    expect(screen.getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      expect.stringContaining("Model overthinking"),
      expect.stringContaining("Old model usage"),
      expect.stringContaining("Overuse of fast mode"),
      expect.stringContaining("Cache churn"),
    ])
  })

  it("shows a failing card's counts and estimated burn", () => {
    snapshot = progress({
      categories: [
        {
          id: "modelOverthinking",
          label: "Model overthinking",
          status: "needsFix",
          estimatedBurnBasisPoints: 180,
          finding: 3,
          clean: 5,
          agents: [],
        },
      ],
    })
    render(<OverviewFixes onOpenCheck={() => {}} />)
    const card = screen.getByText("Model overthinking").closest("li")!
    expect(card).toHaveTextContent("3 failed")
    expect(card).toHaveTextContent("5 passed")
    expect(card).toHaveTextContent("estimated burn")
  })

  it("renders no categories when none are reported", () => {
    snapshot = progress({ categories: [] })
    render(<OverviewFixes onOpenCheck={() => {}} />)
    expect(screen.getByRole("heading", { name: "Config checks" })).toBeInTheDocument()
    expect(screen.queryByRole("listitem")).toBeNull()
  })

  it("shows the list when every check passes or is snoozed", () => {
    snapshot = progress({
      categories: [
        {
          id: "modelOverthinking",
          label: "Model overthinking",
          status: "passing",
          estimatedBurnBasisPoints: null,
          finding: 0,
          clean: 0,
          agents: [],
        },
        {
          id: "unusedSkills",
          label: "Unused skills",
          status: "snoozed",
          estimatedBurnBasisPoints: null,
          finding: 0,
          clean: 0,
          agents: [],
        },
      ],
    } as Partial<OverviewProgress>)
    render(<OverviewFixes onOpenCheck={() => {}} />)
    expect(screen.getByRole("heading", { name: "Config checks" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /Model overthinking/ })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /Unused skills/ })).toBeInTheDocument()
  })
})
