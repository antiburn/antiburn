import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import type { BurnCheckTargetPayload } from "../../../lib/insightsIpc"
import { BurnCheckTargetDetail, targetCostLine } from "./BurnCheckTargetDetail"
import { scopeLabel } from "./BurnCheckTargetPresentation"

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

  it("reveals the folder on demand and distinguishes samples from affected sessions", () => {
    render(
      <BurnCheckTargetDetail
        reportRow
        refresh={() => undefined}
        target={target({
          projectName: "example-project",
          projectLocation: "…/worktrees/example-project",
          affectedSessionCount: 7,
          display: { ...target().display, scopeKind: "project" },
          samples: [
            {
              navigationHandle: "sample-one",
              title: "Example session",
              agent: "claude-code",
              surface: "cli",
              observedAtMs: 1,
            },
          ],
        })}
      />,
    )
    expect(screen.getByText("· example-project")).toBeInTheDocument()
    expect(screen.queryByText("…/worktrees/example-project")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Folder location" }))
    expect(screen.getByText("…/worktrees/example-project")).toBeInTheDocument()
    fireEvent.keyDown(document, { key: "Escape" })
    expect(screen.queryByText("…/worktrees/example-project")).not.toBeInTheDocument()
    const disclosure = screen.getByRole("button", { name: "Sample session 1" })
    expect(disclosure).toHaveAccessibleDescription("1 sample session out of 7 affected.")
    expect(disclosure).toHaveAttribute("aria-expanded", "true")
    expect(
      screen.getByRole("button", { name: "Open sample session Example session" }),
    ).toBeVisible()
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
