import { fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { noteInteraction, requestFolderAccess, scanNow } from "../../../lib/ipc"
import type * as IpcModule from "../../../lib/ipc"
import { OverviewFixes } from "./OverviewFixes"
import { openFixes, openSteps, shrinkFixes } from "./overviewProgressStore"
import type { OverviewProgress } from "./overviewProgressStore"

let snapshot: OverviewProgress

vi.mock("./overviewProgressStore", () => ({
  subscribeOverviewProgress: () => () => undefined,
  overviewProgress: () => snapshot,
  enableNonRepoFolders: vi.fn(),
  openSteps: vi.fn(),
  shrinkSteps: vi.fn(),
  openFixes: vi.fn(),
  shrinkFixes: vi.fn(),
}))

// `requestFolderAccess` wraps the real implementation (denied without a
// shell) so most tests see today's behavior; a granted-folder test overrides
// it with `mockResolvedValueOnce`.
vi.mock("../../../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof IpcModule>()
  return {
    ...actual,
    noteInteraction: vi.fn(),
    scanNow: vi.fn().mockResolvedValue(undefined),
    requestFolderAccess: vi.fn(actual.requestFolderAccess),
  }
})

afterEach(() => vi.clearAllMocks())

function progress(overrides: Partial<OverviewProgress> = {}): OverviewProgress {
  const merged = {
    mode: "firstRun" as const,
    find: {
      done: true,
      rows: [{ agent: "claude-code", label: "Claude Code", sessions: 49, done: true }],
    },
    read: {
      done: false,
      completed: 0,
      total: 0,
      gate: null,
      includeNonRepoFolders: false,
      deferred: [],
    },
    check: { done: false, windowSessions: 0, pendingEvidence: 0, deferredEvidence: 0 },
    categories: [],
    failingCount: 0,
    history: null,
    stepsDone: false,
    dock: { stepsDocked: 0, stepsOpen: false, fixesDocked: false },
    ...overrides,
  }
  // Mirrors `deriveOverviewProgress`'s own formula, so a test only states the
  // check and dock fields it cares about rather than this derived one too.
  const resultReady =
    merged.check.done && (merged.mode !== "firstRun" || merged.dock.stepsDocked >= 3)
  return { ...merged, resultReady }
}

describe("OverviewFixes's pending steps", () => {
  it("shows Waiting, not 0/0, for a read step discovery has not sized yet", () => {
    snapshot = progress({
      read: {
        done: false,
        completed: 0,
        total: 0,
        gate: null,
        includeNonRepoFolders: false,
        deferred: [],
      },
    })
    render(<OverviewFixes />)
    expect(screen.getByText("Read session data").parentElement).toHaveTextContent("Waiting")
    expect(screen.queryByText("0/0")).not.toBeInTheDocument()
  })

  it("shows real numbers for a read step already under way", () => {
    snapshot = progress({
      read: {
        done: false,
        completed: 12,
        total: 50,
        gate: null,
        includeNonRepoFolders: false,
        deferred: [],
      },
    })
    render(<OverviewFixes />)
    expect(screen.getByText("12/50")).toBeInTheDocument()
  })

  it("keeps the check step at Waiting until the read step finishes, even with a settled report", () => {
    // Mirrors the reported bug: the checks report can settle (142 checked)
    // while discovery is still mid-pass. The check step must not read as
    // done, or even as counting up, before step 2 does.
    snapshot = progress({
      read: {
        done: false,
        completed: 0,
        total: 0,
        gate: null,
        includeNonRepoFolders: false,
        deferred: [],
      },
      check: { done: true, windowSessions: 142, pendingEvidence: 0, deferredEvidence: 0 },
    })
    render(<OverviewFixes />)
    expect(screen.getByText("Run session checks").parentElement).toHaveTextContent("Waiting")
    expect(screen.queryByText("142/142")).not.toBeInTheDocument()
  })

  it("shows the check step done once both it and the read step finish", () => {
    snapshot = progress({
      read: {
        done: true,
        completed: 49,
        total: 49,
        gate: null,
        includeNonRepoFolders: false,
        deferred: [],
      },
      check: { done: true, windowSessions: 142, pendingEvidence: 0, deferredEvidence: 0 },
    })
    render(<OverviewFixes />)
    expect(screen.getByText("142/142")).toBeInTheDocument()
  })
})

