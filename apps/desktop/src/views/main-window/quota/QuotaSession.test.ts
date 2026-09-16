import { afterEach, describe, expect, it, vi } from "vitest"

import type { QuotaAccountPayload, QuotaUsagePayload } from "../../../lib/providerUsageIpc"
import { QuotaSession, type QuotaAdapter } from "./QuotaSession"

const NOW = 1_000_000

function account(over: Partial<QuotaAccountPayload> = {}): QuotaAccountPayload {
  return {
    provider: "anthropic",
    displayName: "Claude",
    accountKey: "acct-1",
    lanes: [
      { lane: "weekly", label: "Weekly", hasFactor: true, currentPeriod: null },
      { lane: "fiveHour", label: "5-hour", hasFactor: true, currentPeriod: null },
    ],
    ...over,
  }
}

function usage(
  generatedAt: string,
  periods: QuotaUsagePayload["periods"] = [],
): QuotaUsagePayload {
  return {
    provider: "anthropic",
    accountKey: "acct-1",
    lane: "weekly",
    laneLabel: "Weekly",
    rangeStartEpoch: NOW - 604800,
    rangeEndEpoch: NOW,
    factor: { usdPerPercent: 1, confidence: "learned" },
    periods,
    generatedAt,
  }
}

function setup(overrides: Partial<QuotaAdapter> = {}) {
  let visible: (value: boolean) => void = () => undefined
  let liveChanged: () => void = () => undefined
  let scanFinished: () => void = () => undefined
  const adapter: QuotaAdapter = {
    getAccounts: vi.fn().mockResolvedValue({ accounts: [account()], generatedAt: "g1" }),
    getUsage: vi.fn().mockResolvedValue(usage("u1")),
    getVisible: vi.fn().mockResolvedValue(true),
    onVisible: vi.fn(async (handler) => {
      visible = handler
      return vi.fn()
    }),
    onLiveUsageChanged: vi.fn(async (handler) => {
      liveChanged = handler
      return vi.fn()
    }),
    onScanFinished: vi.fn(async (handler) => {
      scanFinished = handler
      return vi.fn()
    }),
    now: vi.fn(() => NOW),
    ...overrides,
  }
  const session = new QuotaSession(adapter)
  return {
    adapter,
    session,
    setVisible: (value: boolean) => visible(value),
    liveChanged: () => liveChanged(),
    scanFinished: () => scanFinished(),
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

const sessions: QuotaSession[] = []
afterEach(() => sessions.splice(0).forEach((session) => session.dispose()))

describe("QuotaSession", () => {
  it("on first active subscribe, picks the weekly lane and loads this week's usage", async () => {
    const { session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u1"))
    expect(session.getSnapshot().selection).toEqual({
      provider: "anthropic",
      accountKey: "acct-1",
      lane: "weekly",
    })
    expect(session.getSnapshot().range).toBe("thisWeek")
    stop()
  })

  it("does not load while inactive, only once a viewer subscribes actively", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribeInactive(() => undefined)
    await vi.waitFor(() => expect(adapter.getVisible).toHaveBeenCalled())
    expect(adapter.getAccounts).not.toHaveBeenCalled()
    const stopActive = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(adapter.getAccounts).toHaveBeenCalled())
    stopActive()
    stop()
  })

  it("reloads usage with the same range when the lane changes", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u2"))
    session.selectLane("fiveHour")
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u2"))
    expect(session.getSnapshot().selection?.lane).toBe("fiveHour")
    expect(session.getSnapshot().range).toBe("thisWeek")
    const request = vi.mocked(adapter.getUsage).mock.calls.at(-1)![0]
    expect(request.lane).toBe("fiveHour")
    stop()
  })

  it("keeps a shared lane when the account changes if the new account has it", async () => {
    const secondAccount = account({ provider: "openai", accountKey: "acct-2" })
    const { session } = setup({
      getAccounts: vi
        .fn()
        .mockResolvedValue({ accounts: [account(), secondAccount], generatedAt: "g" }),
    })
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    session.selectLane("fiveHour")
    await vi.waitFor(() => expect(session.getSnapshot().selection?.lane).toBe("fiveHour"))
    session.selectAccount("openai", "acct-2")
    await vi.waitFor(() => expect(session.getSnapshot().selection?.accountKey).toBe("acct-2"))
    expect(session.getSnapshot().selection?.lane).toBe("fiveHour")
    stop()
  })

  it("coalesces a burst of live-usage events into one extra refresh while active", async () => {
    const { adapter, session, liveChanged } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    const pending = deferred<QuotaUsagePayload>()
    vi.mocked(adapter.getUsage)
      .mockReturnValueOnce(pending.promise)
      .mockResolvedValueOnce(usage("u-coalesced"))
    liveChanged()
    liveChanged()
    liveChanged()
    expect(adapter.getUsage).toHaveBeenCalledTimes(2)
    pending.resolve(usage("u-in-flight"))
    // Three triggers while the first extra request was in flight coalesce
    // into exactly one more request, not three.
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(3))
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-coalesced"))
    stop()
  })

  it("refreshes usage after a finished scan while active", async () => {
    const { adapter, session, scanFinished } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-scan"))
    scanFinished()
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-scan"))
    stop()
  })

  it("does not refresh on a live-usage event while inactive", async () => {
    const { adapter, session, setVisible, liveChanged } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    setVisible(false)
    const callsBefore = vi.mocked(adapter.getUsage).mock.calls.length
    liveChanged()
    await Promise.resolve()
    expect(vi.mocked(adapter.getUsage).mock.calls.length).toBe(callsBefore)
    stop()
  })

  it("sets accountsError on a failed accounts load", async () => {
    const { session } = setup({ getAccounts: vi.fn().mockRejectedValue(new Error("no")) })
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().accountsError).toBe(true))
    stop()
  })

  it("open sets a custom range and the selected account and lane", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-opened"))
    session.open(
      { provider: "anthropic", accountKey: "acct-1", lane: "fiveHour" },
      { startEpoch: 1000, endEpoch: 2000 },
    )
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-opened"))
    expect(session.getSnapshot().selection).toEqual({
      provider: "anthropic",
      accountKey: "acct-1",
      lane: "fiveHour",
    })
    expect(session.getSnapshot().range).toEqual({
      kind: "custom",
      startEpoch: 1000,
      endEpoch: 2000,
    })
    const request = vi.mocked(adapter.getUsage).mock.calls.at(-1)![0]
    expect(request.rangeStartEpoch).toBe(1000)
    expect(request.rangeEndEpoch).toBe(2000)
    stop()
  })

  it("loads the custom range from an inactive open once the session becomes active", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribeInactive(() => undefined)
    await vi.waitFor(() => expect(adapter.getVisible).toHaveBeenCalled())
    expect(adapter.getAccounts).not.toHaveBeenCalled()

    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-opened"))
    session.open(
      { provider: "anthropic", accountKey: "acct-1", lane: "fiveHour" },
      { startEpoch: 1000, endEpoch: 2000 },
    )
    // Nothing loads while inactive: `open` only records the selection and
    // range, and queues the usage load for later.
    expect(adapter.getUsage).not.toHaveBeenCalled()
    expect(session.getSnapshot().range).toEqual({
      kind: "custom",
      startEpoch: 1000,
      endEpoch: 2000,
    })

    const stopActive = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-opened"))
    const request = vi.mocked(adapter.getUsage).mock.calls.at(-1)![0]
    expect(request).toMatchObject({
      provider: "anthropic",
      accountKey: "acct-1",
      lane: "fiveHour",
      rangeStartEpoch: 1000,
      rangeEndEpoch: 2000,
    })
    stopActive()
    stop()
  })

  it("keeps the last good usage and flags usageError on a failed usage load", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    vi.mocked(adapter.getUsage).mockRejectedValueOnce(new Error("no"))
    session.selectRange("last30Days")
    await vi.waitFor(() => expect(session.getSnapshot().usageError).toBe(true))
    expect(session.getSnapshot().usage?.generatedAt).toBe("u1")
    stop()
  })
})
