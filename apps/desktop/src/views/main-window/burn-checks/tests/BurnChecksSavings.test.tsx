import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { AggregateWinsPayload } from "../../../../lib/insightsIpc"
import type * as ClipboardModule from "../../../../lib/clipboard"
import type * as InsightsIpcModule from "../../../../lib/insightsIpc"
import type * as IpcModule from "../../../../lib/ipc"
import { BurnCheckDetail } from "../BurnCheckDetail"

import {
  report,
  passedTargetReport,
  target,
  aggregate,
  deferred,
  setup,
  renderSavings,
  installBurnChecksCommandMocks,
  restoreBurnChecksTestWindow,
} from "./burnChecksTestSupport"

const commands = vi.hoisted(() => ({
  prepare: vi.fn(),
  apply: vi.fn(),
  copy: vi.fn(),
  copyFallback: vi.fn(),
  copyBatch: vi.fn(),
  writeClipboardText: vi.fn(),
  openSample: vi.fn(),
  noteInteraction: vi.fn(),
}))

vi.mock("../../../../lib/insightsIpc", async (importOriginal) => ({
  ...(await importOriginal<typeof InsightsIpcModule>()),
  prepareAutoFixBurnCheckTarget: commands.prepare,
  applyPreparedBurnCheckOperation: commands.apply,
  copyPromptFixBurnCheckTarget: commands.copy,
  copyPromptFixBurnCheck: commands.copyFallback,
  copyPromptFixBurnCheckTargets: commands.copyBatch,
  openBurnCheckSample: commands.openSample,
}))

vi.mock("../../../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  noteInteraction: commands.noteInteraction,
}))

vi.mock("../../../../lib/clipboard", async (importOriginal) => ({
  ...(await importOriginal<typeof ClipboardModule>()),
  writeClipboardText: commands.writeClipboardText,
}))

beforeEach(() => {
  installBurnChecksCommandMocks(commands)
})

afterEach(() => {
  restoreBurnChecksTestWindow()
})

