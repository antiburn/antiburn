import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import type { BurnCheckTargetPayload } from "../../../lib/insightsIpc"
import { BurnCheckTargetDetail, targetCostLine } from "./BurnCheckTargetDetail"

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
  it("renders the dollar line only when the estimate priced to a dollar unit", () => {
    expect(
      targetCostLine(
        target({
          display: {
            ...target().display,
            estimatedOpportunity: { value: 8.2, unit: "apiEquivalentUsd" },
          },
        }),
      ),
    ).toBe(
      "~$8.20 in cache reads of this unused definition across 2 sessions, sub-agent requests included.",
    )
  })

  it("uses the singular session word for one occurrence", () => {
    expect(
      targetCostLine(
        target({
          occurrenceCount: 1,
          display: {
            ...target().display,
            estimatedOpportunity: { value: 1, unit: "apiEquivalentUsd" },
          },
        }),
      ),
    ).toBe(
      "~$1.00 in cache reads of this unused definition across 1 session, sub-agent requests included.",
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

  it("hides the cost line when the estimate is unavailable", () => {
    render(<BurnCheckTargetDetail target={target()} refresh={() => undefined} />)
    expect(screen.queryByText(/in cache reads/)).not.toBeInTheDocument()
  })
})
