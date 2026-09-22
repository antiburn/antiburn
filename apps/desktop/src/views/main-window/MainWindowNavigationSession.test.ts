import { beforeEach, describe, expect, it, vi } from "vitest"

import { MainWindowNavigationSession } from "./MainWindowNavigationSession"

const mocks = vi.hoisted(() => ({
  listen: vi.fn(),
  take: vi.fn(),
}))

vi.mock("../../lib/ipc", () => ({
  onMainWindowSectionTarget: mocks.listen,
  takeMainWindowSectionTarget: mocks.take,
}))

beforeEach(() => {
  mocks.listen.mockReset()
  mocks.take.mockReset()
  mocks.take.mockResolvedValue(null)
})

describe("MainWindowNavigationSession", () => {
  it("applies live and pending section requests in revision order", async () => {
    const handlers: Array<
      (request: { revision: number; section: "overview" | "activity" | "burnChecks" }) => void
    > = []
    mocks.listen.mockImplementation(async (next: (typeof handlers)[number]) => {
      handlers.push(next)
      return vi.fn()
    })
    const session = new MainWindowNavigationSession()
    const changed = vi.fn()
    const unsubscribe = session.subscribe(changed)
    await vi.waitFor(() => expect(mocks.take).toHaveBeenCalledOnce())

    handlers[0]!({ revision: 2, section: "activity" })
    expect(session.getSnapshot()).toEqual({
      selected: "activity",
      visited: ["overview", "activity"],
      requests: 1,
    })
    handlers[0]!({ revision: 1, section: "burnChecks" })
    expect(session.getSnapshot().selected).toBe("activity")

    unsubscribe()
  })

  it("bumps requests and notifies on a request that retargets the section already selected", async () => {
    const handlers: Array<
      (request: { revision: number; section: "overview" | "activity" | "burnChecks" }) => void
    > = []
    mocks.listen.mockImplementation(async (next: (typeof handlers)[number]) => {
      handlers.push(next)
      return vi.fn()
    })
    const session = new MainWindowNavigationSession()
    const changed = vi.fn()
    const unsubscribe = session.subscribe(changed)
    await vi.waitFor(() => expect(mocks.take).toHaveBeenCalledOnce())

    handlers[0]!({ revision: 1, section: "overview" })
    expect(session.getSnapshot()).toEqual({
      selected: "overview",
      visited: ["overview"],
      requests: 1,
    })
    expect(changed).toHaveBeenCalledTimes(1)

    unsubscribe()
  })

  it("does not bump requests or notify on a stale revision", async () => {
    const handlers: Array<
      (request: { revision: number; section: "overview" | "activity" | "burnChecks" }) => void
    > = []
    mocks.listen.mockImplementation(async (next: (typeof handlers)[number]) => {
      handlers.push(next)
      return vi.fn()
    })
    const session = new MainWindowNavigationSession()
    const changed = vi.fn()
    const unsubscribe = session.subscribe(changed)
    await vi.waitFor(() => expect(mocks.take).toHaveBeenCalledOnce())

    handlers[0]!({ revision: 2, section: "activity" })
    expect(changed).toHaveBeenCalledTimes(1)

    handlers[0]!({ revision: 1, section: "burnChecks" })
    expect(session.getSnapshot()).toEqual({
      selected: "activity",
      visited: ["overview", "activity"],
      requests: 1,
    })
    expect(changed).toHaveBeenCalledTimes(1)

    unsubscribe()
  })
})
