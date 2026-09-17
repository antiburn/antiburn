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
function model(model: string, providerRoute: string | null, working = 1) {
  return { model, providerRoute, recordedProvider: providerRoute, modelVendor: null, working }
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
  it("uses exact routes beyond bounded snapshot rows, independently of harness", () => {
    const live = state([
      count("pi", { working: 128, models: [model("gpt-6-astra", "openai", 128)] }),
      count("pi", { working: 1, models: [model("claude-fable-5", "anthropic")] }),
    ])
    expect(isLive(live)).toBe(true)
    expect(liveProviders(live)).toEqual(["anthropic", "openai"])
    expect(liveModels(live)).toEqual({ anthropic: ["claude-fable-5"], openai: ["gpt-6-astra"] })
  })
  it("keeps quiet sessions active but out of sweeps", () => {
    const live = state([], 12)
    expect(isLive(live)).toBe(false)
    expect(liveProviders(live)).toEqual([])
    expect(liveModels(live)).toEqual({})
  })
  it.each(["openrouter", "aws", "azure", "google-vertex", "cursor", null])(
    "does not turn route %s into direct Anthropic limits",
    (route) => {
      const live = state([
        count("claude-code", {
          working: 1,
          models: [{ ...model("claude-fable-5", route), modelVendor: "anthropic" }],
        }),
      ])
      expect(isLive(live)).toBe(true)
      expect(liveProviders(live)).toEqual([])
      expect(liveModels(live)).toEqual({})
      expect(
        liveWindowSweeps({ scopeModel: "fable" }, false, liveModels(live).anthropic ?? []),
      ).toBe(false)
    },
  )
  it("uses Google route evidence without inferring it from Antigravity", () => {
    expect(liveProviders(state([count("antigravity", { working: 1 })]))).toEqual([])
    expect(
      liveProviders(
        state([
          count("pi", {
            working: 1,
            models: [model("gemini-pro", "google")],
          }),
        ]),
      ),
    ).toEqual(["google"])
  })
  it("never applies a local expiry clock", () => {
    const live = state([count("pi", { working: 1, models: [model("gpt-6-astra", "openai")] })])
    live.sessions = new Map([["old", { agent: "pi", lastActivityAt: 1, quiet: true }]])
    expect(isLive(live)).toBe(true)
    expect(liveProviders(live)).toEqual(["openai"])
  })
  it("keeps anonymous harness activity global only", () => {
    const live = state([
      count("claude-code", { anonymous: 1 }),
      count("codex", { anonymous: 1 }),
    ])
    expect(isLive(live)).toBe(true)
    expect(liveProviders(live)).toEqual([])
    expect(liveModels(live)).toEqual({})
  })
  it("sorts and deduplicates positive models within their recorded route", () => {
    const live = state([
      count("pi", {
        working: 4,
        models: [
          model("claude-opus-4-6", "anthropic"),
          model("claude-fable-5", "anthropic", 2),
          model("claude-fable-5", "openrouter"),
        ],
      }),
    ])
    expect(liveModels(live)).toEqual({ anthropic: ["claude-fable-5", "claude-opus-4-6"] })
  })
  it("keeps positive evidence despite other pending and failed identities", () => {
    const live = state([
      count("pi", {
        working: 4,
        modelPendingWorking: 1,
        modelFailedWorking: 1,
        modelNoneWorking: 1,
        models: [model("claude-fable-5", "anthropic")],
      }),
    ])
    expect(liveModels(live)).toEqual({ anthropic: ["claude-fable-5"] })
  })
  it("does not guess routes for pending, failed, or unmodeled publications", () => {
    const live = state([
      count("claude-code", {
        working: 3,
        modelPendingWorking: 1,
        modelFailedWorking: 1,
        modelNoneWorking: 1,
      }),
    ])
    expect(isLive(live)).toBe(true)
    expect(liveProviders(live)).toEqual([])
    expect(liveModels(live)).toEqual({})
  })
  it("drops zero model counts", () => {
    const live = state([count("pi", { models: [model("claude-fable-5", "anthropic", 0)] })])
    expect(liveProviders(live)).toEqual([])
    expect(liveModels(live)).toEqual({})
  })
})
