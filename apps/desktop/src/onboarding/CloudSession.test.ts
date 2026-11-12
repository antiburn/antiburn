import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { CloudSession } from "./CloudSession"
import { emptyCloudSnapshot, type CloudPort, type CloudSnapshot } from "../lib/cloud"
const analytics = vi.hoisted(() => ({ view: vi.fn(), action: vi.fn() }))
vi.mock("../lib/onboardingAnalytics", () => ({
  noteCloudScreenViewed: analytics.view,
  noteCloudAction: analytics.action,
}))
function fixture(initial: CloudSnapshot = emptyCloudSnapshot) {
  let value = structuredClone(initial)
  let notify: (next: CloudSnapshot) => void = () => {}
  const stop = vi.fn()
  const port: CloudPort = {
    read: vi.fn(async () => value),
    watch: vi.fn(async (listener) => {
      notify = listener
      return stop
    }),
    begin: vi.fn(async () => {
      value = { ...value, status: value.authEnabled ? "waiting" : "idle" }
      return value
    }),
    cancel: vi.fn(async () => {
      value = { ...value, status: "cancelled" }
      return value
    }),
    signOut: vi.fn(async () => ({ ...emptyCloudSnapshot, status: "signed_out" as const })),
    save: vi.fn(async (draft) => {
      value = { ...value, draft }
      return value
    }),
    pick: vi.fn(async () => ["/Example/Project"]),
    agent: vi.fn(async () => "unavailable" as const),
    prepareSync: async () => {},
    start: vi.fn(async () => ({
      status: "accepted" as const,
      operationId: "synthetic-op",
      completed: 0,
      total: 1,
    })),
    reconcile: vi.fn(async () => ({
      status: "accepted" as const,
      operationId: "synthetic-op",
      completed: 0,
      total: 1,
    })),
    cancelSync: vi.fn(async () => ({ status: "cancelled" as const })),
  }
  return { port, stop, emit: (next: CloudSnapshot) => notify(next) }
}
const signedIn: CloudSnapshot = {
  ...emptyCloudSnapshot,
  authEnabled: true,
  status: "signed_in",
  identity: { id: "synthetic", login: "example" },
  draft: { screen: "folders", folders: [{ path: "/Example/Project", selected: false }] },
}
const settle = async () => {
  for (let i = 0; i < 12; i++) await Promise.resolve()
}
beforeEach(() => {
  vi.clearAllMocks()
  Object.defineProperty(document, "hidden", { configurable: true, value: false })
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})
it("shows marketing by default and never polls the browser or infers authentication", async () => {
  vi.useFakeTimers()
  const { port } = fixture()
  const session = new CloudSession(port),
    stop = session.subscribe(vi.fn())
  await settle()
  await session.begin()
  await vi.advanceTimersByTimeAsync(60_000)
  expect(port.read).toHaveBeenCalledTimes(1)
  expect(session.getSnapshot()).toMatchObject({
    screen: "handoff",
    cloud: { identity: null, status: "idle", syncEnabled: false },
  })
  stop()
})
it("deduplicates visible entries and resumes the actual draft without phantom identity views", async () => {
  const { port, stop: stopWatch } = fixture(signedIn)
  const session = new CloudSession(port)
  session.setVisible(false)
  const stop = session.subscribe(vi.fn())
  await settle()
  expect(analytics.view).not.toHaveBeenCalled()
  session.setVisible(true)
  session.setVisible(true)
  expect(analytics.view.mock.calls).toEqual([["folders"]])
  session.setVisible(false)
  session.setVisible(true)
  expect(analytics.view.mock.calls).toEqual([["folders"], ["folders"]])
  stop()
  expect(stopWatch).toHaveBeenCalledOnce()
})
it("selects added folders but keeps sync disabled after real identity", async () => {
  const { port } = fixture({ ...signedIn, draft: { screen: "folders", folders: [] } })
  const session = new CloudSession(port),
    stop = session.subscribe(vi.fn())
  await settle()
  await session.addFolders()
  await session.startSync()
  expect(session.getSnapshot().cloud.draft.folders).toEqual([
    { path: "/Example/Project", selected: true },
  ])
  expect(port.start).not.toHaveBeenCalled()
  await session.go("complete")
  expect(session.getSnapshot()).toMatchObject({
    screen: "complete",
    sync: { status: "unavailable" },
    agent: "unavailable",
  })
  stop()
})
it("does not replace a newer verified event with a late browser launch response", async () => {
  const { port, emit } = fixture()
  let resolve!: (value: CloudSnapshot) => void
  port.begin = vi.fn(
    () =>
      new Promise<CloudSnapshot>((done) => {
        resolve = done
      }),
  )
  const session = new CloudSession(port),
    stop = session.subscribe(vi.fn())
  await settle()
  const pending = session.begin()
  emit(signedIn)
  resolve({ ...emptyCloudSnapshot, status: "waiting" })
  await pending
  expect(session.getSnapshot()).toMatchObject({
    screen: "folders",
    cloud: { status: "signed_in" },
  })
  stop()
})
it("finishes onboarding while the background request is pending and submits only once", async () => {
  const { port } = fixture({
    ...signedIn,
    syncEnabled: true,
    draft: { screen: "folders", folders: [{ path: "/Example/Project", selected: true }] },
  })
  let resolve!: (value: Awaited<ReturnType<CloudPort["start"]>>) => void
  port.start = vi.fn(
    () =>
      new Promise<Awaited<ReturnType<CloudPort["start"]>>>((done) => {
        resolve = done
      }),
  )
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
  await settle()
  const pending = session.startSync()
  await settle()
  expect(session.getSnapshot()).toMatchObject({
    screen: "complete",
    busy: false,
    sync: { status: "starting" },
  })
  await session.startSync()
  expect(port.start).toHaveBeenCalledOnce()
  expect(analytics.action).toHaveBeenCalledExactlyOnceWith("folders", "start_sync")
  stop()
  resolve({ status: "accepted", operationId: "background", completed: 0, total: 1 })
  await pending
  expect(port.cancelSync).not.toHaveBeenCalled()
})
it("keeps ambiguous background requests on completion without resubmitting", async () => {
  const { port } = fixture({
    ...signedIn,
    syncEnabled: true,
    draft: { screen: "folders", folders: [{ path: "/Example/Project", selected: true }] },
  })
  port.start = vi.fn(async () => {
    throw Error("offline")
  })
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
  await settle()
  await session.startSync()
  await session.startSync()
  expect(session.getSnapshot()).toMatchObject({
    screen: "complete",
    busy: false,
    sync: { status: "ambiguous" },
  })
  expect(port.start).toHaveBeenCalledOnce()
  stop()
})
it("drops async picker results after the window closes", async () => {
  const { port } = fixture(signedIn)
  let resolve!: (value: string[]) => void
  port.pick = () =>
    new Promise<string[]>((done) => {
      resolve = done
    })
  const session = new CloudSession(port),
    stop = session.subscribe(vi.fn())
  await settle()
  const pending = session.addFolders()
  stop()
  resolve(["/Example/Late"])
  await pending
  expect(port.save).not.toHaveBeenCalled()
})
it("reconciles a persisted sync request after restart without starting another upload", async () => {
  const { port } = fixture({
    ...signedIn,
    syncEnabled: true,
    pendingSync: { requestId: "synthetic-pending", result: { status: "ambiguous" } },
  })
  const session = new CloudSession(port),
    stop = session.subscribe(vi.fn())
  await settle()
  expect(analytics.view.mock.calls).toEqual([["complete"]])
  expect(port.reconcile).toHaveBeenCalledWith("synthetic-pending")
  expect(port.start).not.toHaveBeenCalled()
  expect(session.getSnapshot()).toMatchObject({
    screen: "complete",
    sync: { status: "accepted" },
  })
  stop()
})
it("keeps a newer sign-out when an older folder navigation finishes", async () => {
  const { port, emit } = fixture(signedIn)
  let resolve!: (value: CloudSnapshot) => void
  port.save = () =>
    new Promise<CloudSnapshot>((done) => {
      resolve = done
    })
  const session = new CloudSession(port),
    stop = session.subscribe(vi.fn())
  await settle()
  const pending = session.go("complete")
  emit({ ...emptyCloudSnapshot, status: "signed_out" })
  resolve({ ...signedIn, draft: { ...signedIn.draft, screen: "complete" } })
  await pending
  expect(session.getSnapshot()).toMatchObject({
    screen: "handoff",
    cloud: { identity: null },
    sync: { status: "unavailable" },
  })
  stop()
})
it("drops a pending restart reconciliation after the account changes", async () => {
  const { port, emit } = fixture({
    ...signedIn,
    syncEnabled: true,
    pendingSync: { requestId: "synthetic-pending", result: { status: "ambiguous" } },
  })
  let resolve!: (value: Awaited<ReturnType<CloudPort["reconcile"]>>) => void
  port.reconcile = () =>
    new Promise((done) => {
      resolve = done
    })
  const session = new CloudSession(port),
    stop = session.subscribe(vi.fn())
  await settle()
  emit({ ...emptyCloudSnapshot, status: "signed_out" })
  resolve({ status: "accepted", operationId: "synthetic-old-account", completed: 0, total: 1 })
  await settle()
  expect(session.getSnapshot()).toMatchObject({
    screen: "handoff",
    cloud: { identity: null },
    sync: { status: "unavailable" },
  })
  stop()
})
it("keeps side-effect cancellation out of action analytics", async () => {
  const { port } = fixture({ ...emptyCloudSnapshot, status: "waiting" })
  const session = new CloudSession(port),
    stop = session.subscribe(vi.fn())
  await settle()
  await session.cancelSilently()
  expect(port.cancel).toHaveBeenCalledOnce()
  expect(analytics.action).not.toHaveBeenCalled()
  stop()
})

