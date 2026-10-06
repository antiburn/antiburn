import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { RemoteHost, RemoteSyncStatus } from "./remoteHosts"

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  handlers: new Map<string, (event: { payload: unknown }) => void>(),
  stops: [] as Array<() => void>,
}))

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
  isTauri: () => true,
}))

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
}))

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

function host(id: string): RemoteHost {
  return {
    id,
    sshAlias: id,
    displayName: null,
    status: "idle",
    lastSuccessfulSyncEpoch: null,
    automaticSyncEnabled: true,
    cachedSessionCount: 0,
    lastError: null,
  }
}

const sync: RemoteSyncStatus = {
  intervalSecs: 300,
  active: null,
  pendingHostIds: [],
}

beforeEach(() => {
  vi.resetModules()
  vi.clearAllMocks()
  mocks.handlers.clear()
  mocks.stops.length = 0
  mocks.listen.mockImplementation(
    async (name: string, handler: (event: { payload: unknown }) => void) => {
      mocks.handlers.set(name, handler)
      const stop = vi.fn(() => mocks.handlers.delete(name))
      mocks.stops.push(stop)
      return stop
    },
  )
})

afterEach(() => {
  for (const stop of mocks.stops) stop()
})

it("sends only host identity and the requested automatic-sync setting", async () => {
  const { setRemoteHostSyncEnabled } = await import("./remoteHosts")
  await setRemoteHostSyncEnabled("host-a", false)
  expect(mocks.invoke).toHaveBeenCalledWith("set_remote_host_sync_enabled", {
    id: "host-a",
    enabled: false,
  })
})

