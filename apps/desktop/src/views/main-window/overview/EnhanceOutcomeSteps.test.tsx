import { render, screen, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type {
  BurnCheckTargetPayload,
  ChecksCategoryLifecycle,
  ChecksCategoryPayload,
} from "../../../lib/insightsIpc"
import * as SnoozedBurnChecks from "../../../lib/snoozedBurnChecks"
import type { BurnChecksSession, BurnChecksSnapshot } from "../BurnChecksSession"
import { DoneStep, WatchStep } from "./EnhanceOutcomeSteps"

function category(
  id: ChecksCategoryPayload["id"],
  lifecycle: ChecksCategoryLifecycle,
): ChecksCategoryPayload {
  return {
    id,
    lifecycle,
    finding: lifecycle === "passing" ? 0 : 2,
    clean: 5,
    unavailable: 0,
    estimatedTokenBurnBasisPoints: 100,
  }
}

function target(unit: string, value: number, watch: object | null): BurnCheckTargetPayload {
  return {
    display: { estimatedOpportunity: { value, unit } },
    watch: watch && {
      origin: "action",
      lifecycle: "watching",
      verification: { status: "watching" },
      ...watch,
    },
  } as unknown as BurnCheckTargetPayload
}

function loaded(targets: BurnCheckTargetPayload[]) {
  return { data: { targets, samples: [], truncated: false }, loading: false, error: false }
}

function snapshot(targets: Record<string, ReturnType<typeof loaded>>): BurnChecksSnapshot {
  return {
    active: true,
    report: {
      evidenceSettled: true,
      pendingEvidence: 0,
      estimatedTokenBurnBasisPoints: 200,
      categories: [
        category("unusedMcpServers", "awaitingVerification"),
        category("sessionsOverDepth", "failing"),
        category("modelOverthinking", "failing"),
      ],
    },
    aggregate: null,
    loading: false,
    refreshing: false,
    error: false,
    targets: targets as unknown as BurnChecksSnapshot["targets"],
  }
}

const session = { setTargetsVisible: vi.fn(), refresh: vi.fn() } as unknown as BurnChecksSession

beforeEach(() => {
  vi.spyOn(SnoozedBurnChecks, "useSnoozedBurnChecks").mockReturnValue({
    status: "ready",
    records: [{ detector: "modelOverthinking", scope: "check", until: null }],
  })
})
afterEach(() => vi.restoreAllMocks())

describe("WatchStep", () => {
  it("lists the fixes that have a watch", () => {
    const state = snapshot({
      unusedMcpServers: loaded([
        target("literalInputTokens", 1000, { verification: { status: "fixed" } }),
      ]),
      sessionsOverDepth: loaded([]),
    })
    render(<WatchStep session={session} state={state} />)
    const rows = within(screen.getByRole("list", { name: "Fixes being watched" })).getAllByRole(
      "listitem",
    )
    expect(rows).toHaveLength(1)
    expect(rows[0]).toHaveTextContent("Confirmed. This fix works.")
    expect(session.setTargetsVisible).toHaveBeenCalledWith("sessionsOverDepth", true)
  })
})

describe("DoneStep", () => {
  it("projects applied fixes in tokens and dollars, and leaves snoozed checks out", () => {
    const state = snapshot({
      unusedMcpServers: loaded([target("literalInputTokens", 10_000, {})]),
      sessionsOverDepth: loaded([
        target("apiEquivalentUsd", 5, {}),
        target("apiEquivalentUsd", 100, null),
      ]),
    })
    render(<DoneStep session={session} state={state} />)
    const figures = screen.getByRole("region", { name: "Projected savings" })
    expect(figures).toHaveTextContent("Your fixes save about")
    expect(figures).toHaveTextContent("30k")
    expect(figures).toHaveTextContent("~$15.00")
    expect(screen.getByText("Snoozed, not counted").previousSibling).toHaveTextContent("1")
    expect(screen.getByText("Fixes applied").previousSibling).toHaveTextContent("2")
  })

  it("shows what the open fixes save when none is applied", () => {
    const state = snapshot({
      unusedMcpServers: loaded([]),
      sessionsOverDepth: loaded([target("apiEquivalentUsd", 2, null)]),
    })
    render(<DoneStep session={session} state={state} />)
    expect(screen.getByText("Apply the open fixes to save about")).toBeInTheDocument()
  })
})
