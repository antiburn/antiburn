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
    resetLocal: vi.fn(async () => ({
      ...emptyCloudSnapshot,
      status: "signed_out" as const,
      remoteRevocationUnconfirmed: true,
    })),
    save: vi.fn(async (draft) => {
      value = { ...value, draft }
      return value
    }),
    pick: vi.fn(async () => ["/Example/Project"]),
    agent: vi.fn(async () => "unavailable" as const),
    prepareSync: vi.fn(async () => {}),
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
  destination: { id: "synthetic-personal", name: "Personal", kind: "personal" },
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
it("selects a suggested folder when the picker returns its existing path", async () => {
  const { port } = fixture(signedIn)
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
  await settle()
  await session.addFolders()
  expect(session.getSnapshot().cloud.draft.folders).toEqual([
    { path: "/Example/Project", selected: true },
  ])
  expect(port.save).toHaveBeenCalledOnce()
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
  session.setFullContentConsent(true)
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
  session.setFullContentConsent(true)
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
it("drops folder picker results when the account changes while the picker is open", async () => {
  const { port, emit } = fixture(signedIn)
  let resolve!: (value: string[]) => void
  port.pick = () =>
    new Promise<string[]>((done) => {
      resolve = done
    })
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
  await settle()
  const pending = session.addFolders()
  emit({ ...signedIn, identity: { id: "another-account", login: "Another" } })
  resolve(["/Example/Late"])
  await pending
  expect(port.save).not.toHaveBeenCalled()
  stop()
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
  session.setFullContentConsent(true)
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
  session.setFullContentConsent(true)
  const pending = session.startSync()
  emit({ ...emptyCloudSnapshot, status: "signed_out" })
  resolve()
  await pending
  expect(port.start).not.toHaveBeenCalled()
  expect(session.getSnapshot().screen).toBe("handoff")
  stop()
})

it("requires full-content consent and binds it to the current destination", async () => {
  const { port, emit } = fixture({
    ...signedIn,
    syncEnabled: true,
    draft: { screen: "folders", folders: [{ path: "/Example/Project", selected: true }] },
  })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  await session.startSync()
  expect(port.start).not.toHaveBeenCalled()
  session.setFullContentConsent(true)
  await session.startSync()
  expect(port.start).toHaveBeenCalledWith(
    [{ path: "/Example/Project", selected: true }],
    expect.any(String),
    {
      version: "native-full-content-v1",
      accountId: "synthetic",
      destinationId: "synthetic-personal",
      fullContent: true,
      attachments: false,
      selectedRoots: ["/Example/Project"],
    },
  )
  emit({ ...signedIn, destination: { id: "team-a", name: "Example team", kind: "team" } })
  expect(session.getSnapshot().fullContentConsent).toBe(false)
  stop()
})

it("does not submit a prepared capture to a changed destination", async () => {
  const { port, emit } = fixture({
    ...signedIn,
    syncEnabled: true,
    draft: { screen: "folders", folders: [{ path: "/Example/Project", selected: true }] },
  })
  let finishPrepare!: () => void
  port.prepareSync = () =>
    new Promise((resolve) => {
      finishPrepare = resolve
    })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  const pending = session.startSync()
  emit({ ...signedIn, destination: { id: "team-a", name: "Example team", kind: "team" } })
  finishPrepare()
  await pending
  expect(port.start).not.toHaveBeenCalled()
  stop()
})

it("keeps consent for count updates but clears it when selected sources change", async () => {
  const initial: CloudSnapshot = {
    ...signedIn,
    draft: { screen: "folders", folders: [{ path: "/Example/Project", selected: true }] },
  }
  const { port, emit } = fixture(initial)
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  emit({
    ...initial,
    draft: {
      ...initial.draft,
      folders: [{ path: "/Example/Project", selected: true, sessionCount: 2 }],
    },
  })
  expect(session.getSnapshot().fullContentConsent).toBe(true)
  emit({
    ...initial,
    draft: {
      ...initial.draft,
      folders: [
        { path: "/Example/Project", selected: true },
        { path: "/Example/Another", selected: true },
      ],
    },
  })
  expect(session.getSnapshot().fullContentConsent).toBe(false)
  stop()
})

it("does not submit a prepared capture after its source selection changes", async () => {
  const { port, emit } = fixture({
    ...signedIn,
    syncEnabled: true,
    draft: { screen: "folders", folders: [{ path: "/Example/Project", selected: true }] },
  })
  let finishPrepare!: () => void
  port.prepareSync = () =>
    new Promise((resolve) => {
      finishPrepare = resolve
    })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  const pending = session.startSync()
  emit({
    ...signedIn,
    draft: { screen: "folders", folders: [{ path: "/Example/Another", selected: true }] },
  })
  finishPrepare()
  await pending
  expect(port.start).not.toHaveBeenCalled()
  stop()
})

it("does not submit a prepared capture after its content confirmation is cleared", async () => {
  const { port } = fixture({
    ...signedIn,
    syncEnabled: true,
    draft: { screen: "folders", folders: [{ path: "/Example/Project", selected: true }] },
  })
  let finishPrepare!: () => void
  port.prepareSync = () =>
    new Promise((resolve) => {
      finishPrepare = resolve
    })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  const pending = session.startSync()
  session.setFullContentConsent(false)
  finishPrepare()
  await pending
  expect(port.start).not.toHaveBeenCalled()
  stop()
})

it("blocks a team destination without selecting personal instead", async () => {
  const team = { id: "team-a", name: "Example team", kind: "team" as const }
  const { port } = fixture({
    ...signedIn,
    destination: team,
    syncEnabled: true,
    draft: { screen: "folders", folders: [{ path: "/Example/Project", selected: true }] },
  })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  await session.startSync()
  expect(port.start).not.toHaveBeenCalled()
  expect(session.getSnapshot().cloud.destination).toEqual(team)
  stop()
})

const nativeCapture: CloudSnapshot = {
  ...signedIn,
  captureEnabled: true,
  syncEnabled: false,
  draft: {
    screen: "complete",
    folders: [
      { path: "/Example/claude/projects/z", selected: true },
      { path: "/Example/claude/projects/a", selected: true },
      { path: "/Example/claude/projects/ignored", selected: false },
    ],
  },
}

it("saves a native capture request after onboarding with exact sorted selected roots", async () => {
  const { port } = fixture(nativeCapture)
  vi.mocked(port.start).mockResolvedValue({ status: "upload_unavailable" })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  await session.startSync()
  expect(port.prepareSync).not.toHaveBeenCalled()
  session.setFullContentConsent(true)
  await session.startSync()
  const folders = nativeCapture.draft.folders.filter((folder) => folder.selected)
  const consent = {
    version: "native-full-content-v1",
    accountId: "synthetic",
    destinationId: "synthetic-personal",
    fullContent: true,
    attachments: false,
    selectedRoots: ["/Example/claude/projects/a", "/Example/claude/projects/z"],
  }
  expect(port.prepareSync).toHaveBeenCalledExactlyOnceWith(folders, expect.any(String), consent)
  expect(port.start).toHaveBeenCalledWith(
    folders,
    vi.mocked(port.prepareSync).mock.calls[0]![1],
    consent,
  )
  expect(session.getSnapshot().sync).toEqual({ status: "upload_unavailable" })
  stop()
})

it("retries native preparation with the same request ID after an unconfirmed save", async () => {
  const { port } = fixture(nativeCapture)
  vi.mocked(port.prepareSync).mockRejectedValueOnce(new Error("write failed"))
  vi.mocked(port.start).mockResolvedValue({ status: "upload_unavailable" })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  await session.startSync()
  expect(session.getSnapshot().sync.status).toBe("ambiguous")
  await session.startSync()
  const calls = vi.mocked(port.prepareSync).mock.calls
  expect(calls).toHaveLength(2)
  expect(calls[1]![1]).toBe(calls[0]![1])
  expect(port.start).toHaveBeenCalledOnce()
  stop()
})

it("retries native source listing with the saved request ID without preparing it again", async () => {
  const { port } = fixture(nativeCapture)
  vi.mocked(port.start).mockRejectedValueOnce(new Error("response lost"))
  vi.mocked(port.start).mockResolvedValue({ status: "upload_unavailable" })
  vi.mocked(port.reconcile).mockResolvedValue({ status: "upload_unavailable" })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  await session.startSync()
  await session.startSync()
  expect(port.prepareSync).toHaveBeenCalledOnce()
  expect(port.start).toHaveBeenCalledTimes(2)
  expect(vi.mocked(port.start).mock.calls[0]![1]).toBe(vi.mocked(port.start).mock.calls[1]![1])
  expect(port.reconcile).not.toHaveBeenCalled()
  expect(session.getSnapshot().sync).toEqual({ status: "upload_unavailable" })
  stop()
})

it("clears consent before a delayed source edit can finish during preparation", async () => {
  const { port } = fixture(nativeCapture)
  let finishPrepare!: () => void
  let finishSave!: (value: CloudSnapshot) => void
  port.prepareSync = vi.fn(
    () =>
      new Promise<void>((done) => {
        finishPrepare = done
      }),
  )
  port.save = vi.fn(
    () =>
      new Promise<CloudSnapshot>((done) => {
        finishSave = done
      }),
  )
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  const preparation = session.startSync()
  const edit = session.selectFolder(nativeCapture.draft.folders[0]!.path, false)
  expect(session.getSnapshot().fullContentConsent).toBe(false)
  finishPrepare()
  await preparation
  expect(port.start).not.toHaveBeenCalled()
  finishSave({
    ...nativeCapture,
    draft: {
      ...nativeCapture.draft,
      folders: nativeCapture.draft.folders.map((folder, index) => ({
        ...folder,
        selected: index === 0 ? false : folder.selected,
      })),
    },
  })
  await edit
  stop()
})

it("reports listing failure after the native prepare event and retries the same saved request", async () => {
  const { port, emit } = fixture(nativeCapture)
  port.prepareSync = vi.fn(async (_folders, requestId) => {
    emit({
      ...nativeCapture,
      pendingSync: { requestId, result: { status: "upload_unavailable" } },
    })
  })
  vi.mocked(port.start).mockRejectedValueOnce(new Error("listing failed"))
  vi.mocked(port.start).mockResolvedValue({ status: "upload_unavailable" })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  await session.startSync()
  expect(session.getSnapshot()).toMatchObject({
    sync: { status: "upload_unavailable" },
    error: expect.stringContaining("Could not finish listing sources"),
  })
  const id = vi.mocked(port.prepareSync).mock.calls[0]![1]
  await session.startSync()
  expect(port.prepareSync).toHaveBeenCalledOnce()
  expect(port.start).toHaveBeenLastCalledWith(expect.any(Array), id, expect.any(Object))
  expect(port.start).toHaveBeenCalledTimes(2)
  expect(session.getSnapshot().error).toBeNull()
  stop()
})

it("restores a saved native request without content consent or a new submission", async () => {
  const { port } = fixture({
    ...nativeCapture,
    pendingSync: { requestId: "saved-capture", result: { status: "upload_unavailable" } },
  })
  vi.mocked(port.reconcile).mockResolvedValue({ status: "upload_unavailable" })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  expect(port.reconcile).toHaveBeenCalledExactlyOnceWith("saved-capture")
  expect(port.prepareSync).not.toHaveBeenCalled()
  expect(port.start).not.toHaveBeenCalled()
  expect(session.getSnapshot()).toMatchObject({
    screen: "complete",
    sync: { status: "upload_unavailable" },
    fullContentConsent: false,
  })
  stop()
})

it("loads agent readiness when reconciliation emits state before its reply", async () => {
  const saved: CloudSnapshot = {
    ...nativeCapture,
    pendingSync: { requestId: "saved", result: { status: "upload_unavailable" } },
  }
  const { port, emit } = fixture(saved)
  port.agent = vi.fn(async () => "ready" as const)
  port.reconcile = vi.fn(async () => {
    emit(saved)
    return { status: "upload_unavailable" as const }
  })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  expect(port.agent).toHaveBeenCalledOnce()
  expect(session.getSnapshot().agent).toBe("ready")
  expect(port.start).not.toHaveBeenCalled()
  stop()
})

it("clears consent on every observed surface when a status read closes authority", async () => {
  let value = nativeCapture
  const listeners = new Set<(next: CloudSnapshot) => void>()
  const { port } = fixture(nativeCapture)
  port.watch = async (listener) => {
    listeners.add(listener)
    return () => listeners.delete(listener)
  }
  port.read = async () => {
    for (const listener of listeners) listener(value)
    return value
  }
  const first = new CloudSession(port, false, true)
  const second = new CloudSession(port, false, true)
  const stopFirst = first.subscribe(vi.fn())
  const stopSecond = second.subscribe(vi.fn())
  await settle()
  first.setFullContentConsent(true)
  second.setFullContentConsent(true)
  value = {
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "reauth_required",
    revocationPending: true,
  }
  await second.load()
  expect(first.getSnapshot()).toMatchObject({
    cloud: { identity: null, status: "reauth_required" },
    fullContentConsent: false,
  })
  expect(second.getSnapshot().fullContentConsent).toBe(false)
  await first.startSync()
  expect(port.prepareSync).not.toHaveBeenCalled()
  stopFirst()
  stopSecond()
})

it("does not navigate an unsupported connection through a failing draft save", async () => {
  vi.useFakeTimers()
  const { port } = fixture({
    ...signedIn,
    draft: { screen: "handoff", folders: [] },
    setupError: "Native capture is not supported on this platform.",
  })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  await vi.advanceTimersByTimeAsync(4000)
  expect(port.save).not.toHaveBeenCalled()
  expect(session.getSnapshot()).toMatchObject({
    screen: "identity",
    error: null,
    cloud: { setupError: "Native capture is not supported on this platform." },
  })
  stop()
})

it.each(["signed_out", "reauth_required", "credential_failed"] as const)(
  "blocks a native save outside the current signed-in context: %s",
  async (status) => {
    const { port } = fixture({ ...nativeCapture, status })
    const session = new CloudSession(port, false, true)
    const stop = session.subscribe(vi.fn())
    await settle()
    session.setFullContentConsent(true)
    await session.startSync()
    expect(port.prepareSync).not.toHaveBeenCalled()
    stop()
  },
)

it("blocks a native save when setup could not load", async () => {
  const { port } = fixture({ ...nativeCapture, setupError: "Could not read Cloud setup." })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  await session.startSync()
  expect(port.prepareSync).not.toHaveBeenCalled()
  stop()
})

it("does not start native capture after the window closes during preparation", async () => {
  const { port } = fixture(nativeCapture)
  let finish!: () => void
  port.prepareSync = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve
      }),
  )
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  const pending = session.startSync()
  stop()
  finish()
  await pending
  expect(port.start).not.toHaveBeenCalled()
})