describe("remoteHosts external store", () => {
  it("keeps a newer authoritative host event over an in-flight initial read", async () => {
    const initialHosts = deferred<RemoteHost[]>()
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_remote_hosts") return initialHosts.promise
      if (command === "get_remote_sync_status") return Promise.resolve(sync)
      throw new Error(`Unexpected command: ${command}`)
    })
    const { remoteHosts } = await import("./remoteHosts")
    const notify = vi.fn()
    const unsubscribe = remoteHosts.subscribe(notify)
    await vi.waitFor(() => expect(mocks.handlers.has("remote-hosts-changed")).toBe(true))
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("get_remote_hosts"))

    mocks.handlers.get("remote-hosts-changed")!({ payload: [host("current")] })
    initialHosts.resolve([host("removed")])
    await vi.waitFor(() => expect(remoteHosts.getSnapshot().loading).toBe(false))

    expect(remoteHosts.getSnapshot().hosts.map((item) => item.id)).toEqual(["current"])
    expect(remoteHosts.getSnapshot().loaded).toBe(true)
    unsubscribe()
  })

  it("keeps a newer host event over an explicit refresh response", async () => {
    const refreshHosts = deferred<RemoteHost[]>()
    let hostRead = 0
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_remote_hosts") {
        hostRead += 1
        return hostRead === 1 ? Promise.resolve([host("initial")]) : refreshHosts.promise
      }
      if (command === "get_remote_sync_status") return Promise.resolve(sync)
      throw new Error(`Unexpected command: ${command}`)
    })
    const { remoteHosts } = await import("./remoteHosts")
    const unsubscribe = remoteHosts.subscribe(() => undefined)
    await vi.waitFor(() => expect(remoteHosts.getSnapshot().loaded).toBe(true))

    const refresh = remoteHosts.refresh()
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledTimes(3))
    mocks.handlers.get("remote-hosts-changed")!({ payload: [] })
    refreshHosts.resolve([host("stale")])
    await refresh

    expect(remoteHosts.getSnapshot().hosts).toEqual([])
    expect(remoteHosts.getSnapshot().loading).toBe(false)
    unsubscribe()
  })

  it("keeps pushed sync status when an older interval response arrives", async () => {
    const response = deferred<RemoteSyncStatus>()
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_remote_hosts") return Promise.resolve([])
      if (command === "get_remote_sync_status") return Promise.resolve(sync)
      if (command === "set_remote_sync_interval") return response.promise
      throw new Error(command)
    })
    const { remoteHosts } = await import("./remoteHosts")
    const stop = remoteHosts.subscribe(() => undefined)
    await vi.waitFor(() => expect(remoteHosts.getSnapshot().loaded).toBe(true))
    const change = remoteHosts.setInterval(60)
    const current: RemoteSyncStatus = {
      intervalSecs: 900,
      active: { hostId: "current", completed: 1, total: 2 },
      pendingHostIds: ["next"],
    }
    mocks.handlers.get("remote-sync-status")!({ payload: current })
    response.resolve({ ...sync, intervalSecs: 60 })
    await change
    expect(remoteHosts.getSnapshot().sync).toEqual(current)
    stop()
  })

  it("ignores interval responses from a disposed subscription generation", async () => {
    const response = deferred<RemoteSyncStatus>()
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_remote_hosts") return Promise.resolve([])
      if (command === "set_remote_sync_interval") return response.promise
      return Promise.resolve(sync)
    })
    const { remoteHosts } = await import("./remoteHosts")
    const stop = remoteHosts.subscribe(() => undefined)
    await vi.waitFor(() => expect(remoteHosts.getSnapshot().loaded).toBe(true))
    const change = remoteHosts.setInterval(60)
    stop()
    const restarted = remoteHosts.subscribe(() => undefined)
    response.resolve({ ...sync, intervalSecs: 60 })
    await change
    expect(remoteHosts.getSnapshot().sync).toEqual(sync)
    restarted()
  })

  it("contains listener setup failures and cleans up successful listeners", async () => {
    mocks.listen.mockImplementation(
      async (name: string, handler: (event: { payload: unknown }) => void) => {
        if (name === "remote-hosts-changed") throw new Error("listener unavailable")
        mocks.handlers.set(name, handler)
        const stop = vi.fn()
        mocks.stops.push(stop)
        return stop
      },
    )
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_remote_hosts") return Promise.resolve([])
      if (command === "get_remote_sync_status") return Promise.resolve(sync)
      throw new Error(`Unexpected command: ${command}`)
    })
    const { remoteHosts } = await import("./remoteHosts")
    const unsubscribe = remoteHosts.subscribe(() => undefined)
    await vi.waitFor(() => expect(remoteHosts.getSnapshot().loading).toBe(false))

    expect(remoteHosts.getSnapshot().loaded).toBe(true)
    unsubscribe()
    expect(mocks.stops).toHaveLength(2)
    for (const stop of mocks.stops) expect(stop).toHaveBeenCalledOnce()
  })

  it("refreshes saved counts after session removal and keeps disabled hosts browsable", async () => {
    const cached = { ...host("paused"), automaticSyncEnabled: false, cachedSessionCount: 2 }
    mocks.invoke.mockImplementation(async (command: string) =>
      command === "get_remote_hosts" ? [{ ...cached }] : sync,
    )
    const { remoteHosts } = await import("./remoteHosts")
    const unsubscribe = remoteHosts.subscribe(() => undefined)
    await vi.waitFor(() => expect(remoteHosts.getSnapshot().loaded).toBe(true))
    expect(remoteHosts.getSnapshot().hosts[0]?.cachedSessionCount).toBe(2)
    cached.cachedSessionCount = 1
    mocks.handlers.get("session:index-changed")!({ payload: { cause: "removed" } })
    await vi.waitFor(() =>
      expect(remoteHosts.getSnapshot().hosts[0]?.cachedSessionCount).toBe(1),
    )
    expect(remoteHosts.getSnapshot().hosts[0]?.automaticSyncEnabled).toBe(false)
    unsubscribe()
    expect(mocks.handlers.has("session:index-changed")).toBe(false)
  })
})
