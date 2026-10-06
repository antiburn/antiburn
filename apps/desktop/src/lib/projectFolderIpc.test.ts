import { beforeEach, expect, it, vi } from "vitest"
import { openProjectFolder } from "./ipc"
import { listOverviewSessions, listRecentSessions } from "./sessionIpc"

const invoke = vi.hoisted(() => vi.fn().mockResolvedValue([]))
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
beforeEach(() => invoke.mockClear())

it("sends complete folder identities without a renderer path", async () => {
  const target = {
    kind: "session" as const,
    environmentKey: "wsl:ubuntu",
    agent: "codex",
    sessionId: "same",
  }
  await openProjectFolder(target)
  expect(invoke).toHaveBeenCalledWith("open_project_folder", { target })
  const check = { kind: "burnCheck" as const, actionId: "issued-action" }
  await openProjectFolder(check)
  expect(invoke).toHaveBeenLastCalledWith("open_project_folder", { target: check })
})

it("requests a local projection independently of the all-source Sessions list", async () => {
  await listOverviewSessions(7)
  await listRecentSessions(7)
  expect(invoke.mock.calls).toEqual([
    ["list_recent_sessions", { windowDays: 7, localOnly: true }],
    ["list_recent_sessions", { windowDays: 7 }],
  ])
})