// This file holds the savings tests for the Burn Checks view.
// The suite is split across files in this folder by theme. Each test renders
// the full view. CI runs this file next to the other heavy view suites, so
// one test can take five times its local run time. 15 s is the bound, not a
// target.
describe("BurnChecksView savings", { timeout: 15_000 }, () => {
  it("does not invent an estimated opportunity when it is unavailable", () => {
    render(
      <BurnCheckDetail
        detector="oldModelUsage"
        targets={[target]}
        samples={target.samples}
        failedSessionCount={1}
        refresh={vi.fn()}
      />,
    )

    expect(
      screen.getByText("Some sessions used an older model when a newer one was available."),
    ).toBeVisible()
    expect(screen.queryByText("Estimated opportunity:")).not.toBeInTheDocument()
    expect(screen.queryByText(/ opportunity$/)).not.toBeInTheDocument()
  })

  it("shows aggregate savings for a passed check", () => {
    renderSavings([
      aggregate.wins[0]!,
      { ...aggregate.wins[0]!, verifiedBoundaryMs: 3 },
      {
        ...aggregate.wins[0]!,
        findingId: "win-2",
        display: {
          ...aggregate.wins[0]!.display,
          estimatedOpportunity: { value: 400, unit: "literalInputTokens" },
        },
        savings: {
          status: { status: "unavailable" },
          tokenSavings: null,
          apiEquivalentCostAvoidedUsd: null,
          improvementCount: null,
          method: null,
        },
      },
    ])

    const savings = screen.getByRole("region", { name: "Savings" })
    expect(within(savings).getByText("2 verified remediation cycles")).toBeVisible()
    expect(within(savings).getAllByText("~400 input tokens projected")).toHaveLength(2)
    expect(within(savings).getAllByText("Unavailable for this check.")).toHaveLength(2)
    expect(
      within(savings).getAllByText("~$1.25 confirmed from 1 of 2 verified cycles"),
    ).toHaveLength(2)
    expect(within(savings).getByRole("button", { name: "Details" })).toHaveAttribute(
      "aria-expanded",
      "false",
    )
  })

  it("uses backend-trusted active wins without remediation progress", () => {
    const unrelated = {
      ...aggregate.wins[0]!,
      findingId: "unrelated-finding",
      remediationCycleId: "unrelated-cycle",
      display: {
        ...aggregate.wins[0]!.display,
        estimatedOpportunity: { value: 900, unit: "literalInputTokens" as const },
      },
    }
    renderSavings([aggregate.wins[0]!, unrelated])

    const savings = screen.getByRole("region", { name: "Savings" })
    expect(within(savings).getByText("2 verified remediation cycles")).toBeVisible()
    expect(within(savings).getAllByText("~900 input tokens projected")).toHaveLength(2)
  })

  it("hides savings while aggregate wins load and shows them when ready", async () => {
    const pending = deferred<AggregateWinsPayload>()
    setup(target, false, pending.promise, passedTargetReport)

    await screen.findByRole("button", { name: /Passed checks/ })
    expect(screen.queryByRole("region", { name: "Savings" })).not.toBeInTheDocument()

    pending.resolve(aggregate)
    expect(await screen.findByRole("region", { name: "Savings" })).toBeVisible()
  })

  it("counts one estimated opportunity per exact target", () => {
    const opportunity = {
      ...aggregate.wins[0]!,
      display: {
        ...aggregate.wins[0]!.display,
        estimatedOpportunity: { value: 400, unit: "literalInputTokens" as const },
      },
    }
    renderSavings([opportunity, { ...opportunity, remediationCycleId: "cycle-2" }])

    const savings = screen.getByRole("region", { name: "Savings" })
    expect(within(savings).getAllByText("~400 input tokens projected")).toHaveLength(2)
    expect(within(savings).queryByText("~800 input tokens projected")).not.toBeInTheDocument()
  })

  it("uses generic clean copy without unrelated action attribution", async () => {
    setup(target, false, aggregate, passedTargetReport)

    expect(await screen.findByText("No finding in 2 complete sessions.")).toBeVisible()
    expect(screen.queryByText("Current verification passed.")).not.toBeInTheDocument()
    expect(screen.queryByText("Verified after your fix.")).not.toBeInTheDocument()
  })

  it("pairs authoritative token and cost totals that cover the same wins", () => {
    renderSavings([
      {
        ...aggregate.wins[0]!,
        savings: {
          status: {
            status: "known",
            method: "oldModelPriceDifference",
            methodRevision: 1,
            pricingRevision: "pricing-1",
            apiEquivalentCostAvoidedUsd: 1.25,
            measuredThroughMs: 2,
            recurrenceMs: null,
          },
          tokenSavings: null,
          apiEquivalentCostAvoidedUsd: 1.25,
          improvementCount: 2,
          method: "oldModelPriceDifference",
        },
      },
    ])

    const savings = screen.getByRole("region", { name: "Savings" })
    expect(within(savings).getAllByText("~$1.25 confirmed")).toHaveLength(2)
    fireEvent.click(within(savings).getByRole("button", { name: "Details" }))
    expect(within(savings).getByText("Old model usage")).toHaveClass("text-label")
  })

  it("removes savings when a passed check regresses", async () => {
    const { adapter, session } = setup(target, false, aggregate, passedTargetReport)

    expect(await screen.findByRole("region", { name: "Savings" })).toBeVisible()

    vi.mocked(adapter.getReport).mockResolvedValue(report)
    act(() => session.refresh())

    await screen.findByRole("heading", { name: "Failed checks 1" })
    expect(screen.queryByRole("region", { name: "Savings" })).not.toBeInTheDocument()
  })

  it("shows pending confirmed savings and the approved tooltip text", async () => {
    renderSavings([
      {
        ...aggregate.wins[0]!,
        savings: {
          ...aggregate.wins[0]!.savings,
          status: { status: "pending", methodRevision: 1 },
          apiEquivalentCostAvoidedUsd: null,
        },
      },
    ])

    const savings = screen.getByRole("region", { name: "Savings" })
    expect(within(savings).getAllByText("Pending recent usage.")).toHaveLength(2)
    fireEvent.focus(within(savings).getByRole("button", { name: "About confirmed savings" }))
    expect(
      await screen.findByText(
        "Savings observed across sessions that passed after remediation. This is still an estimate and may not match provider billing exactly.",
      ),
    ).toBeVisible()
  })

  it("describes estimated savings as a pre-remediation opportunity", async () => {
    renderSavings(aggregate.wins)

    const savings = screen.getByRole("region", { name: "Savings" })
    fireEvent.focus(within(savings).getByRole("button", { name: "About estimated savings" }))
    expect(
      await screen.findByText(
        "Pre-remediation opportunity estimated from evidence observed before the fix. Actual results can vary.",
      ),
    ).toBeVisible()
    expect(screen.queryByText(/your recent usage/i)).not.toBeInTheDocument()
  })
})
