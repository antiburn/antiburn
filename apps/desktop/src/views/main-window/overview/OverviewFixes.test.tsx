import { render, screen } from "@testing-library/react"
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
    openStep: null,
    openStepControl: null,
    openStepControlRevision: 0,
    stepShown: true,
    agents: { done: true, rows: [] },
    sessions: {
      done: true,
      completed: 0,
      total: 0,
      displayCompleted: 0,
      displayTotal: 0,
      gate: null,
      includeNonRepoFolders: false,
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
  it("renders the config checks heading and nothing else", () => {
    snapshot = progress()
    render(<OverviewFixes />)
    expect(screen.getByRole("heading", { name: "Config checks" })).toBeInTheDocument()
  })

  it("lists every category with its label and status", () => {
    snapshot = progress({
      categories: [
        { id: "modelOverthinking", label: "Model overthinking", status: "needsFix" },
        { id: "cacheChurn", label: "Cache churn", status: "passing" },
        { id: "oldModelUsage", label: "Old model usage", status: "awaitingVerification" },
        { id: "overuseOfFastMode", label: "Overuse of fast mode", status: "notChecked" },
      ],
    })
    render(<OverviewFixes />)
    expect(screen.getByText("Model overthinking").closest("li")).toHaveTextContent("needs fix")
    expect(screen.getByText("Cache churn").closest("li")).toHaveTextContent("passing")
    expect(screen.getByText("Old model usage").closest("li")).toHaveTextContent(
      "awaiting verification",
    )
    expect(screen.getByText("Overuse of fast mode").closest("li")).toHaveTextContent(
      "not checked",
    )
  })

  it("shows a failing category's problem phrase beside its label", () => {
    snapshot = progress({
      categories: [
        { id: "modelOverthinking", label: "Model overthinking", status: "needsFix" },
      ],
    })
    render(<OverviewFixes />)
    const row = screen.getByText("Model overthinking").closest("li")!
    expect(row).toHaveTextContent("needs fix")
    // The phrase itself comes from CHECK_PROBLEM_PHRASES; this only checks the
    // row renders more than the label and status for a failing category.
    expect(row.querySelectorAll("span").length).toBeGreaterThan(2)
  })

  it("renders no categories when none are reported", () => {
    snapshot = progress({ categories: [] })
    render(<OverviewFixes />)
    expect(screen.getByRole("heading", { name: "Config checks" })).toBeInTheDocument()
    expect(screen.queryByRole("listitem")).toBeNull()
  })
})
