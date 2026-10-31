import { beforeEach, expect, it, vi } from "vitest"

import { onAgentIntegrationsChanged, type IntegrationStatus } from "./agentIntegrationsIpc"

const api = vi.hoisted(() => ({ isTauri: vi.fn(), listen: vi.fn() }))
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: api.isTauri }))
vi.mock("./tauriEvents", () => ({ listen: api.listen }))

beforeEach(() => {
  vi.resetAllMocks()
  api.isTauri.mockReturnValue(true)
})

it("passes native integration snapshots to the store", async () => {
  const unlisten = vi.fn()
  let deliver!: (event: { payload: IntegrationStatus[] }) => void
  api.listen.mockImplementation(async (_event, callback: typeof deliver) => {
    deliver = callback
    return unlisten
  })
  const callback = vi.fn()
  const stop = await onAgentIntegrationsChanged(callback)
  const statuses: IntegrationStatus[] = [
    {
      vendor: "codex",
      phase: "registered",
      reason: "registered",
      hostVersion: null,
      profile: null,
      nodeVersion: null,
      owned: true,
    },
  ]
  deliver({ payload: statuses })
  expect(callback).toHaveBeenCalledWith(statuses)
  expect(api.listen).toHaveBeenCalledWith("agent-integrations:changed", expect.any(Function))
  stop()
  expect(unlisten).toHaveBeenCalledOnce()
})

it("does not attach a native listener in browser previews", async () => {
  api.isTauri.mockReturnValue(false)
  const stop = await onAgentIntegrationsChanged(vi.fn())
  expect(api.listen).not.toHaveBeenCalled()
  stop()
})
