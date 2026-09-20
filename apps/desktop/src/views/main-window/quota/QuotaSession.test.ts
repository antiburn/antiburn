import { afterEach, describe, expect, it, vi } from "vitest"

import type { QuotaAccountPayload, QuotaUsagePayload } from "../../../lib/providerUsageIpc"
import { QUOTA_USAGE_CACHE_TTL_MS, QuotaSession, type QuotaAdapter } from "./QuotaSession"
import { rangeForPreset } from "./quotaSeries"
import { readQuotaViewPrefs, writeQuotaViewPrefs } from "./quotaViewPrefs"

const NOW = 1_000_000
const WEEK = 604800

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
  const adapter: QuotaAdapter = {
    getAccounts: vi.fn().mockResolvedValue({ accounts: [account()], generatedAt: "g1" }),
    getUsage: vi.fn().mockResolvedValue(usage("u1")),
    getVisible: vi.fn().mockResolvedValue(true),
    onVisible: vi.fn(async (handler) => {
      visible = handler
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
afterEach(() => {
  sessions.splice(0).forEach((session) => session.dispose())
  localStorage.clear()
})

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

  it("does not live-update: a fake adapter with no push channels never reloads on its own", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    const callsBefore = vi.mocked(adapter.getUsage).mock.calls.length
    // Nothing pushes a refresh: the adapter exposes no live-usage or
    // scan-finished channel, so only a selection change loads usage again.
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(vi.mocked(adapter.getUsage).mock.calls.length).toBe(callsBefore)
    session.selectLane("fiveHour")
    await vi.waitFor(() =>
      expect(vi.mocked(adapter.getUsage).mock.calls.length).toBe(callsBefore + 1),
    )
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

  it("reloads with the range a window preset resolves to", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-window"))
    session.selectRange("last3Windows")
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-window"))
    expect(session.getSnapshot().range).toBe("last3Windows")
    const request = vi.mocked(adapter.getUsage).mock.calls.at(-1)![0]
    // The weekly lane carries no current period in this fixture, so the
    // preset falls back to a trailing three weeks ending now — exactly what
    // `rangeForPreset` itself computes for the same inputs.
    const expected = rangeForPreset(
      "last3Windows",
      { lane: "weekly", label: "Weekly", hasFactor: true, currentPeriod: null },
      NOW,
    )
    expect(request.rangeStartEpoch).toBe(expected.startEpoch)
    expect(request.rangeEndEpoch).toBe(expected.endEpoch)
    expect(request.rangeStartEpoch).toBe(NOW - 3 * WEEK)
    expect(request.rangeEndEpoch).toBe(NOW)
    stop()
  })

  it("marks loading true for a reload beside existing usage, then false once it settles", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    expect(session.getSnapshot().loading).toBe(false)

    const pending = deferred<QuotaUsagePayload>()
    vi.mocked(adapter.getUsage).mockReturnValueOnce(pending.promise)
    session.selectLane("fiveHour")
    expect(session.getSnapshot().loading).toBe(true)

    pending.resolve(usage("u-reload"))
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-reload"))
    expect(session.getSnapshot().loading).toBe(false)
    stop()
  })

  it("reuses a cached reading when the reader returns to a previously loaded range, without a loading flash", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    const callsBefore = vi.mocked(adapter.getUsage).mock.calls.length

    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-A"))
    session.selectRange("lastWeek")
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-A"))

    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-B"))
    session.selectRange("last30Days")
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-B"))
    expect(vi.mocked(adapter.getUsage).mock.calls.length).toBe(callsBefore + 2)

    const loadingSnapshots: boolean[] = []
    const stopRecorder = session.subscribe(() =>
      loadingSnapshots.push(session.getSnapshot().loading),
    )
    session.selectRange("lastWeek")
    stopRecorder()

    expect(session.getSnapshot().usage?.generatedAt).toBe("u-A")
    // The third selection is a cache hit: no new call, and `loading` never
    // reported true, so a reader flipping back sees no dimmed flash.
    expect(vi.mocked(adapter.getUsage).mock.calls.length).toBe(callsBefore + 2)
    expect(loadingSnapshots).not.toContain(true)
    stop()
  })

  it("restores the now captured with the cached payload on a cache hit", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())

    vi.mocked(adapter.now).mockReturnValueOnce(NOW + 500)
    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-A"))
    session.selectRange("lastWeek")
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-A"))
    expect(session.getSnapshot().now).toBe(NOW + 500)

    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-B"))
    session.selectRange("last30Days")
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-B"))

    session.selectRange("lastWeek")
    expect(session.getSnapshot().usage?.generatedAt).toBe("u-A")
    expect(session.getSnapshot().now).toBe(NOW + 500)
    stop()
  })

  it("queries again once a cached reading passes its TTL", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())

    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-A"))
    session.selectRange("lastWeek")
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-A"))

    session.selectRange("last30Days")
    await vi.waitFor(() => expect(session.getSnapshot().range).toBe("last30Days"))

    vi.mocked(adapter.now).mockReturnValue(NOW + QUOTA_USAGE_CACHE_TTL_MS / 1000 + 1)
    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-A2"))
    session.selectRange("lastWeek")
    // A fresh call, not the stale cache entry, is the only way this resolves.
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-A2"))
    stop()
  })

  it("refresh queries again even when the cached reading is still inside its TTL", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    const callsBefore = vi.mocked(adapter.getUsage).mock.calls.length

    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-refreshed"))
    session.refresh()
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-refreshed"))
    expect(vi.mocked(adapter.getUsage).mock.calls.length).toBe(callsBefore + 1)
    stop()
  })

  it("caches a custom range from open by its epochs, and misses for a different custom range", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())

    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-custom1"))
    session.open(
      { provider: "anthropic", accountKey: "acct-1", lane: "fiveHour" },
      { startEpoch: 1000, endEpoch: 2000 },
    )
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-custom1"))
    const callsAfterFirstOpen = vi.mocked(adapter.getUsage).mock.calls.length

    // The same selection and the same custom epochs: a cache hit, no new call.
    session.open(
      { provider: "anthropic", accountKey: "acct-1", lane: "fiveHour" },
      { startEpoch: 1000, endEpoch: 2000 },
    )
    expect(session.getSnapshot().usage?.generatedAt).toBe("u-custom1")
    expect(vi.mocked(adapter.getUsage).mock.calls.length).toBe(callsAfterFirstOpen)

    // A different custom range misses.
    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage("u-custom2"))
    session.open(
      { provider: "anthropic", accountKey: "acct-1", lane: "fiveHour" },
      { startEpoch: 3000, endEpoch: 4000 },
    )
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("u-custom2"))
    stop()
  })

  it("restores the saved account, lane, and range preset when they exist in the accounts payload", async () => {
    writeQuotaViewPrefs({
      provider: "anthropic",
      accountKey: "acct-1",
      lane: "fiveHour",
      rangePreset: "last30Days",
    })
    const { session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    expect(session.getSnapshot().selection).toEqual({
      provider: "anthropic",
      accountKey: "acct-1",
      lane: "fiveHour",
    })
    expect(session.getSnapshot().range).toBe("last30Days")
    stop()
  })

  it("ignores a saved account, lane, or preset absent from the accounts payload, keeping today's default", async () => {
    writeQuotaViewPrefs({
      provider: "openai",
      accountKey: "does-not-exist",
      lane: "weekly",
      rangePreset: "notAPreset" as never,
    })
    const { session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    expect(session.getSnapshot().selection).toEqual({
      provider: "anthropic",
      accountKey: "acct-1",
      lane: "weekly",
    })
    expect(session.getSnapshot().range).toBe("thisWeek")
    stop()
  })

  it("writes the account, lane, and range as the reader changes them", async () => {
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
    expect(readQuotaViewPrefs()).toMatchObject({
      provider: "anthropic",
      accountKey: "acct-1",
      lane: "fiveHour",
    })

    session.selectRange("last30Days")
    await vi.waitFor(() => expect(session.getSnapshot().range).toBe("last30Days"))
    expect(readQuotaViewPrefs().rangePreset).toBe("last30Days")

    session.selectAccount("openai", "acct-2")
    await vi.waitFor(() => expect(session.getSnapshot().selection?.accountKey).toBe("acct-2"))
    expect(readQuotaViewPrefs()).toMatchObject({ provider: "openai", accountKey: "acct-2" })
    stop()
  })

  it("does not persist a custom range from open()", async () => {
    const { session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())

    session.open(
      { provider: "anthropic", accountKey: "acct-1", lane: "fiveHour" },
      { startEpoch: 1000, endEpoch: 2000 },
    )
    await vi.waitFor(() =>
      expect(session.getSnapshot().range).toEqual({
        kind: "custom",
        startEpoch: 1000,
        endEpoch: 2000,
      }),
    )
    expect(readQuotaViewPrefs().rangePreset).toBeUndefined()
    stop()
  })
})
