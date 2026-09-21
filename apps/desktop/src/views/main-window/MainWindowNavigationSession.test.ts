import { beforeEach, describe, expect, it, vi } from "vitest"

import type { MainWindowNavigationRequest } from "../../lib/ipc"
import type { SessionFilter } from "../../lib/sessionFilters"
import type { SessionSubject } from "../../lib/sessionSubject"
import type { MainActivitySession } from "./MainActivitySession"
import { MainWindowNavigationSession } from "./MainWindowNavigationSession"

const mocks = vi.hoisted(() => ({
  acknowledge: vi.fn(),
  existing: vi.fn(),
  handler: null as ((request: MainWindowNavigationRequest) => void) | null,
  listen: vi.fn(),
  noteInteraction: vi.fn(),
  peek: vi.fn(),
  stop: vi.fn(),
}))

vi.mock("../../lib/ipc", () => ({
  acknowledgeMainWindowNavigationTarget: mocks.acknowledge,
  existingMainWindowSessionTargets: mocks.existing,
  noteInteraction: mocks.noteInteraction,
  onMainWindowNavigationTarget: mocks.listen,
  peekMainWindowNavigationTarget: mocks.peek,
}))

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

function subject(sessionId: string): SessionSubject & { wslDistro: null } {
  return { agent: "codex", sessionId, wslDistro: null }
}

class FakeActivitySession {
  onNavigation?: (origin: "user" | "automatic") => void
  onDeleted?: (subject: SessionSubject) => void
  onSessionInventoryInvalidated?: () => void
  private snapshot: { filter: SessionFilter; subject: SessionSubject | null } = {
    filter: { kind: "all" },
    subject: null,
  }
  restoreNavigation = vi.fn(
    (filter: SessionFilter, selected: SessionSubject | null, _origin: "user" | "automatic") => {
      this.snapshot = { filter, subject: selected }
    },
  )
  getSnapshot = () => this.snapshot
  select(selected: SessionSubject, origin: "user" | "automatic" = "user"): void {
    this.snapshot = { ...this.snapshot, subject: selected }
    this.onNavigation?.(origin)
  }
  filter(filter: SessionFilter): void {
    this.snapshot = { ...this.snapshot, filter }
    this.onNavigation?.("user")
  }
  delete(selected: SessionSubject): void {
    if (this.snapshot.subject?.sessionId === selected.sessionId) {
      this.snapshot = { ...this.snapshot, subject: null }
    }
    this.onDeleted?.(selected)
  }
  removeDeletedSubject = (selected: SessionSubject): void => {
    if (this.snapshot.subject?.sessionId !== selected.sessionId) return
    this.snapshot = { ...this.snapshot, subject: null }
    this.onDeleted?.(selected)
  }
  invalidate(): void {
    this.onSessionInventoryInvalidated?.()
  }
}

function setup() {
  const activity = new FakeActivitySession()
  const session = new MainWindowNavigationSession(activity as unknown as MainActivitySession)
  return { activity, session }
}

beforeEach(() => {
  vi.clearAllMocks()
  mocks.handler = null
  mocks.peek.mockResolvedValue(null)
  mocks.acknowledge.mockResolvedValue(undefined)
  mocks.existing.mockImplementation(async (targets) => targets)
  mocks.listen.mockImplementation(
    async (handler: (request: MainWindowNavigationRequest) => void) => {
      mocks.handler = handler
      return mocks.stop
    },
  )
  Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
    configurable: true,
    value: 7,
  })
})

