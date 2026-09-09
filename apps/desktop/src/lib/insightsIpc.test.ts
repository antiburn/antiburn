import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  autoFixBurnCheckTarget,
  copyPromptFixBurnCheckTarget,
  listBurnCheckTargets,
} from "./insightsIpc"

const invoke = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))

describe("burn check IPC", () => {
  beforeEach(() => invoke.mockReset())

  it("sends only the detector when listing", async () => {
    invoke.mockResolvedValue({ targets: [], truncated: false })

    await listBurnCheckTargets("oldModelUsage")

    expect(invoke).toHaveBeenCalledWith("list_burn_check_targets", {
      detector: "oldModelUsage",
    })
  })

  it("sends only the opaque target ID to each action", async () => {
    invoke
      .mockResolvedValueOnce({ outcome: "appliedAwaitingVerification", watchId: "watch-1" })
      .mockResolvedValueOnce({
        outcome: "promptReady",
        prompt: "Fix the target.",
        watch: {
          watchId: "watch-1",
          lifecycle: "watching",
          verification: { status: "watching" },
          savings: { status: "pending" },
        },
      })

    await autoFixBurnCheckTarget("target-1")
    await copyPromptFixBurnCheckTarget("target-1")

    expect(invoke).toHaveBeenNthCalledWith(1, "auto_fix_burn_check_target", {
      targetId: "target-1",
    })
    expect(invoke).toHaveBeenNthCalledWith(2, "copy_prompt_fix_burn_check_target", {
      targetId: "target-1",
    })
  })
})
