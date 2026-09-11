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
      (request: { revision: number; section: "activity" | "burnChecks" }) => void
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
      visited: ["burnChecks", "activity"],
    })
    handlers[0]!({ revision: 1, section: "burnChecks" })
    expect(session.getSnapshot().selected).toBe("activity")

    unsubscribe()
  })
})
