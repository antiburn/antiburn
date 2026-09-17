import { describe, expect, it, vi } from "vitest"

import type { LiveUsageSummaryPayload } from "../../lib/providerUsageIpc"
import {
  MainWindowLimitsSession,
  type MainWindowLimitsAdapter,
} from "./MainWindowLimitsSession"

const liveUsage = (generatedAt: string): LiveUsageSummaryPayload => ({
  providers: [],
  errors: [],
  meters: [],
  generatedAt,
})

function setup(visibleInitially = true) {
  let visible: (value: boolean) => void = () => undefined
  let liveChanged: (value: LiveUsageSummaryPayload) => void = () => undefined
  const adapter: MainWindowLimitsAdapter = {
    getLiveUsage: vi.fn().mockResolvedValue(liveUsage("first")),
    getVisible: vi.fn().mockResolvedValue(visibleInitially),
    onVisible: vi.fn(async (handler) => {
      visible = handler
      return vi.fn()
    }),
    onLiveUsageChanged: vi.fn(async (handler) => {
      liveChanged = handler
      return vi.fn()
    }),
  }
  return {
    adapter,
    session: new MainWindowLimitsSession(adapter),
    setVisible: (value: boolean) => visible(value),
    liveChanged: (value: LiveUsageSummaryPayload) => liveChanged(value),
  }
}

describe("MainWindowLimitsSession", () => {
  it("reads the limits for the sidebar without any section being active", async () => {
    // The sidebar shows the limits in every section, so the store answers to
    // the window alone.
    const { adapter, session } = setup()
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().liveUsage?.generatedAt).toBe("first"))
    expect(session.getSnapshot().loading).toBe(false)
    expect(adapter.getLiveUsage).toHaveBeenCalledOnce()
    stop()
  })

  it("stays quiet while the window is hidden and reads again when it returns", async () => {
    const { adapter, session, setVisible, liveChanged } = setup(false)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(adapter.getVisible).toHaveBeenCalled())
    expect(adapter.getLiveUsage).not.toHaveBeenCalled()

    // A push to a hidden window reaches nobody, so it must not be kept.
    liveChanged(liveUsage("hidden"))
    expect(session.getSnapshot().liveUsage).toBeNull()

    setVisible(true)
    await vi.waitFor(() => expect(session.getSnapshot().liveUsage?.generatedAt).toBe("first"))
    liveChanged(liveUsage("pushed"))
    expect(session.getSnapshot().liveUsage?.generatedAt).toBe("pushed")
    stop()
  })

  it("keeps a visibility event that arrives before the first read answers", async () => {
    // The first read asks for the state at start. An event that arrives while
    // it is in flight carries a later state and must win.
    const { adapter, session, setVisible, liveChanged } = setup(false)
    let answer: (value: boolean) => void = () => undefined
    vi.mocked(adapter.getVisible).mockReturnValueOnce(
      new Promise<boolean>((resolve) => {
        answer = resolve
      }),
    )
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(adapter.getVisible).toHaveBeenCalled())

    setVisible(true)
    await vi.waitFor(() => expect(session.getSnapshot().liveUsage?.generatedAt).toBe("first"))
    answer(false)
    // Let the read's continuation run before the next push.
    for (let tick = 0; tick < 5; tick += 1) await Promise.resolve()

    liveChanged(liveUsage("pushed"))
    expect(session.getSnapshot().liveUsage?.generatedAt).toBe("pushed")
    stop()
  })

  it("keeps a pushed snapshot over a read that answers after it", async () => {
    // A read stays in flight while the provider pushes newer figures. The
    // read carries the older state, so it must not land on top of the push.
    const { adapter, session, setVisible, liveChanged } = setup(false)
    let answer: (value: LiveUsageSummaryPayload) => void = () => undefined
    vi.mocked(adapter.getLiveUsage).mockReturnValueOnce(
      new Promise<LiveUsageSummaryPayload>((resolve) => {
        answer = resolve
      }),
    )
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(adapter.getVisible).toHaveBeenCalled())

    setVisible(true)
    await vi.waitFor(() => expect(adapter.getLiveUsage).toHaveBeenCalled())
    liveChanged(liveUsage("pushed"))
    expect(session.getSnapshot().liveUsage?.generatedAt).toBe("pushed")

    answer(liveUsage("stale"))
    for (let tick = 0; tick < 5; tick += 1) await Promise.resolve()
    expect(session.getSnapshot().liveUsage?.generatedAt).toBe("pushed")
    stop()
  })

  it("leaves the sidebar usable when the read fails", async () => {
    const { adapter, session } = setup()
    vi.mocked(adapter.getLiveUsage).mockRejectedValueOnce(new Error("no reading"))
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().loading).toBe(false))
    expect(session.getSnapshot().liveUsage).toBeNull()
    stop()
  })
})
