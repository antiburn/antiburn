import { describe, expect, it } from "vitest"

import type { SessionHygienePayload } from "../insightsIpc"
import {
  INITIAL_SESSION_HYGIENE,
  notAssessedReasonLabel,
  sessionHygieneChecks,
  sessionHygieneDocumentation,
  sessionHygieneStateLabel,
} from "./sessionHygiene"

const PAYLOAD: SessionHygienePayload = {
  badges: [
    { id: "sessionOverdepth", status: "finding", notAssessedReason: null },
    { id: "modelOverthinking", status: "clean", notAssessedReason: null },
    {
      id: "overpoweredSubagents",
      status: "notAssessed",
      notAssessedReason: "incompleteEvidence",
    },
    { id: "obsoleteModel", status: "clean", notAssessedReason: null },
    { id: "fastModeOveruse", status: "clean", notAssessedReason: null },
    { id: "excessCacheRehydration", status: "clean", notAssessedReason: null },
  ],
  evidenceState: "ready",
}

describe("sessionHygieneChecks", () => {
  it("maps every engine identifier to reader copy in a stable order", () => {
    expect(sessionHygieneChecks(PAYLOAD).map(({ id, title }) => ({ id, title }))).toEqual([
      { id: "sessionOverdepth", title: "Session went too deep" },
      { id: "modelOverthinking", title: "Thinking/reasoning modes ok" },
      {
        id: "overpoweredSubagents",
        title: "Subagent models not assessed",
      },
      { id: "obsoleteModel", title: "All models up to date" },
      { id: "fastModeOveruse", title: "Fast mode not overused" },
      {
        id: "excessCacheRehydration",
        title: "Cache rehydration under control",
      },
    ])
  })

  it("names every check without a verdict, whatever the status", () => {
    expect(sessionHygieneChecks(PAYLOAD).map((check) => check.name)).toEqual([
      "Session overdepth",
      "Model overthinking",
      "Overpowered subagents",
      "Obsolete model",
      "Fast mode overuse",
      "Excess cache rehydration",
    ])
  })

  it("keeps every name free of verdict wording, whatever the status", () => {
    for (const check of sessionHygieneChecks(PAYLOAD)) {
      expect(check.name.toLowerCase()).not.toContain("detected")
      expect(check.name.toLowerCase()).not.toContain("not assessed")
    }
  })

  it("provides an explanation and guidance for every check", () => {
    for (const check of sessionHygieneChecks(PAYLOAD)) {
      const documentation = sessionHygieneDocumentation(check)
      expect(documentation.summary.length).toBeGreaterThan(0)
      expect(documentation.guidance.length).toBeGreaterThan(0)
    }
  })

  it("describes the stored evidence that caused a finding", () => {
    const payload: SessionHygienePayload = {
      ...PAYLOAD,
      badges: PAYLOAD.badges.map((badge) =>
        badge.id === "modelOverthinking"
          ? {
              ...badge,
              status: "finding" as const,
              findingEvidence: {
                kind: "modelOverthinking" as const,
                tiers: [{ tier: "max", mainLoopTurns: 2, delegatedTurns: 1 }],
              },
            }
          : badge,
      ),
    }
    const check = sessionHygieneChecks(payload).find(
      (candidate) => candidate.id === "modelOverthinking",
    )!

    expect(sessionHygieneDocumentation(check).findingDetails).toEqual([
      "The session used max reasoning for 3 turns.",
    ])
  })

  it("uses concise model names in finding details", () => {
    const payload: SessionHygienePayload = {
      ...PAYLOAD,
      badges: PAYLOAD.badges.map((badge) =>
        badge.id === "overpoweredSubagents"
          ? {
              ...badge,
              status: "finding" as const,
              findingEvidence: {
                kind: "overpoweredSubagents" as const,
                mainModels: ["claude-fable-5"],
                delegatedModels: ["claude-fable-5", "claude-opus-5"],
              },
            }
          : badge,
      ),
    }
    const check = sessionHygieneChecks(payload).find(
      (candidate) => candidate.id === "overpoweredSubagents",
    )!

    expect(sessionHygieneDocumentation(check).findingDetails).toEqual([
      "fable-5 used premium subagents (fable-5 and opus-5).",
    ])
  })

  it("names the not-assessed obsolete-model check without its verdict", () => {
    const notAssessedPayload: SessionHygienePayload = {
      ...PAYLOAD,
      badges: PAYLOAD.badges.map((badge) =>
        badge.id === "obsoleteModel"
          ? {
              id: "obsoleteModel" as const,
              status: "notAssessed" as const,
              notAssessedReason: "incompleteEvidence" as const,
            }
          : badge,
      ),
    }
    const obsoleteModel = sessionHygieneChecks(notAssessedPayload).find(
      (check) => check.id === "obsoleteModel",
    )
    expect(obsoleteModel?.name).toBe("Obsolete model")
  })

  it("maps finding, clean, and not-assessed states to semantic ink", () => {
    expect(sessionHygieneChecks(PAYLOAD).map((check) => check.ink)).toEqual([
      "system-red-text",
      "system-green",
      "label-tertiary",
      "system-green",
      "system-green",
      "system-green",
    ])
  })

  it("names the accounting-specific detail copy for a cache-write finding", () => {
    const cacheWritePayload: SessionHygienePayload = {
      ...PAYLOAD,
      badges: PAYLOAD.badges.map((badge) =>
        badge.id === "excessCacheRehydration"
          ? {
              id: "excessCacheRehydration" as const,
              status: "finding" as const,
              notAssessedReason: null,
              accounting: "cacheWrite" as const,
            }
          : badge,
      ),
    }
    const check = sessionHygieneChecks(cacheWritePayload).find(
      (candidate) => candidate.id === "excessCacheRehydration",
    )
    expect(check?.detail).toBe("Reduce repeated cache writes")
    expect(check && sessionHygieneDocumentation(check).guidance[0]).toBe(
      "Reduce repeated cache writes",
    )
  })

  it("names the accounting-specific detail copy for an uncached-input finding", () => {
    const uncachedInputPayload: SessionHygienePayload = {
      ...PAYLOAD,
      badges: PAYLOAD.badges.map((badge) =>
        badge.id === "excessCacheRehydration"
          ? {
              id: "excessCacheRehydration" as const,
              status: "finding" as const,
              notAssessedReason: null,
              accounting: "uncachedInput" as const,
            }
          : badge,
      ),
    }
    const check = sessionHygieneChecks(uncachedInputPayload).find(
      (candidate) => candidate.id === "excessCacheRehydration",
    )
    expect(check?.detail).toBe("Reduce full-price context re-reads")
  })

  it("leaves detail null for a finding with no accounting, and for every non-finding check", () => {
    const uncachedInputPayload: SessionHygienePayload = {
      ...PAYLOAD,
      badges: PAYLOAD.badges.map((badge) =>
        badge.id === "excessCacheRehydration"
          ? {
              id: "excessCacheRehydration" as const,
              status: "finding" as const,
              notAssessedReason: null,
            }
          : badge,
      ),
    }
    for (const check of sessionHygieneChecks(uncachedInputPayload)) {
      expect(check.detail).toBeNull()
    }
  })

  it("starts every badge as not assessed while evidence is pending", () => {
    const checks = sessionHygieneChecks(INITIAL_SESSION_HYGIENE)
    expect(checks).toHaveLength(6)
    expect(checks.every((check) => check.status === "notAssessed")).toBe(true)
    expect(sessionHygieneStateLabel(INITIAL_SESSION_HYGIENE.evidenceState)).toBe("Computing")
  })

  it("names the signal-missing reason without claiming a clean result", () => {
    expect(notAssessedReasonLabel("signalMissing")).toBe(
      "this session did not record the setting this check needs",
    )
  })

  it("names every non-ready state and leaves ready evidence unlabeled", () => {
    expect(sessionHygieneStateLabel("processing")).toBe("Computing")
    expect(sessionHygieneStateLabel("stale")).toBe("Refreshing")
    expect(sessionHygieneStateLabel("activelyGrowing")).toBe("Still writing")
    expect(sessionHygieneStateLabel("unsupported")).toBe("Unsupported")
    expect(sessionHygieneStateLabel("failed")).toBe("Unavailable")
    expect(sessionHygieneStateLabel("ready")).toBeNull()
  })
})
