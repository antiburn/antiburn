import { afterEach, describe, expect, it, vi } from "vitest"

import type { ChecksReportPayload } from "../../lib/insightsIpc"
import type * as IpcModule from "../../lib/ipc"
import { BurnChecksSession, type BurnChecksAdapter } from "./BurnChecksSession"

const noteInteraction = vi.hoisted(() => vi.fn())

vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  noteInteraction,
}))

const report = (burn: number): ChecksReportPayload => ({
  evidenceSettled: true,
  pendingEvidence: 0,
  estimatedTokenBurnBasisPoints: burn,
  categories: [],
})

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

function setup(visibleInitially = true, overrides: Partial<BurnChecksAdapter> = {}) {
  let visible: (value: boolean) => void = () => undefined
  let changed: () => void = () => undefined
  const adapter: BurnChecksAdapter = {
    getReport: vi.fn().mockResolvedValue(report(100)),
    getAggregateWins: vi.fn().mockResolvedValue({ wins: [] }),
    getTargets: vi.fn().mockResolvedValue({ targets: [], truncated: false }),
    cancelReport: vi.fn().mockResolvedValue(undefined),
    getVisible: vi.fn().mockResolvedValue(visibleInitially),
    onVisible: vi.fn(async (handler) => {
      visible = handler
      return vi.fn()
    }),
    onChanged: vi.fn(async (handler) => {
      changed = handler
      return vi.fn()
    }),
    ...overrides,
  }
  const session = new BurnChecksSession(adapter)
  const stop = session.subscribe(() => undefined)
  return {
    adapter,
    session,
    stop,
    setVisible: (value: boolean) => visible(value),
    changed: () => changed(),
  }
}

const sessions: BurnChecksSession[] = []
afterEach(() => sessions.splice(0).forEach((session) => session.dispose()))

