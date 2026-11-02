import { fireEvent, render, screen, within } from "@testing-library/react"
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
  it.each([1, 2, undefined])(
    "keeps an installed check not assessed with %s running sessions and no saved result",
    (checkingCount) => {
      snapshot = progress({
        categories: [
          {
            id: "installed:retry",
            installedAvailability: { status: "available" },
            label: "Failed retry loop",
            status: "checking",
            lifecycle: null,
            finding: 0,
            clean: 0,
            checking: true,
            reviewFailed: 1,
            ...(checkingCount == null ? {} : { checkingCount }),
            estimatedBurnBasisPoints: null,
            agents: [],
          },
        ],
      })
      render(<OverviewFixes onOpenCheck={() => {}} />)
      const card = screen.getByRole("button", {
        name: "Failed retry loop, Not assessed · Checking",
      })
      expect(within(card).getByText("Checking")).toBeVisible()
      expect(within(card).getByText("Not assessed")).toBeVisible()
      expect(card).not.toHaveTextContent("Passed")
    },
  )

  it.each([
    { catchingUp: false, count: 3, waitingForDataCount: 0 },
    { catchingUp: true, count: 3, waitingForDataCount: 1 },
  ])("keeps unassessed waiting and catch-up details off the card: %j", (waiting) => {
    snapshot = progress({
      categories: [
        {
          id: "installed:retry",
          installedAvailability: { status: "available" },
          label: "Failed retry loop",
          status: "waiting",
          lifecycle: null,
          finding: 0,
          clean: 0,
          hasStarted: true,
          checking: false,
          waitingForDataCount: waiting.waitingForDataCount,
          waitingForEvents: {
            event: "toolResult",
            count: waiting.count,
            catchingUp: waiting.catchingUp,
          },
          estimatedBurnBasisPoints: null,
          agents: [],
        },
      ],
    })
    render(<OverviewFixes onOpenCheck={() => {}} />)
    const card = screen.getByRole("button", { name: "Failed retry loop, Not assessed" })
    expect(card).not.toHaveTextContent(/Passed|Checking|Waiting on|Catching up/)
  })

  it.each(["paused", "invalid", "unsupported"] as const)(
    "keeps an installed %s error not assessed without active checking",
    (availability) => {
      snapshot = progress({
        categories: [
          {
            id: "installed:retry",
            installedAvailability: { status: availability },
            label: "Failed retry loop",
            status: "error",
            finding: 0,
            clean: 0,
            checking: true,
            reviewFailed: 1,
            estimatedBurnBasisPoints: null,
            agents: [],
          },
        ],
      })
      render(<OverviewFixes onOpenCheck={() => {}} />)
      const card = screen.getByRole("button", { name: "Failed retry loop, Not assessed" })
      expect(card).not.toHaveTextContent(/Connection needed|Could not check|Checking|Passed/)
    },
  )

  it.each([
    { lifecycle: "failing" as const, finding: 2, clean: 1, result: "2 failed · 1 passed" },
    { lifecycle: "passing" as const, finding: 0, clean: 2, result: "Passed" },
  ])(
    "shows Snoozed visibly and accessibly with a retained $lifecycle result",
    ({ lifecycle, finding, clean }) => {
      snapshot = progress({
        categories: [
          {
            id: "installed:retry",
            installedAvailability: { status: "available" },
            label: "Failed retry loop",
            status: "snoozed",
            lifecycle,
            finding,
            clean,
            checking: true,
            checkingCount: 1,
            reviewedWithoutStatus: 1,
            waitingForDataCount: 0,
            waitingForResultsCount: 2,
            waitingForEvents: { event: "toolResult", count: 1, catchingUp: false },
            estimatedBurnBasisPoints: null,
            agents: [],
          },
        ],
      })
      render(<OverviewFixes onOpenCheck={() => {}} />)
      const status = "Snoozed"
      const card = screen.getByRole("button", { name: `Failed retry loop, ${status}` })
      expect(card).toHaveTextContent(status)
      expect(within(card).getByText("Snoozed")).toBeVisible()
      expect(within(card).queryByText(/checking/i)).not.toBeInTheDocument()
    },
  )

  it.each([
    {
      text: "Passed · Checking",
      checking: true,
      reviewFailed: 1,
      waitingForDataCount: 1,
    },
    {
      text: "Passed",
      checking: false,
      reviewFailed: 1,
      waitingForDataCount: 1,
    },
    { text: "Passed", checking: false, reviewFailed: 0, waitingForDataCount: 1 },
    {
      text: "Passed",
      checking: false,
      reviewFailed: 0,
      waitingForDataCount: 0,
    },
  ])("shows $text with the same accessible status text", ({ text, ...facts }) => {
    snapshot = progress({
      categories: [
        {
          id: "installed:retry",
          installedAvailability: { status: "available" },
          label: "Failed retry loop",
          status: "passing",
          lifecycle: "passing",
          finding: 0,
          clean: 1,
          checkingCount: 2,
          ...facts,
          waitingForResultsCount: 1,
          waitingForEvents: { event: "toolResult", count: 1, catchingUp: false },
          estimatedBurnBasisPoints: null,
          agents: [],
        },
      ],
    })
    render(<OverviewFixes onOpenCheck={() => {}} />)
    expect(
      screen.getByRole("button", { name: `Failed retry loop, ${text}` }),
    ).toHaveTextContent(text)
  })
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
    expect(screen.getByRole("heading", { name: "Checks" })).toBeInTheDocument()
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
    expect(screen.getByText("Model overthinking").closest("li")).toHaveTextContent(
      "Not assessed",
    )
    expect(screen.getByText("Cache churn").closest("li")).toHaveTextContent("Passed")
    expect(screen.getByText("Old model usage").closest("li")).toHaveTextContent("Verifying")
    expect(screen.getByText("Overuse of fast mode").closest("li")).toHaveTextContent(
      "Not assessed",
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
    expect(screen.getByRole("heading", { name: "Checks" })).toBeInTheDocument()
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
    expect(screen.getByRole("heading", { name: "Checks" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /Model overthinking/ })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /Unused skills/ })).toBeInTheDocument()
  })
})
