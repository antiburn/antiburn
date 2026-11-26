import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { CloudSession } from "./CloudSession"
import { CLOUD_BUSY_MESSAGE, emptyCloudSnapshot, type CloudSnapshot } from "../lib/cloud"
import { cloudProjectFixture } from "./__tests__/cloudProjectFixtures"

const analytics = vi.hoisted(() => ({ view: vi.fn(), action: vi.fn() }))
vi.mock("../lib/onboardingAnalytics", () => ({
  noteCloudScreenViewed: analytics.view,
  noteCloudAction: analytics.action,
}))

const signedIn: CloudSnapshot = {
  ...emptyCloudSnapshot,
  authEnabled: true,
  captureEnabled: true,
  status: "signed_in",
  identity: { id: "synthetic-account", login: "example" },
  destination: { id: "synthetic-personal", name: "Personal", kind: "personal" },
  draft: { screen: "folders", folders: [] },
}

function fixture(initial = emptyCloudSnapshot) {
  const { port, ongoing, emit } = cloudProjectFixture()
  emit(initial)
  const stop = vi.fn()
  const watch = port.watch
  port.watch = async (listener) => {
    const unsubscribe = await watch(listener)
    return () => {
      unsubscribe()
      stop()
    }
  }
  const read = port.read
  port.read = vi.fn(read)
  port.begin = vi.fn(async (): Promise<CloudSnapshot> => ({
    ...initial,
    status: initial.authEnabled ? "waiting" : "idle",
  }))
  port.cancel = vi.fn(async (): Promise<CloudSnapshot> => ({ ...initial, status: "cancelled" }))
  return { port, ongoing, stop, emit }
}

const settle = async () => {
  for (let i = 0; i < 24; i++) await Promise.resolve()
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
  const { port, ongoing } = fixture()
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
  await settle()
  await session.begin()
  await vi.advanceTimersByTimeAsync(60_000)
  expect(port.read).toHaveBeenCalledOnce()
  expect(ongoing.enable).not.toHaveBeenCalled()
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

it.each(["account", "ongoing", "workspace"] as const)(
  "stops a %s watch returned to a superseded load",
  async (kind) => {
    const { port, ongoing } = cloudProjectFixture()
    let resolveFirst!: (stop: () => void) => void
    const first = new Promise<() => void>((resolve) => {
      resolveFirst = resolve
    })
    const stopFirst = vi.fn()
    const stopCurrent = vi.fn()
    const watch = vi.fn<() => Promise<() => void>>()
    watch.mockReturnValueOnce(first).mockResolvedValueOnce(stopCurrent)
    if (kind === "account") port.watch = watch
    else if (kind === "ongoing") ongoing.watch = watch
    else port.watchWorkspace = watch

    const session = new CloudSession(port)
    const stop = session.subscribe(vi.fn())
    await settle()
    expect(watch).toHaveBeenCalledOnce()
    await session.load()
    resolveFirst(stopFirst)
    await settle()

    expect(watch).toHaveBeenCalledTimes(2)
    expect(stopFirst).toHaveBeenCalledOnce()
    expect(stopCurrent).not.toHaveBeenCalled()
    stop()
    expect(stopCurrent).toHaveBeenCalledOnce()
  },
)

it("reports a watch registration failure after an account event", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  let rejectWatch!: (error: Error) => void
  ongoing.watch.mockReturnValueOnce(
    new Promise<() => void>((_, reject) => {
      rejectWatch = reject
    }),
  )
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
  await settle()
  emit({ status: "signed_out", identity: null, destination: null })
  rejectWatch(new Error("Synthetic watch failure"))
  await settle()

  expect(session.getSnapshot()).toMatchObject({
    busy: false,
    error: "Could not load Cloud. Try again or continue locally.",
  })
  stop()
})

