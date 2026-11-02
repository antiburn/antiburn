import { beforeEach, expect, it, vi } from "vitest"

const api = vi.hoisted(() => ({
  load: vi.fn(),
  scan: { running: false, finishedAt: null as string | null },
  listeners: new Set<() => void>(),
}))
vi.mock("./ipc", () => ({ agentSessionLocations: api.load }))
vi.mock("./scanStatusStore", () => ({
  scanStatusStore: {
    getSnapshot: () => api.scan,
    subscribe: (listener: () => void) => {
      api.listeners.add(listener)
      return () => api.listeners.delete(listener)
    },
  },
}))

const roots = (found: boolean) => [{ agent: "codex", locations: [{ path: "~/.codex", found }] }]

function finishScan(finishedAt = "2026-10-30T12:00:00Z") {
  api.scan = { running: false, finishedAt }
  for (const listener of api.listeners) listener()
}

beforeEach(() => {
  vi.resetModules()
  api.load.mockReset()
  api.scan = { running: false, finishedAt: null }
  api.listeners.clear()
})

it("reads roots and follows shared scans only while the locations are used", async () => {
  const store = await import("./agentSessionLocationsStore")
  api.load.mockResolvedValue(roots(false))
  expect(api.load).not.toHaveBeenCalled()
  expect(api.listeners.size).toBe(0)
  const stop = store.subscribeAgentSessionLocations(vi.fn())
  const stopAgain = store.subscribeAgentSessionLocations(vi.fn())
  await vi.waitFor(() => expect(store.agentSessionLocationsSnapshot()).toEqual(roots(false)))
  expect(api.load).toHaveBeenCalledOnce()
  expect(api.listeners.size).toBe(1)
  stop()
  expect(api.listeners.size).toBe(1)
  stopAgain()
  expect(api.listeners.size).toBe(0)
  finishScan()
  expect(api.load).toHaveBeenCalledOnce()
})

it("detects a new empty root after a shared scan completes", async () => {
  const store = await import("./agentSessionLocationsStore")
  api.load.mockResolvedValueOnce(roots(false)).mockResolvedValueOnce(roots(true))
  const notify = vi.fn()
  const stop = store.subscribeAgentSessionLocations(notify)
  await vi.waitFor(() => expect(store.agentSessionLocationsSnapshot()).toEqual(roots(false)))
  api.scan = { ...api.scan, running: true }
  for (const listener of api.listeners) listener()
  expect(api.load).toHaveBeenCalledOnce()
  finishScan()
  await vi.waitFor(() => expect(store.agentSessionLocationsSnapshot()).toEqual(roots(true)))
  expect(api.load).toHaveBeenCalledTimes(2)
  expect(notify).toHaveBeenCalledTimes(2)
  stop()
})

it("reads roots again on return after a Sessions rescan while Agents was closed", async () => {
  const store = await import("./agentSessionLocationsStore")
  api.load.mockResolvedValueOnce(roots(false)).mockResolvedValueOnce(roots(true))
  const stop = store.subscribeAgentSessionLocations(vi.fn())
  await vi.waitFor(() => expect(store.agentSessionLocationsSnapshot()).toEqual(roots(false)))
  stop()
  finishScan()
  expect(api.load).toHaveBeenCalledOnce()
  const stopAgain = store.subscribeAgentSessionLocations(vi.fn())
  await vi.waitFor(() => expect(store.agentSessionLocationsSnapshot()).toEqual(roots(true)))
  expect(api.load).toHaveBeenCalledTimes(2)
  stopAgain()
})

it("retries after a scan overlaps an initial root read", async () => {
  const store = await import("./agentSessionLocationsStore")
  let finish!: (value: ReturnType<typeof roots>) => void
  api.load.mockImplementationOnce(
    () => new Promise<ReturnType<typeof roots>>((resolve) => (finish = resolve)),
  )
  api.load.mockResolvedValueOnce(roots(true))
  const stop = store.subscribeAgentSessionLocations(vi.fn())
  finishScan()
  finish(roots(false))
  await vi.waitFor(() => expect(store.agentSessionLocationsSnapshot()).toEqual(roots(true)))
  expect(api.load).toHaveBeenCalledTimes(2)
  stop()
})

it("reconciles a scan that completes during a root refresh", async () => {
  const store = await import("./agentSessionLocationsStore")
  let finish!: (value: ReturnType<typeof roots>) => void
  api.load.mockResolvedValueOnce(roots(false))
  const stop = store.subscribeAgentSessionLocations(vi.fn())
  await vi.waitFor(() => expect(store.agentSessionLocationsSnapshot()).toEqual(roots(false)))
  api.load.mockImplementationOnce(
    () => new Promise<ReturnType<typeof roots>>((resolve) => (finish = resolve)),
  )
  api.load.mockResolvedValueOnce(roots(true))
  finishScan()
  finishScan("2026-10-30T12:01:00Z")
  finish(roots(false))
  await vi.waitFor(() => expect(store.agentSessionLocationsSnapshot()).toEqual(roots(true)))
  expect(api.load).toHaveBeenCalledTimes(3)
  stop()
})

it("keeps the prior snapshot after a refresh fails and retries at the next scan", async () => {
  const store = await import("./agentSessionLocationsStore")
  api.load.mockResolvedValueOnce(roots(false))
  const stop = store.subscribeAgentSessionLocations(vi.fn())
  await vi.waitFor(() => expect(store.agentSessionLocationsSnapshot()).toEqual(roots(false)))
  api.load.mockRejectedValueOnce(new Error("read failed"))
  finishScan()
  await vi.waitFor(() => expect(api.load).toHaveBeenCalledTimes(2))
  expect(store.agentSessionLocationsSnapshot()).toEqual(roots(false))
  api.load.mockResolvedValueOnce(roots(true))
  finishScan("2026-10-30T12:01:00Z")
  await vi.waitFor(() => expect(store.agentSessionLocationsSnapshot()).toEqual(roots(true)))
  stop()
})

it("does not publish an old lookup after the last subscriber leaves", async () => {
  const store = await import("./agentSessionLocationsStore")
  let finish!: (value: ReturnType<typeof roots>) => void
  api.load.mockImplementationOnce(
    () => new Promise<ReturnType<typeof roots>>((resolve) => (finish = resolve)),
  )
  const notify = vi.fn()
  const stop = store.subscribeAgentSessionLocations(notify)
  stop()
  finish(roots(false))
  await Promise.resolve()
  expect(store.agentSessionLocationsSnapshot()).toBeNull()
  expect(notify).not.toHaveBeenCalled()
  api.load.mockResolvedValueOnce(roots(true))
  const stopAgain = store.subscribeAgentSessionLocations(notify)
  await vi.waitFor(() => expect(store.agentSessionLocationsSnapshot()).toEqual(roots(true)))
  expect(api.load).toHaveBeenCalledTimes(2)
  stopAgain()
})