describe("OverviewFixes's find step", () => {
  it("shows every searched agent: pulsing while searching, grey when empty, counted when found", () => {
    snapshot = progress({
      find: {
        done: false,
        rows: [
          { agent: "claude-code", label: "Claude Code", sessions: 27, done: true },
          { agent: "codex", label: "Codex", sessions: 0, done: false },
          { agent: "cursor", label: "Cursor", sessions: 0, done: true },
        ],
      },
    })
    render(<OverviewFixes />)
    const found = screen.getByLabelText("Claude Code: 27 sessions")
    expect(found).toHaveTextContent("27")
    expect(found).not.toHaveClass("animate-pulse")
    expect(screen.getByLabelText("Codex: searching")).toHaveClass("animate-pulse")
    const empty = screen.getByLabelText("Cursor: 0 sessions")
    expect(empty).toHaveClass("grayscale")
    expect(empty).not.toHaveTextContent("0")
  })
})

describe("OverviewFixes's dock row", () => {
  const finished = {
    find: {
      done: true,
      rows: [{ agent: "claude-code", label: "Claude Code", sessions: 47, done: true }],
    },
    read: {
      done: true,
      completed: 47,
      total: 47,
      gate: null,
      includeNonRepoFolders: false,
      deferred: [],
    },
    check: { done: true, windowSessions: 43, pendingEvidence: 0, deferredEvidence: 0 },
    categories: [
      {
        id: "modelOverthinking" as const,
        label: "Model overthinking",
        status: "needsFix" as const,
      },
    ],
    failingCount: 1,
    stepsDone: true,
  }

  it("keeps the result hidden until the last step reaches the row", () => {
    snapshot = progress({
      ...finished,
      dock: { stepsDocked: 2, stepsOpen: false, fixesDocked: false },
    })
    render(<OverviewFixes />)
    expect(screen.getByLabelText(/^Find session files: 47/)).toBeInTheDocument()
    expect(screen.getByLabelText(/^Read session data: 47\/47/)).toBeInTheDocument()
    expect(screen.getByText("Run session checks")).toBeInTheDocument()
    expect(screen.queryByText(/fix found in your config/)).not.toBeInTheDocument()
  })

  it("shows the result once every step is docked, and shrinks it into the row", () => {
    snapshot = progress({
      ...finished,
      dock: { stepsDocked: 3, stepsOpen: false, fixesDocked: false },
    })
    render(<OverviewFixes />)
    expect(screen.getByText(/1 fix found in your config/)).toBeInTheDocument()
    // The row's fixes cell shows too, before the reader shrinks the headline,
    // but it opens nothing while the headline is already in the middle.
    expect(screen.getByText("1 fix found")).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /^1 fix found/ })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Shrink" }))
    expect(shrinkFixes).toHaveBeenCalled()
  })

  it("opens the steps as a group from a docked cell", () => {
    snapshot = progress({
      ...finished,
      dock: { stepsDocked: 3, stepsOpen: false, fixesDocked: true },
    })
    render(<OverviewFixes />)
    expect(screen.getByRole("button", { name: /^1 fix found/ })).toBeInTheDocument()
    fireEvent.click(screen.getByLabelText(/^Run session checks: 43\/43/))
    expect(openSteps).toHaveBeenCalled()
  })

  it("opens the result from the whole fixes cell, and keeps Enhance a separate button", () => {
    snapshot = progress({
      ...finished,
      failingCount: 2,
      dock: { stepsDocked: 3, stepsOpen: false, fixesDocked: true },
    })
    render(<OverviewFixes />)
    fireEvent.click(screen.getByRole("button", { name: /^2 fixes found/ }))
    expect(openFixes).toHaveBeenCalled()
    expect(screen.getByRole("button", { name: "Enhance" })).toBeInTheDocument()
  })
})

describe("OverviewFixes's pending mode", () => {
  it("renders no row and no middle overlay before the first-run decision", () => {
    snapshot = progress({
      mode: "pending",
      find: { done: false, rows: [] },
      read: {
        done: false,
        completed: 0,
        total: 0,
        gate: null,
        includeNonRepoFolders: false,
        deferred: [],
      },
      check: { done: false, windowSessions: 0, pendingEvidence: 0, deferredEvidence: 0 },
    })
    render(<OverviewFixes />)
    expect(screen.queryByText("Find session files")).not.toBeInTheDocument()
    expect(screen.queryByText("Run session checks")).not.toBeInTheDocument()
  })
})

