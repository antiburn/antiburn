import { render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { OverviewFixes } from "./OverviewFixes"
import type { FtueSnapshot } from "./ftueStore"

let snapshot: FtueSnapshot

vi.mock("./ftueStore", () => ({
  subscribeFtue: () => () => undefined,
  ftueSnapshot: () => snapshot,
  dismissFtueCallout: vi.fn(),
  enableNonRepoFolders: vi.fn(),
}))

function ftue(overrides: Partial<FtueSnapshot> = {}): FtueSnapshot {
  return {
    showSteps: true,
    find: { done: true, rows: [{ agent: "Claude Code", sessions: 49 }] },
    read: { done: false, completed: 0, total: 0, gate: null, includeNonRepoFolders: false },
    check: { done: false, windowSessions: 0, pendingEvidence: 0 },
    categories: [],
    failingCount: 0,
    history: null,
    dismissed: false,
    ...overrides,
  }
}

describe("OverviewFixes's pending steps", () => {
  it("shows Waiting, not 0 of 0, for a read step discovery has not sized yet", () => {
    snapshot = ftue({
      read: { done: false, completed: 0, total: 0, gate: null, includeNonRepoFolders: false },
    })
    render(<OverviewFixes />)
    expect(screen.getByText("Read sessions").parentElement).toHaveTextContent("Waiting")
    expect(screen.queryByText("0 of 0")).not.toBeInTheDocument()
  })

  it("shows real numbers for a read step already under way", () => {
    snapshot = ftue({
      read: { done: false, completed: 12, total: 50, gate: null, includeNonRepoFolders: false },
    })
    render(<OverviewFixes />)
    expect(screen.getByText("12 of 50")).toBeInTheDocument()
  })

  it("keeps the check step at Waiting until the read step finishes, even with a settled report", () => {
    // Mirrors the reported bug: the checks report can settle (142 checked)
    // while discovery is still mid-pass. The check step must not read as
    // done, or even as counting up, before step 2 does.
    snapshot = ftue({
      read: { done: false, completed: 0, total: 0, gate: null, includeNonRepoFolders: false },
      check: { done: true, windowSessions: 142, pendingEvidence: 0 },
    })
    render(<OverviewFixes />)
    expect(screen.getByText("Check sessions").parentElement).toHaveTextContent("Waiting")
    expect(screen.queryByText("Checked 142")).not.toBeInTheDocument()
  })

  it("shows the check step done once both it and the read step finish", () => {
    snapshot = ftue({
      read: { done: true, completed: 49, total: 49, gate: null, includeNonRepoFolders: false },
      check: { done: true, windowSessions: 142, pendingEvidence: 0 },
    })
    render(<OverviewFixes />)
    expect(screen.getByText("Checked 142")).toBeInTheDocument()
  })
})