it("does not reconcile a stale saved request when a newer event arrives during initial read", async () => {
  const { port, emit } = fixture()
  let resolve!: (value: CloudSnapshot) => void
  port.read = vi.fn(
    () =>
      new Promise<CloudSnapshot>((done) => {
        resolve = done
      }),
  )
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
  await settle()
  emit({ ...emptyCloudSnapshot, status: "signed_out" })
  resolve({
    ...signedIn,
    syncEnabled: true,
    pendingSync: { requestId: "synthetic-stale-request", result: { status: "ambiguous" } },
  })
  await settle()
  expect(port.reconcile).not.toHaveBeenCalled()
  expect(port.start).not.toHaveBeenCalled()
  expect(session.getSnapshot()).toMatchObject({
    screen: "handoff",
    cloud: { identity: null, status: "signed_out" },
    sync: { status: "unavailable" },
    busy: false,
  })
  stop()
})

it("updates the current account's sync progress from events without resubmitting", async () => {
  const { port, emit } = fixture(signedIn)
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
  await settle()
  const operation = { operationId: "synthetic-operation", total: 2 }
  emit({
    ...signedIn,
    syncEnabled: true,
    pendingSync: {
      requestId: "synthetic-request",
      result: { ...operation, status: "syncing", completed: 1 },
    },
  })
  expect(session.getSnapshot()).toMatchObject({
    screen: "complete",
    sync: { status: "syncing", completed: 1 },
  })
  emit({
    ...signedIn,
    syncEnabled: true,
    pendingSync: {
      requestId: "synthetic-request",
      result: { ...operation, status: "complete", completed: 2 },
    },
  })
  expect(session.getSnapshot()).toMatchObject({
    screen: "complete",
    sync: { status: "complete", completed: 2 },
  })
  expect(port.reconcile).not.toHaveBeenCalled()
  expect(port.start).not.toHaveBeenCalled()
  emit({ ...signedIn, identity: { id: "synthetic-other", login: "another" } })
  expect(session.getSnapshot().sync).toEqual({ status: "unavailable" })
  stop()
})
it("reopens a failed marketing link without cancelling an auth attempt", async () => {
  const { port } = fixture({ ...emptyCloudSnapshot, status: "browser_failed" })
  const session = new CloudSession(port),
    stop = session.subscribe(vi.fn())
  await settle()
  await session.retry()
  expect(port.begin).toHaveBeenCalledOnce()
  expect(port.cancel).not.toHaveBeenCalled()
  expect(session.getSnapshot().cloud.status).toBe("idle")
  stop()
})
it("ignores legacy marketing expiry instead of scheduling auth status reads", async () => {
  vi.useFakeTimers()
  vi.setSystemTime(0)
  const { port } = fixture({ ...emptyCloudSnapshot, status: "waiting", expiresAt: 1 })
  const session = new CloudSession(port),
    stop = session.subscribe(vi.fn())
  await settle()
  await vi.advanceTimersByTimeAsync(60_000)
  expect(port.read).toHaveBeenCalledOnce()
  expect(port.cancel).not.toHaveBeenCalled()
  stop()
})