describe("OverviewFixes's steady mode", () => {
  const steady = {
    mode: "steady" as const,
    find: {
      done: true,
      rows: [{ agent: "claude-code", label: "Claude Code", sessions: 47, done: true }],
    },
    read: {
      done: true,
      completed: 47,
      total: 47,
      gate: null,
      includeNonRepoFolders: false,
      deferred: [],
    },
    check: { done: true, windowSessions: 43, pendingEvidence: 1, deferredEvidence: 0 },
    categories: [],
    failingCount: 0,
    stepsDone: true,
  }

  it("renders the row with Find, Read and Run checks cells", () => {
    snapshot = progress({
      ...steady,
      dock: { stepsDocked: 0, stepsOpen: false, fixesDocked: false },
    })
    render(<OverviewFixes />)
    expect(screen.getByLabelText(/^Find session files: 47/)).toBeInTheDocument()
    expect(screen.getByLabelText(/^Read session data: 47\/47/)).toBeInTheDocument()
    expect(screen.getByLabelText(/^Run session checks: 42\/43/)).toBeInTheDocument()
  })

  it("counts the Run checks cell as started even though read.done is false", () => {
    snapshot = progress({
      ...steady,
      read: {
        done: false,
        completed: 0,
        total: 0,
        gate: null,
        includeNonRepoFolders: false,
        deferred: [],
      },
      dock: { stepsDocked: 0, stepsOpen: false, fixesDocked: false },
    })
    render(<OverviewFixes />)
    expect(screen.getByLabelText(/^Run session checks: 42\/43/)).toBeInTheDocument()
  })

  it("clicking a docked cell opens the steps in the middle", () => {
    snapshot = progress({
      ...steady,
      dock: { stepsDocked: 0, stepsOpen: false, fixesDocked: false },
    })
    render(<OverviewFixes />)
    fireEvent.click(screen.getByLabelText(/^Find session files: 47/))
    expect(openSteps).toHaveBeenCalled()
  })

  it("the open steps in steady mode show all three steps and a Shrink button", () => {
    // The fixes result stays undecided here (`check.done` false), so only
    // the opened steps' own Shrink button renders.
    snapshot = progress({
      ...steady,
      check: { done: false, windowSessions: 43, pendingEvidence: 1, deferredEvidence: 0 },
      dock: { stepsDocked: 0, stepsOpen: true, fixesDocked: false },
    })
    render(<OverviewFixes />)
    expect(screen.getByText("Find session files")).toBeInTheDocument()
    expect(screen.getByText("Read session data")).toBeInTheDocument()
    expect(screen.getByText("Run session checks")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Shrink" })).toBeInTheDocument()
  })
})

describe("OverviewFixes's first-run welcome line", () => {
  it("shows the pitch before any step has docked", () => {
    snapshot = progress({ dock: { stepsDocked: 0, stepsOpen: false, fixesDocked: false } })
    render(<OverviewFixes />)
    expect(screen.getByText("Stop hitting your token limits.")).toBeInTheDocument()
  })

  it("hides the pitch once a step has docked", () => {
    snapshot = progress({ dock: { stepsDocked: 1, stepsOpen: false, fixesDocked: false } })
    render(<OverviewFixes />)
    expect(screen.queryByText("Stop hitting your token limits.")).not.toBeInTheDocument()
  })
})

describe("OverviewFixes's read step folder permission notice", () => {
  it("stays hidden with nothing deferred", () => {
    snapshot = progress({
      read: {
        done: false,
        completed: 0,
        total: 0,
        gate: null,
        includeNonRepoFolders: false,
        deferred: [],
      },
    })
    render(<OverviewFixes />)
    expect(screen.queryByText(/needs? your permission/)).not.toBeInTheDocument()
  })

  it("asks for the deferred folders and starts the flow on click", () => {
    snapshot = progress({
      read: {
        done: false,
        completed: 0,
        total: 0,
        gate: null,
        includeNonRepoFolders: false,
        deferred: [{ dir: "/Users/dave/work", pathCount: 3 }],
      },
    })
    render(<OverviewFixes />)
    expect(
      screen.getByText(/1 folder needs your permission before antiburn can read it\./),
    ).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Allow access" }))
    expect(screen.getByRole("button", { name: "Asking…" })).toBeInTheDocument()
  })

  it("fires folder_access_requested the moment Allow access is clicked", () => {
    snapshot = progress({
      read: {
        done: false,
        completed: 0,
        total: 0,
        gate: null,
        includeNonRepoFolders: false,
        deferred: [{ dir: "/Users/dave/work", pathCount: 3 }],
      },
    })
    render(<OverviewFixes />)
    fireEvent.click(screen.getByRole("button", { name: "Allow access" }))
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "firstRunAction",
      action: "folder_access_requested",
    })
  })

  it("fires folder_access_granted and rescans once the flow grants a folder", async () => {
    vi.mocked(requestFolderAccess).mockResolvedValueOnce({ outcome: "granted", elapsedMs: 5 })
    snapshot = progress({
      read: {
        done: false,
        completed: 0,
        total: 0,
        gate: null,
        includeNonRepoFolders: false,
        deferred: [{ dir: "/Users/dave/work", pathCount: 3 }],
      },
    })
    render(<OverviewFixes />)
    fireEvent.click(screen.getByRole("button", { name: "Allow access" }))
    await vi.waitFor(() =>
      expect(noteInteraction).toHaveBeenCalledWith({
        kind: "firstRunAction",
        action: "folder_access_granted",
      }),
    )
    expect(scanNow).toHaveBeenCalled()
  })
})