describe("MainWindowNavigationSession", () => {
  it("records selection, filter, related, and adjacent destinations and restores them", () => {
    const { activity, session } = setup()
    session.select("activity")
    activity.select(subject("selected"))
    activity.filter({ kind: "notable" })
    activity.select(subject("related"))
    activity.select(subject("adjacent"))

    expect(session.getSnapshot().destination).toEqual({
      section: "activity",
      filter: { kind: "notable" },
      subject: subject("adjacent"),
    })

    session.back()
    expect(activity.getSnapshot()).toEqual({
      filter: { kind: "notable" },
      subject: subject("related"),
    })
    session.back()
    expect(activity.getSnapshot().subject).toEqual(subject("selected"))
    session.forward()
    expect(activity.getSnapshot().subject).toEqual(subject("related"))
    expect(mocks.noteInteraction.mock.calls).toEqual([
      [{ kind: "navigationHistoryMoved", direction: "back" }],
      [{ kind: "navigationHistoryMoved", direction: "back" }],
      [{ kind: "navigationHistoryMoved", direction: "forward" }],
    ])
  })

  it("replaces automatic selection without creating history or changing another section", () => {
    const { activity, session } = setup()
    session.select("activity")
    activity.select(subject("automatic"), "automatic")

    session.back()
    expect(session.getSnapshot().selected).toBe("overview")
    expect(session.getSnapshot().canBack).toBe(false)
    expect(session.getSnapshot().canForward).toBe(true)

    activity.select(subject("background"), "automatic")
    session.forward()
    expect(session.getSnapshot().destination.subject).toEqual(subject("automatic"))
  })

  it("deduplicates history, refreshes deliberate destinations, and truncates forward", () => {
    const { session } = setup()
    session.navigate({ section: "burnChecks", check: "modelOverthinking" })
    const firstRevision = session.getSnapshot().destinationRevision
    session.navigate({ section: "burnChecks", check: "modelOverthinking" })
    expect(session.getSnapshot().destinationRevision).toBe(firstRevision + 1)

    session.back()
    expect(session.getSnapshot().selected).toBe("overview")
    session.navigate({ section: "burnChecks", check: "oldModelUsage" })
    expect(session.getSnapshot().canForward).toBe(false)
    session.back()
    expect(session.getSnapshot().canBack).toBe(false)
  })

  it("retains at most one hundred destinations", () => {
    const { session } = setup()
    for (let index = 0; index < 110; index += 1) {
      session.navigate({
        section: "activity",
        filter: { kind: "all" },
        subject: subject(`session-${index}`),
      })
    }
    mocks.noteInteraction.mockClear()

    let moves = 0
    while (session.getSnapshot().canBack) {
      session.back()
      moves += 1
    }

    expect(moves).toBe(99)
    expect(mocks.noteInteraction).toHaveBeenCalledTimes(99)
    expect(session.getSnapshot().destination.subject).toEqual(subject("session-10"))
  })

  it("prunes every deleted destination and restores a consistent current subject", () => {
    const { activity, session } = setup()
    session.navigate({ section: "activity", filter: { kind: "all" }, subject: subject("kept") })
    session.navigate({
      section: "activity",
      filter: { kind: "all" },
      subject: subject("deleted"),
    })
    session.navigate({ section: "burnChecks" })
    session.navigate({
      section: "activity",
      filter: { kind: "notable" },
      subject: subject("deleted"),
    })

    activity.delete(subject("deleted"))

    expect(session.getSnapshot().destination).toEqual({
      section: "burnChecks",
    })
    expect(session.getSnapshot().canForward).toBe(false)
    session.back()
    expect(activity.getSnapshot().subject).toEqual(subject("kept"))
    session.forward()
    expect(session.getSnapshot().selected).toBe("burnChecks")
  })

  it("retries deletion reconciliation after navigation changes during the check", async () => {
    const first = deferred<ReturnType<typeof subject>[]>()
    mocks.existing.mockReturnValueOnce(first.promise).mockResolvedValue([subject("new")])
    const { activity, session } = setup()
    session.navigate({ section: "activity", subject: subject("deleted-a") })
    session.navigate({ section: "activity", subject: subject("deleted-b") })

    activity.invalidate()
    await vi.waitFor(() => expect(mocks.existing).toHaveBeenCalledTimes(1))
    session.navigate({ section: "activity", subject: subject("new") })
    first.resolve([])

    await vi.waitFor(() => expect(mocks.existing).toHaveBeenCalledTimes(2))
    await vi.waitFor(() =>
      expect(session.getSnapshot().destination.subject).toEqual(subject("new")),
    )
    session.back()
    expect(session.getSnapshot()).toEqual(
      expect.objectContaining({ selected: "overview", canBack: false }),
    )
  })

  it("discards an existence result superseded by a newer invalidation", async () => {
    const stale = deferred<ReturnType<typeof subject>[]>()
    mocks.existing.mockReturnValueOnce(stale.promise).mockResolvedValue([])
    const { activity, session } = setup()
    session.navigate({ section: "activity", subject: subject("deleted") })

    activity.invalidate()
    await vi.waitFor(() => expect(mocks.existing).toHaveBeenCalledTimes(1))
    activity.invalidate()
    stale.resolve([subject("deleted")])

    await vi.waitFor(() => expect(mocks.existing).toHaveBeenCalledTimes(2))
    await vi.waitFor(() => expect(session.getSnapshot().selected).toBe("overview"))
    expect(session.getSnapshot().canBack).toBe(false)
  })

  it("clears the current subject when Back moves during deletion reconciliation", async () => {
    const pending = deferred<ReturnType<typeof subject>[]>()
    mocks.existing.mockReturnValueOnce(pending.promise)
    const { activity, session } = setup()
    session.navigate({ section: "activity", subject: subject("deleted-a") })
    session.navigate({ section: "activity", subject: subject("deleted-b") })

    activity.invalidate()
    await vi.waitFor(() => expect(mocks.existing).toHaveBeenCalledTimes(1))
    session.back()
    expect(activity.getSnapshot().subject).toEqual(subject("deleted-a"))
    pending.resolve([])

    await vi.waitFor(() => expect(session.getSnapshot().selected).toBe("overview"))
    expect(activity.getSnapshot().subject).toBeNull()
  })

  it("applies one correlated external destination and acknowledges it", async () => {
    const { activity, session } = setup()
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(mocks.handler).not.toBeNull())

    mocks.handler!({
      revision: 1,
      destination: { section: "activity", target: subject("external") },
    })

    expect(session.getSnapshot().destination).toEqual({
      section: "activity",
      filter: { kind: "all" },
      subject: subject("external"),
    })
    expect(activity.getSnapshot().subject).toEqual(subject("external"))
    expect(mocks.acknowledge).toHaveBeenCalledWith(7, 1)
    session.back()
    expect(session.getSnapshot().selected).toBe("overview")
    expect(session.getSnapshot().canBack).toBe(false)
    stop()
  })

  it("keeps the newest target across event and peek ordering races", async () => {
    const pending = deferred<MainWindowNavigationRequest | null>()
    mocks.peek.mockReturnValue(pending.promise)
    const { session } = setup()
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(mocks.handler).not.toBeNull())

    mocks.handler!({
      revision: 2,
      destination: { section: "activity", target: subject("new") },
    })
    pending.resolve({
      revision: 1,
      destination: { section: "activity", target: subject("old") },
    })
    await vi.waitFor(() =>
      expect(session.getSnapshot().destination.subject).toEqual(subject("new")),
    )
    expect(mocks.acknowledge).toHaveBeenCalledWith(7, 2)
    expect(mocks.acknowledge).not.toHaveBeenCalledWith(7, 1)
    stop()
  })

  it("retries acknowledgement without replaying a duplicate target", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined)
    mocks.acknowledge.mockRejectedValueOnce(new Error("unavailable"))
    const { session } = setup()
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(mocks.handler).not.toBeNull())
    const request: MainWindowNavigationRequest = {
      revision: 3,
      destination: { section: "burnChecks", target: null },
    }

    mocks.handler!(request)
    const appliedRevision = session.getSnapshot().destinationRevision
    await vi.waitFor(() => expect(consoleError).toHaveBeenCalled())
    mocks.handler!(request)

    expect(session.getSnapshot().destinationRevision).toBe(appliedRevision)
    expect(mocks.acknowledge).toHaveBeenCalledTimes(2)
    stop()
    consoleError.mockRestore()
  })

  it("notifies once for a fresh request to the current section and ignores stale requests", async () => {
    const session = new MainWindowNavigationSession()
    const changed = vi.fn()
    const unsubscribe = session.subscribe(changed)
    await vi.waitFor(() => expect(mocks.peek).toHaveBeenCalledOnce())
    mocks.handler!({ revision: 2, destination: { section: "overview", target: null } })
    expect(session.getSnapshot()).toMatchObject({ selected: "overview", requests: 1 })
    expect(changed).toHaveBeenCalledTimes(1)
    mocks.handler!({ revision: 1, destination: { section: "burnChecks", target: null } })
    expect(session.getSnapshot()).toMatchObject({ selected: "overview", requests: 1 })
    expect(changed).toHaveBeenCalledTimes(1)
    unsubscribe()
  })

  it("restores Limits through history without changing retained session selection", () => {
    const { session, activity } = setup()
    session.navigate({ section: "activity", subject: subject("retained") })
    session.select("quota")
    session.select("burnChecks")
    session.back()
    expect(session.getSnapshot().selected).toBe("quota")
    expect(activity.getSnapshot().subject).toEqual(subject("retained"))
    session.back()
    expect(session.getSnapshot().selected).toBe("activity")
    session.forward()
    expect(session.getSnapshot().selected).toBe("quota")
  })
})
