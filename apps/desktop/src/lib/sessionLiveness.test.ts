import { describe, expect, it } from "vitest"
import type { SweepCountsPayload } from "./sessionIpc"
import type { LiveSessionsSnapshot } from "./sessionLifecycle"
import { isLive, liveModels, liveProviders } from "./sessionLiveness"
import { liveWindowSweeps } from "./presentation/liveUsage"

function count(agent: string, values: Partial<SweepCountsPayload> = {}): SweepCountsPayload {
  return {
    agent,
    working: 0,
    anonymous: 0,
    modelPendingWorking: 0,
    modelFailedWorking: 0,
    modelNoneWorking: 0,
    models: [],
    ...values,
  }
}
function state(sweep: SweepCountsPayload[], total?: number): LiveSessionsSnapshot {
  const working = sweep.reduce((sum, value) => sum + value.working, 0)
  return {
    seq: 0,
    ready: true,
    sessions: new Map(),
    absent: new Set(),
    keylessAgents: new Set(),
    working,
    total: total ?? working,
    anonymous: sweep.reduce((sum, value) => sum + value.anonymous, 0),
    complete: false,
    sweep,
  }
}

describe("canonical sweep selectors", () => {
  it("routes agents independently of bounded snapshot rows", () => {
    const live = state([count("codex", { working: 128 }), count("claude-code", { working: 1 })])
    expect(isLive(live)).toBe(true)
    expect(liveProviders(live)).toEqual(["anthropic", "openai"])
    expect(liveModels(live)).toEqual({})
  })
  it("keeps quiet sessions active but out of sweeps", () => {
    const live = state([], 12)
    expect(isLive(live)).toBe(false)
    expect(liveProviders(live)).toEqual([])
    expect(liveModels(live)).toEqual({})
  })
  it("counts unknown agents globally only", () => {
    const live = state([
      count("cursor", { working: 1, models: [{ model: "claude-fable-5", working: 1 }] }),
    ])
    expect(isLive(live)).toBe(true)
    expect(liveProviders(live)).toEqual([])
    expect(liveModels(live)).toEqual({})
  })
  it("routes Antigravity to Google and canonical removal drops its scope", () => {
    expect(liveProviders(state([count("antigravity", { working: 1 })]))).toEqual(["google"])
    expect(isLive(state([]))).toBe(false)
  })
  it("never applies a local expiry clock", () => {
    const live = state([count("claude-code", { working: 1 })])
    live.sessions = new Map([["old", { agent: "claude-code", lastActivityAt: 1, quiet: true }]])
    expect(isLive(live)).toBe(true)
    expect(liveProviders(live)).toEqual(["anthropic"])
  })
  it("keeps anonymous activity provider-scoped with no model", () => {
    const live = state([
      count("claude-code", { anonymous: 1 }),
      count("codex", { anonymous: 1 }),
    ])
    expect(isLive(live)).toBe(true)
    expect(liveProviders(live)).toEqual(["anthropic", "openai"])
    expect(liveModels(live)).toEqual({})
    expect(liveProviders(state([count("codex", { anonymous: 1 })]))).toEqual(["openai"])
  })
  it("sorts and deduplicates positive models within their provider", () => {
    const live = state([
      count("claude-code", {
        working: 3,
        models: [
          { model: "claude-opus-4-6", working: 1 },
          { model: "claude-fable-5", working: 2 },
        ],
      }),
    ])
    expect(liveModels(live)).toEqual({ anthropic: ["claude-fable-5", "claude-opus-4-6"] })
  })
  it("does not let a model on another provider prove the matching scope", () => {
    const live = state([
      count("claude-code", { anonymous: 1 }),
      count("codex", { working: 1, models: [{ model: "claude-fable-5", working: 1 }] }),
    ])
    expect(
      liveWindowSweeps({ scopeModel: "fable" }, true, liveModels(live).anthropic ?? []),
    ).toBe(false)
  })
  it("keeps positive model evidence despite other pending and failed identities", () => {
    const live = state([
      count("claude-code", {
        working: 4,
        modelPendingWorking: 1,
        modelFailedWorking: 1,
        modelNoneWorking: 1,
        models: [{ model: "claude-fable-5", working: 1 }],
      }),
    ])
    expect(liveModels(live)).toEqual({ anthropic: ["claude-fable-5"] })
  })
  it("does not guess models for pending, failed, or unmodeled publications", () => {
    const live = state([
      count("claude-code", {
        working: 3,
        modelPendingWorking: 1,
        modelFailedWorking: 1,
        modelNoneWorking: 1,
      }),
    ])
    expect(liveProviders(live)).toEqual(["anthropic"])
    expect(liveModels(live)).toEqual({})
  })
  it("drops zero model counts", () => {
    expect(
      liveModels(
        state([count("claude-code", { models: [{ model: "claude-fable-5", working: 0 }] })]),
      ),
    ).toEqual({})
  })
})
