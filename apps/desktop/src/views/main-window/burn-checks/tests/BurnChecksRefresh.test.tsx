import { act, fireEvent, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import type {
  AggregateWinsPayload,
  BurnCheckTargetListPayload,
  ChecksReportPayload,
} from "../../../../lib/insightsIpc"
import * as SnoozedBurnChecks from "../../../../lib/snoozedBurnChecks"
import { aggregate, deferred, report, setup, target } from "./burnChecksTestSupport"

beforeEach(() => {
  vi.spyOn(SnoozedBurnChecks, "useSnoozedBurnChecks").mockReturnValue({
    status: "ready",
    records: [],
  })
})

afterEach(() => vi.restoreAllMocks())

it("keeps the selected detail and expanded groups mounted during background reads", async () => {
  const initial: ChecksReportPayload = {
    ...report,
    categories: [
      report.categories[0]!,
      { ...report.categories[0]!, id: "unusedMcpServers" },
      report.categories[1]!,
    ],
  }
  const { adapter, session, view } = setup(target, false, aggregate, initial)
  const row = await screen.findByRole("button", { name: /Unused MCP servers/ })
  fireEvent.click(row)
  const passed = screen.getByRole("button", { name: "Passed checks 1" })
  expect(passed).toHaveAttribute("aria-expanded", "true")
  await waitFor(() =>
    expect(session.getSnapshot().targets.unusedMcpServers?.loading).toBe(false),
  )
  const detail = document.getElementById("burn-check-unusedMcpServers-detail")!
  const card = detail.querySelector("article")!
  expect(card).toBeVisible()

  const pendingReport = deferred<ChecksReportPayload | null>()
  const pendingTargets = deferred<BurnCheckTargetListPayload | null>()
  const pendingAggregate = deferred<AggregateWinsPayload | null>()
  vi.mocked(adapter.getReport).mockReturnValueOnce(pendingReport.promise)
  vi.mocked(adapter.getTargets).mockReturnValueOnce(pendingTargets.promise)
  vi.mocked(adapter.getAggregateWins).mockReturnValueOnce(pendingAggregate.promise)
  const previousAggregate = session.getSnapshot().aggregate
  act(() => session.refresh())

  const expectStableDetail = () => {
    expect(row).toHaveAttribute("aria-pressed", "true")
    expect(passed).toHaveAttribute("aria-expanded", "true")
    expect(document.getElementById("burn-check-unusedMcpServers-detail")).toBe(detail)
    expect(detail.querySelector("[data-finding-id]")).toBe(card)
    expect(card).toBeVisible()
    expect(screen.queryByRole("region", { name: "Loading finding details" })).toBeNull()
    expect(session.getSnapshot().aggregate).toBe(previousAggregate)
  }
  expectStableDetail()
  await act(async () => pendingReport.resolve({ ...initial, pendingEvidence: 1 }))
  await waitFor(() =>
    expect(session.getSnapshot().targets.unusedMcpServers?.loading).toBe(true),
  )
  expectStableDetail()
  await act(async () => {
    pendingTargets.reject(new Error("Unavailable"))
    pendingAggregate.reject(new Error("Unavailable"))
  })
  expectStableDetail()
  expect(session.getSnapshot().targets.unusedMcpServers?.error).toBe(true)
  expect(screen.getByRole("alert")).toHaveTextContent("The check details did not load.")
  expect(screen.getByRole("button", { name: "Retry" })).toBeVisible()
  view.unmount()
})
