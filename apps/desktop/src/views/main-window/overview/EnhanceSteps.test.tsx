import { render, screen, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { ChecksCategoryLifecycle, ChecksCategoryPayload } from "../../../lib/insightsIpc"
import * as SnoozedBurnChecks from "../../../lib/snoozedBurnChecks"
import type { BurnChecksSession, BurnChecksSnapshot } from "../BurnChecksSession"
import * as Ipc from "../../../lib/ipc"
import type { ScanStatus } from "../../../lib/ipc"
import { scanStatusStore } from "../../../lib/scanStatusStore"
import { FixStep, ScanStep, SourcesStep } from "./EnhanceSteps"

function category(
  id: ChecksCategoryPayload["id"],
  lifecycle: ChecksCategoryLifecycle | null,
  burn: number | null = null,
): ChecksCategoryPayload {
  return {
    id,
    lifecycle,
    finding: lifecycle === "failing" ? 3 : 0,
    clean: lifecycle == null ? 0 : 5,
    unavailable: 0,
    estimatedTokenBurnBasisPoints: burn,
  }
}

const state: BurnChecksSnapshot = {
  active: true,
  report: {
    evidenceSettled: true,
    pendingEvidence: 0,
    estimatedTokenBurnBasisPoints: 900,
    categories: [
      category("unusedMcpServers", "failing", 200),
      category("sessionsOverDepth", "failing", 600),
      category("modelOverthinking", "failing", 100),
      category("oldModelUsage", "passing", 0),
      category("cacheChurn", null),
    ],
  },
  aggregate: null,
  loading: false,
  refreshing: false,
  error: false,
  targets: {},
}

beforeEach(() => {
  vi.spyOn(SnoozedBurnChecks, "useSnoozedBurnChecks").mockReturnValue({
    status: "ready",
    records: [{ detector: "modelOverthinking", scope: "check", until: null }],
  })
})
afterEach(() => vi.restoreAllMocks())

describe("SourcesStep", () => {
  it("fetches the agent list when scan events arrived without it", async () => {
    scanStatusStore.set({ running: true, agents: [] } as unknown as ScanStatus)
    vi.spyOn(Ipc, "getScanStatus").mockResolvedValue({
      running: true,
      agents: [{ agent: "claude-code", lastCompletedAt: null, sessionsSeen: 42 }],
    } as unknown as ScanStatus)
    render(<SourcesStep />)
    expect(screen.getByText("Looking for your agents…")).toBeInTheDocument()
    expect(await screen.findByText("42 sessions")).toBeInTheDocument()
  })
})

describe("ScanStep", () => {
  it("counts only failing checks that are not snoozed, and marks each state", () => {
    render(<ScanStep state={state} />)
    expect(screen.getByText("2 fixes found")).toBeInTheDocument()
    const tiles = within(screen.getByRole("list", { name: "Burn checks" })).getAllByRole(
      "listitem",
    )
    expect(tiles).toHaveLength(5)
    const text = (label: string) => tiles.find((tile) => tile.textContent?.includes(label))
    expect(text("Model overthinking")).toHaveTextContent("Snoozed")
    expect(text("Excess cache rehydration")).toHaveTextContent("Not checked yet")
    expect(text("Old model usage")).toHaveTextContent("Passed")
  })
})

describe("FixStep", () => {
  it("shows one card per failing check, biggest first, and lists snoozed checks", () => {
    const session = {
      setTargetsVisible: vi.fn(),
      refresh: vi.fn(),
    } as unknown as BurnChecksSession
    render(<FixStep session={session} state={state} />)
    const cards = screen.getAllByRole("article")
    expect(cards.map((card) => card.getAttribute("aria-label"))).toEqual([
      "Session overdepth",
      "Unused MCP servers",
    ])
    expect(session.setTargetsVisible).toHaveBeenCalledWith("sessionsOverDepth", true)
    expect(
      within(screen.getByRole("region", { name: "Snoozed checks" })).getByText(
        "Model overthinking",
      ),
    ).toBeInTheDocument()
  })
})