describe("BurnChecksSession", () => {
  it("suspends hidden and inactive work and cancels only its own consumer", async () => {
    const { adapter, session, stop, setVisible, changed } = setup(false)
    sessions.push(session)
    await vi.waitFor(() => expect(adapter.getVisible).toHaveBeenCalled())
    expect(adapter.getReport).not.toHaveBeenCalled()
    setVisible(true)
    await vi.waitFor(() => expect(adapter.getReport).toHaveBeenCalledOnce())
    setVisible(false)
    changed()
    expect(adapter.getReport).toHaveBeenCalledOnce()
    stop()
    expect(adapter.cancelReport).toHaveBeenCalledWith(
      expect.stringMatching(/^main-burn-checks-/),
    )
  })

  it("coalesces refreshes, rejects stale hidden results, and reconciles on resume", async () => {
    const pending = deferred<ChecksReportPayload | null>()
    const { adapter, session, setVisible } = setup()
    sessions.push(session)
    await vi.waitFor(() => expect(session.getSnapshot().report).not.toBeNull())
    vi.mocked(adapter.getReport).mockReturnValueOnce(pending.promise)
    session.refresh()
    session.refresh()
    session.refresh()
    expect(adapter.getReport).toHaveBeenCalledTimes(2)
    setVisible(false)
    pending.resolve(report(900))
    await Promise.resolve()
    expect(session.getSnapshot().report?.estimatedTokenBurnBasisPoints).toBe(100)
    setVisible(true)
    await vi.waitFor(() => expect(adapter.getReport).toHaveBeenCalledTimes(3))
    expect(vi.mocked(adapter.getReport).mock.calls[2]?.[0]).not.toBe(
      vi.mocked(adapter.getReport).mock.calls[0]?.[0],
    )
  })

  it("retains report and target data after refresh failures", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    await vi.waitFor(() => expect(session.getSnapshot().report).not.toBeNull())
    session.loadTargets("oldModelUsage")
    await vi.waitFor(() =>
      expect(session.getSnapshot().targets.oldModelUsage?.data).not.toBeNull(),
    )
    vi.mocked(adapter.getReport).mockRejectedValueOnce(new Error("Unavailable"))
    vi.mocked(adapter.getTargets).mockRejectedValueOnce(new Error("Unavailable"))
    session.refresh()
    session.loadTargets("oldModelUsage", true)
    await vi.waitFor(() => expect(session.getSnapshot().error).toBe(true))
    await vi.waitFor(() =>
      expect(session.getSnapshot().targets.oldModelUsage?.error).toBe(true),
    )
    expect(session.getSnapshot().report?.estimatedTokenBurnBasisPoints).toBe(100)
    expect(session.getSnapshot().targets.oldModelUsage?.data).not.toBeNull()
  })

  it("loads targets only after a detector is requested", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    await vi.waitFor(() => expect(session.getSnapshot().report).not.toBeNull())
    expect(adapter.getTargets).not.toHaveBeenCalled()
    session.loadTargets("unusedSkills")
    await vi.waitFor(() => expect(adapter.getTargets).toHaveBeenCalledWith("unusedSkills"))
  })

  it("publishes the report before aggregate savings resolve", async () => {
    const pending = deferred<Awaited<ReturnType<BurnChecksAdapter["getAggregateWins"]>>>()
    const { session } = setup(true, { getAggregateWins: vi.fn(() => pending.promise) })
    sessions.push(session)

    await vi.waitFor(() => expect(session.getSnapshot().report).not.toBeNull())
    expect(session.getSnapshot()).toMatchObject({
      aggregate: null,
      loading: false,
      error: false,
    })

    pending.resolve({ wins: [] })
    await vi.waitFor(() => expect(session.getSnapshot().aggregate).toEqual({ wins: [] }))
  })

  it("keeps the report available when aggregate savings fail", async () => {
    const { session } = setup(true, {
      getAggregateWins: vi.fn().mockRejectedValue(new Error("Unavailable")),
    })
    sessions.push(session)

    await vi.waitFor(() => expect(session.getSnapshot().report).not.toBeNull())
    expect(session.getSnapshot()).toMatchObject({
      aggregate: null,
      loading: false,
      error: false,
    })
  })

  it("does not refresh targets after their check is collapsed", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    await vi.waitFor(() => expect(session.getSnapshot().report).not.toBeNull())
    session.setTargetsVisible("oldModelUsage", true)
    await vi.waitFor(() => expect(adapter.getTargets).toHaveBeenCalledOnce())
    session.setTargetsVisible("oldModelUsage", false)

    session.refresh()
    await vi.waitFor(() => expect(adapter.getReport).toHaveBeenCalledTimes(2))
    expect(adapter.getTargets).toHaveBeenCalledOnce()
  })

  it("does not publish a target result that completes after collapse", async () => {
    const pending = deferred<Awaited<ReturnType<BurnChecksAdapter["getTargets"]>>>()
    const { adapter, session } = setup()
    sessions.push(session)
    await vi.waitFor(() => expect(session.getSnapshot().report).not.toBeNull())
    vi.mocked(adapter.getTargets).mockReturnValueOnce(pending.promise)

    session.setTargetsVisible("oldModelUsage", true)
    await vi.waitFor(() => expect(adapter.getTargets).toHaveBeenCalledOnce())
    session.setTargetsVisible("oldModelUsage", false)
    pending.resolve({ targets: [], truncated: false })
    await pending.promise
    await Promise.resolve()

    expect(session.getSnapshot().targets.oldModelUsage?.data).toBeNull()
    expect(session.getSnapshot().targets.oldModelUsage?.loading).toBe(false)
  })

  it("does not record recurrence from automatic initial expansion", async () => {
    noteInteraction.mockClear()
    const { adapter, session } = setup()
    sessions.push(session)
    vi.mocked(adapter.getTargets).mockResolvedValue({
      targets: [
        {
          findingId: "finding",
          actionId: "action",
          finding: {
            detector: "oldModelUsage",
            agent: "claude-code",
            sourceFormat: "claudeJsonl",
            observation: "A finding recurred.",
            labels: [],
            omitted: 0,
          },
          display: {
            resourceKind: "model",
            resourceIdentity: "old-model",
            currentValue: "old-model",
            replacementValue: "new-model",
            scopeKind: "global",
            quantity: 1,
            quantityUnit: "turns",
            observationCount: 1,
            firstObservedAtMs: 1,
            lastObservedAtMs: 2,
            estimateMethod: "oldModelPriceDifference",
            estimatedOpportunity: null,
            verificationLimit: "freshEvidenceFromSameSourceAndTarget",
          },
          occurrenceCount: 1,
          autoFix: { status: "available" },
          promptFix: { status: "available" },
          watch: {
            watchId: "watch",
            origin: "action",
            lifecycle: "recurred",
            verification: { status: "recurred", methodRevision: 1, evidenceRevision: "e2" },
            savings: { status: "pending" },
          },
          coverageLimits: ["currentPublishedEvidenceOnly"],
          samples: [],
          expiresAtEpoch: 100,
        },
      ],
      truncated: false,
    })
    await vi.waitFor(() => expect(session.getSnapshot().report).not.toBeNull())

    session.setTargetsVisible("oldModelUsage", true, false)
    await vi.waitFor(() => expect(adapter.getTargets).toHaveBeenCalledOnce())
    expect(noteInteraction).not.toHaveBeenCalledWith({
      kind: "burnCheckOutcomeObserved",
      outcome: "recurred",
      origin: "action",
    })

    session.setTargetsVisible("oldModelUsage", false)
    session.setTargetsVisible("oldModelUsage", true)
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "burnCheckOutcomeObserved",
      outcome: "recurred",
      origin: "action",
    })
  })

  it("records only visible Burn Checks outcomes and deduplicates coarse results", async () => {
    noteInteraction.mockClear()
    const { adapter, session, setVisible } = setup(false)
    sessions.push(session)
    vi.mocked(adapter.getAggregateWins).mockResolvedValue({
      wins: [
        {
          findingId: "private-win",
          detector: "oldModelUsage",
          origin: "passive",
          display: {
            resourceKind: "model",
            resourceIdentity: null,
            currentValue: null,
            replacementValue: null,
            scopeKind: "global",
            quantity: 1,
            quantityUnit: "turns",
            observationCount: 1,
            firstObservedAtMs: 1,
            lastObservedAtMs: 2,
            estimateMethod: null,
            estimatedOpportunity: null,
            verificationLimit: "freshEvidenceFromSameSourceAndTarget",
          },
          savings: {
            tokenSavings: null,
            apiEquivalentCostAvoidedUsd: null,
            improvementCount: 1,
            method: null,
          },
          startsAtMs: 1,
          endsAtMs: 2,
        },
      ],
    })
    await vi.waitFor(() => expect(adapter.getVisible).toHaveBeenCalled())
    expect(noteInteraction).not.toHaveBeenCalled()

    setVisible(true)
    await vi.waitFor(() =>
      expect(noteInteraction).toHaveBeenCalledWith({
        kind: "burnCheckOutcomeObserved",
        outcome: "verified",
        origin: "passive",
      }),
    )
    session.refresh()
    await vi.waitFor(() => expect(adapter.getReport).toHaveBeenCalledTimes(2))
    expect(
      noteInteraction.mock.calls.filter(
        ([interaction]) => interaction.kind === "burnCheckOutcomeObserved",
      ),
    ).toHaveLength(1)
    expect(
      noteInteraction.mock.calls.flatMap(([interaction]) => Object.keys(interaction)),
    ).not.toContain("findingId")
  })
})