it("does not replace a newer verified event with a late browser launch response", async () => {
  const { port, emit } = fixture()
  let resolve!: (value: CloudSnapshot) => void
  port.begin = () =>
    new Promise<CloudSnapshot>((done) => {
      resolve = done
    })
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
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

it("keeps a newer sign-out when an older folder navigation finishes", async () => {
  const { port, emit } = fixture(signedIn)
  let resolve!: (value: CloudSnapshot) => void
  port.save = () =>
    new Promise<CloudSnapshot>((done) => {
      resolve = done
    })
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
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

it("keeps side-effect cancellation out of action analytics and upload consent", async () => {
  const { port, ongoing } = fixture({ ...emptyCloudSnapshot, status: "waiting" })
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
  await settle()
  await session.cancelSilently()
  expect(port.cancel).toHaveBeenCalledOnce()
  expect(analytics.action).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
  stop()
})

it.each(["browser_failed", "reauth_required"] as const)(
  "retries %s without cancelling browser approval",
  async (status) => {
    const { port } = fixture({
      ...emptyCloudSnapshot,
      status,
      authEnabled: status === "reauth_required",
    })
    const session = new CloudSession(port)
    const stop = session.subscribe(vi.fn())
    await settle()
    await session.retry()
    expect(port.begin).toHaveBeenCalledOnce()
    expect(port.cancel).not.toHaveBeenCalled()
    stop()
  },
)

it("ignores marketing expiry when authentication is disabled", async () => {
  vi.useFakeTimers()
  vi.setSystemTime(0)
  const { port } = fixture({ ...emptyCloudSnapshot, status: "waiting", expiresAt: 1 })
  const session = new CloudSession(port)
  const stop = session.subscribe(vi.fn())
  await settle()
  await vi.advanceTimersByTimeAsync(60_000)
  expect(port.read).toHaveBeenCalledOnce()
  expect(port.cancel).not.toHaveBeenCalled()
  stop()
})

it.each([0, 2_000])(
  "does not poll an elapsed browser deadline after a busy status read from clock %i",
  async (clock) => {
    vi.useFakeTimers()
    vi.setSystemTime(clock)
    const waiting: CloudSnapshot = {
      ...emptyCloudSnapshot,
      authEnabled: true,
      status: "waiting",
      expiresAt: 1,
    }
    const { port, emit } = fixture(waiting)
    vi.mocked(port.read).mockResolvedValueOnce(waiting).mockRejectedValue(CLOUD_BUSY_MESSAGE)
    const session = new CloudSession(port, false)
    const stop = session.subscribe(vi.fn())
    await settle()
    await vi.advanceTimersByTimeAsync(5_000)
    const expectedReads = clock === 0 ? 2 : 1
    expect(port.read).toHaveBeenCalledTimes(expectedReads)
    session.setVisible(false)
    session.setVisible(true)
    emit(waiting)
    await vi.advanceTimersByTimeAsync(5_000)
    expect(port.read).toHaveBeenCalledTimes(expectedReads)
    stop()
  },
)

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

it("does not navigate an unsupported connection through a failing draft save", async () => {
  vi.useFakeTimers()
  const { port, ongoing } = fixture({
    ...signedIn,
    draft: { screen: "handoff", folders: [] },
    setupError: "Desktop capture is not supported on this platform.",
  })
  const save = vi.spyOn(port, "save")
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  await settle()
  await vi.advanceTimersByTimeAsync(4000)
  expect(save).not.toHaveBeenCalled()
  await session.startSync()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(session.getSnapshot()).toMatchObject({ screen: "identity", error: null })
  stop()
})

it.each([
  [CLOUD_BUSY_MESSAGE, CLOUD_BUSY_MESSAGE],
  ["synthetic private provider detail", "Cloud could not complete that action. Try again."],
])("keeps authority and folder choices when a draft save fails", async (error, message) => {
  const { port, ongoing, emit } = fixture(signedIn)
  port.save = vi.fn(async () => {
    throw new Error(error)
  })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  await settle()
  await session.go("folders")
  expect(session.getSnapshot()).toMatchObject({ error: message, cloud: signedIn })
  expect(ongoing.enable).not.toHaveBeenCalled()
  emit(signedIn)
  expect(session.getSnapshot().error).toBe(error === CLOUD_BUSY_MESSAGE ? null : message)
  stop()
})

it("invalidates a rendered Start on every surface when a status read closes authority", async () => {
  const { port, ongoing } = fixture(signedIn)
  let cloud = signedIn
  const listeners = new Set<(value: CloudSnapshot) => void>()
  port.watch = async (listener) => {
    listeners.add(listener)
    return () => listeners.delete(listener)
  }
  port.read = async () => {
    for (const listener of listeners) listener(cloud)
    return cloud
  }
  const first = new CloudSession(port, false, true)
  const second = new CloudSession(port, false, true)
  const stopFirst = first.subscribe(vi.fn())
  const stopSecond = second.subscribe(vi.fn())
  await settle()
  const confirmation = first.getSnapshot()
  cloud = { ...emptyCloudSnapshot, authEnabled: true, status: "reauth_required" }
  await second.load()
  expect(first.getSnapshot()).toMatchObject({
    cloud: { identity: null, status: "reauth_required" },
  })
  await first.startSync(confirmation)
  expect(ongoing.enable).not.toHaveBeenCalled()
  stopFirst()
  stopSecond()
})
