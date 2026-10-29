import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  applyPreparedBurnCheckOperation,
  copyPromptFixBurnCheckTarget,
  copyPromptFixBurnCheck,
  copyPromptFixBurnCheckTargets,
  getBurnCheckAggregateWins,
  getChecksReport,
  listBurnCheckTargets,
  openBurnCheckSample,
  prepareAutoFixBurnCheckTarget,
  getInstalledCheckEvidence,
  openInstalledCheckDefinition,
} from "./insightsIpc"

const invoke = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))

describe("burn check IPC", () => {
  beforeEach(() => {
    invoke.mockReset()
  })

  it("sends only the detector when listing", async () => {
    invoke.mockResolvedValue({ targets: [], truncated: false })

    await listBurnCheckTargets("oldModelUsage")

    expect(invoke).toHaveBeenCalledWith("list_burn_check_targets", {
      detector: "oldModelUsage",
    })
  })

  it("uses only the exact read-only installed evidence target", async () => {
    invoke.mockResolvedValue({ status: "unavailable", items: [] })
    await getInstalledCheckEvidence("installed-check-target")
    expect(invoke).toHaveBeenCalledWith("get_installed_check_evidence", {
      targetId: "installed-check-target",
    })
  })

  it("opens the definition with the backend source reference", async () => {
    invoke.mockResolvedValue(undefined)
    await openInstalledCheckDefinition("definition-reference")
    expect(invoke).toHaveBeenCalledWith("open_installed_check_definition", {
      sourceReference: "definition-reference",
    })
  })

  it("returns a typed estimated opportunity without changing its value or unit", async () => {
    invoke.mockResolvedValue({
      targets: [
        {
          display: {
            estimateMethod: "repeatedContextAboveDepthCap",
            estimatedOpportunity: { value: 2048, unit: "literalInputTokens" },
          },
        },
      ],
      truncated: false,
    })

    const result = await listBurnCheckTargets("sessionsOverDepth")

    expect(result?.targets[0]?.display.estimatedOpportunity).toEqual({
      value: 2048,
      unit: "literalInputTokens",
    })
  })

  it("keeps an unavailable estimated opportunity unavailable", async () => {
    invoke.mockResolvedValue({
      targets: [{ display: { estimatedOpportunity: null } }],
      truncated: false,
    })

    const result = await listBurnCheckTargets("modelOverthinking")

    expect(result?.targets[0]?.display.estimatedOpportunity).toBeNull()
  })

  it("uses stable identity for Copy and expiring handles for Auto Fix", async () => {
    invoke
      .mockResolvedValueOnce({ outcome: "reviewReady", review: {} })
      .mockResolvedValueOnce({ outcome: "appliedAwaitingVerification", watchId: "watch-1" })
      .mockResolvedValueOnce({
        outcome: "promptReady",
        prompt: "Fix the target.",
        watch: {
          watchId: "watch-1",
          origin: "action",
          lifecycle: "watching",
          verification: { status: "watching" },
          savings: { status: "pending" },
        },
      })

    await prepareAutoFixBurnCheckTarget("action-1")
    await applyPreparedBurnCheckOperation("prepared-1")
    await copyPromptFixBurnCheckTarget("oldModelUsage", "finding-1")

    expect(invoke).toHaveBeenNthCalledWith(1, "prepare_auto_fix_burn_check_target", {
      actionId: "action-1",
    })
    expect(invoke).toHaveBeenNthCalledWith(2, "apply_prepared_burn_check_operation", {
      preparedOperationId: "prepared-1",
    })
    expect(invoke).toHaveBeenNthCalledWith(3, "copy_prompt_fix_burn_check_target", {
      detector: "oldModelUsage",
      findingId: "finding-1",
    })
  })

  it("sends only the detector for a current check-level prompt", async () => {
    invoke.mockResolvedValue({ outcome: "promptReady", prompt: "Inspect the evidence." })

    await copyPromptFixBurnCheck("oldModelUsage")

    expect(invoke).toHaveBeenCalledWith("copy_prompt_fix_burn_check", {
      detector: "oldModelUsage",
    })
  })

  it("uses stable target identities for a selected prompt batch", async () => {
    invoke.mockResolvedValue({ outcome: "promptReady", prompt: "Inspect both targets." })

    await copyPromptFixBurnCheckTargets("unusedSkills", ["finding-1", "finding-2"])

    expect(invoke).toHaveBeenCalledWith("copy_prompt_fix_burn_check_targets", {
      detector: "unusedSkills",
      findingIds: ["finding-1", "finding-2"],
    })
  })

  it("reads aggregate wins without a current-finding selector", async () => {
    invoke.mockResolvedValue({ wins: [] })

    await getBurnCheckAggregateWins()

    expect(invoke).toHaveBeenCalledWith("get_burn_check_aggregate_wins")
  })

  it("uses independent report consumers and opaque sample handles", async () => {
    invoke.mockResolvedValue({ outcome: "opened" })

    await getChecksReport("main-burn-checks-1")
    await openBurnCheckSample("opaque-handle")

    expect(invoke).toHaveBeenNthCalledWith(1, "get_checks_report", {
      consumerId: "main-burn-checks-1",
    })
    expect(invoke).toHaveBeenNthCalledWith(2, "open_burn_check_sample", {
      navigationHandle: "opaque-handle",
    })
  })
})
