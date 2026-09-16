import { act, fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { BurnCheckTargetPayload } from "../../../lib/insightsIpc"
import { BurnCheckTargetDetail, targetCostLine } from "./BurnCheckTargetDetail"
import { scopeLabel } from "./BurnCheckTargetPresentation"
import { performProjectFolderAction } from "../../../lib/projectFolder"

vi.mock("../../../lib/projectFolder", () => ({
  performProjectFolderAction: vi.fn().mockResolvedValue(undefined),
}))
beforeEach(() => vi.mocked(performProjectFolderAction).mockClear())

function target(overrides: Partial<BurnCheckTargetPayload> = {}): BurnCheckTargetPayload {
  return {
    findingId: "finding-stable",
    actionId: "action-fresh",
    finding: {
      detector: "unusedBuiltInTools",
      agent: "claude-code",
      sourceFormat: "claudeJsonl",
      observation: "The Workflow tool was never invoked.",
      labels: [],
      omitted: 0,
    },
    display: {
      resourceKind: "builtInTool",
      resourceIdentity: "Workflow",
      currentValue: null,
      replacementValue: null,
      scopeKind: "session",
      quantity: 400,
      quantityUnit: "tokens",
      observationCount: 2,
      firstObservedAtMs: 1,
      lastObservedAtMs: 2,
      estimateMethod: "builtInDefinitionReplication",
      estimatedOpportunity: null,
      estimatedTokenBurnBasisPoints: null,
      verificationLimit: "freshEvidenceFromSameSourceAndTarget",
    },
    occurrenceCount: 2,
    autoFix: { status: "unavailable", reason: "unsupportedOrUnprovenTarget" },
    promptFix: { status: "unavailable", reason: "checkUnsupportedForAgent" },
    watch: null,
    coverageLimits: ["currentPublishedEvidenceOnly"],
    samples: [],
    expiresAtEpoch: 100,
    ...overrides,
  }
}

describe("target cost line", () => {
  it("uses the distinct affected-session count instead of the occurrence count", () => {
    expect(
      targetCostLine(
        target({
          affectedSessionCount: 1,
          occurrenceCount: 4,
          display: {
            ...target().display,
            estimatedOpportunity: { value: 8.2, unit: "apiEquivalentUsd" },
          },
        }),
      ),
    ).toBe(
      "~$8.20 in cache reads of this unused definition across 1 session, sub-agent requests included.",
    )
  })

  it("falls back to occurrence wording when the affected-session count is unavailable", () => {
    expect(
      targetCostLine(
        target({
          display: {
            ...target().display,
            estimatedOpportunity: { value: 1, unit: "apiEquivalentUsd" },
          },
        }),
      ),
    ).toBe(
      "~$1.00 in cache reads of this unused definition across 2 occurrences, sub-agent requests included.",
    )
  })

  it("omits the line when there is no estimate", () => {
    expect(targetCostLine(target())).toBeNull()
  })

  it("omits the line when the estimate did not price to dollars", () => {
    expect(
      targetCostLine(
        target({
          display: {
            ...target().display,
            estimatedOpportunity: { value: 400, unit: "literalInputTokens" },
          },
        }),
      ),
    ).toBeNull()
  })
})

describe("BurnCheckTargetDetail", () => {
  it("shows the cost line under the recommendation when priced", () => {
    render(
      <BurnCheckTargetDetail
        target={target({
          display: {
            ...target().display,
            estimatedOpportunity: { value: 8.2, unit: "apiEquivalentUsd" },
          },
        })}
        refresh={() => undefined}
      />,
    )
    expect(screen.getByText(/~\$8\.20 in cache reads/)).toBeInTheDocument()
  })

  it("offers full-path folder actions and distinguishes samples from affected sessions", async () => {
    render(
      <BurnCheckTargetDetail
        reportRow
        refresh={() => undefined}
        target={target({
          projectName: "example-project",
          projectLocation: "…/worktrees/example-project",
          projectPath: "/tmp/worktrees/example-project",
          affectedSessionCount: 7,
          display: { ...target().display, scopeKind: "project" },
          samples: [
            {
              navigationHandle: "sample-one",
              title: "Example session",
              agent: "claude-code",
              surface: "cli",
              observedAtMs: 1,
              repo: "demo",
              timestamp: "2026-09-14T12:00:00Z",
              isActive: false,
              hasForkParent: false,
              forkChildCount: 0,
              cost: null,
              models: [],
              modelRuns: [],
              hygiene: { evidenceState: "pending", badges: [] },
            },
          ],
        })}
      />,
    )
    expect(screen.getByText("· example-project")).toBeInTheDocument()
    expect(screen.queryByText("…/worktrees/example-project")).not.toBeInTheDocument()
    act(() => screen.getByRole("button", { name: "Project folder" }).focus())
    expect(document.querySelector(".project-folder-path")).toHaveTextContent(
      "/tmp/worktrees/example-project",
    )
    expect(screen.getByRole("dialog").parentElement).toBe(document.body)
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Copy path" })))
    expect(performProjectFolderAction).toHaveBeenCalledWith(
      "/tmp/worktrees/example-project",
      "copy",
    )
    await act(async () => fireEvent.click(screen.getByRole("button", { name: /^Open in/ })))
    expect(performProjectFolderAction).toHaveBeenCalledWith(
      "/tmp/worktrees/example-project",
      "open",
    )
    fireEvent.keyDown(document, { key: "Escape" })
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /Failed sessions/ })).toBeNull()
    expect(screen.getByRole("button", { name: /Example session/ })).toBeVisible()
  })

  it("does not use a shortened display location as an actionable path", () => {
    render(
      <BurnCheckTargetDetail
        reportRow
        refresh={() => undefined}
        target={target({ projectLocation: "…/worktrees/example-project" })}
      />,
    )
    expect(screen.queryByRole("button", { name: "Project folder" })).toBeNull()
  })
})

describe("scopeLabel", () => {
  it.each([
    ["global", "Global configuration"],
    ["project", "Project configuration"],
    ["session", "Session scope"],
    ["worker", "Worker scope"],
  ] as const)("formats %s scope", (scope, label) => {
    expect(scopeLabel(scope)).toBe(label)
  })
})