it("advances verified identity automatically and suspends the delay when hidden", async () => {
  vi.stubGlobal(
    "matchMedia",
    vi.fn(() => ({ matches: false })),
  )
  vi.useFakeTimers()
  const { port } = fixture({ ...signedIn, draft: { ...signedIn.draft, screen: "identity" } })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setVisible(false)
  await vi.advanceTimersByTimeAsync(4000)
  expect(session.getSnapshot().screen).toBe("identity")
  session.setVisible(true)
  await vi.advanceTimersByTimeAsync(3000)
  expect(session.getSnapshot().screen).toBe("folders")
  stop()
})

it("routes an accepted operation directly to completion and keeps later failures in the background", async () => {
  const { port, emit } = fixture({
    ...signedIn,
    syncEnabled: true,
    draft: { screen: "folders", folders: [{ path: "/Example/Project", selected: true }] },
  })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  await session.startSync()
  expect(session.getSnapshot().screen).toBe("complete")
  emit({
    ...signedIn,
    pendingSync: { requestId: "synthetic-request", result: { status: "failed" } },
  })
  expect(session.getSnapshot()).toMatchObject({
    screen: "complete",
    sync: { status: "failed" },
  })
  stop()
})

it("keeps untouched folder defaults when the picker is cancelled", async () => {
  const { port } = fixture(signedIn)
  vi.mocked(port.pick).mockResolvedValue([])
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  await session.addFolders()
  expect(port.save).not.toHaveBeenCalled()
  stop()
})

it("keeps Done available while a persisted request is reconciled", async () => {
  const { port } = fixture({
    ...signedIn,
    syncEnabled: true,
    pendingSync: { requestId: "saved", result: { status: "ambiguous" } },
  })
  port.reconcile = () => new Promise(() => {})
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
  await settle()
  expect(session.getSnapshot()).toMatchObject({ screen: "complete", busy: false })
  stop()
})
it("does not submit a prepared request after account changes", async () => {
  const { port, emit } = fixture({
    ...signedIn,
    syncEnabled: true,
    draft: { screen: "folders", folders: [{ path: "/Example/Project", selected: true }] },
  })
  let resolve!: () => void
  port.prepareSync = () =>
    new Promise((done) => {
      resolve = done
    })
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
  await settle()
  const pending = session.startSync()
  emit({ ...emptyCloudSnapshot, status: "signed_out" })
  resolve()
  await pending
  expect(port.start).not.toHaveBeenCalled()
  expect(session.getSnapshot().screen).toBe("handoff")
  stop()
})