it("does not reuse an earlier confirmation after consent is cleared and checked again", async () => {
  const { port } = fixture(nativeCapture)
  let finish!: () => void
  port.prepareSync = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve
      }),
  )
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  const pending = session.startSync()
  session.setFullContentConsent(false)
  session.setFullContentConsent(true)
  finish()
  await pending
  expect(port.start).not.toHaveBeenCalled()
  stop()
})

it("fences late native results and creates a new request for changed roots", async () => {
  const { port, emit } = fixture(nativeCapture)
  let finish!: (result: Awaited<ReturnType<CloudPort["start"]>>) => void
  vi.mocked(port.start).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  session.setFullContentConsent(true)
  const pending = session.startSync()
  await settle()
  emit({
    ...nativeCapture,
    draft: {
      screen: "complete",
      folders: [{ path: "/Example/claude/projects/new", selected: true }],
    },
  })
  finish({ status: "upload_unavailable" })
  await pending
  expect(session.getSnapshot().fullContentConsent).toBe(false)
  expect(session.getSnapshot().sync).toEqual({ status: "unavailable" })
  session.setFullContentConsent(true)
  vi.mocked(port.start).mockResolvedValue({ status: "upload_unavailable" })
  await session.startSync()
  const calls = vi.mocked(port.prepareSync).mock.calls
  expect(calls).toHaveLength(2)
  expect(calls[1]![1]).not.toBe(calls[0]![1])
  expect(calls[1]![2].selectedRoots).toEqual(["/Example/claude/projects/new"])
  stop()
})
