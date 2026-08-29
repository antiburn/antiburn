import { describe, expect, it } from "vitest"

import type { SessionHygienePayload } from "../insightsIpc"
import {
  INITIAL_SESSION_HYGIENE,
  notAssessedReasonLabel,
  sessionHygieneChecks,
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
      { id: "sessionOverdepth", title: "Session overdepth detected" },
      { id: "modelOverthinking", title: "No model overthinking detected" },
      {
        id: "overpoweredSubagents",
        title: "Overpowered subagents not assessed",
      },
      { id: "obsoleteModel", title: "No obsolete model detected" },
      { id: "fastModeOveruse", title: "No fast mode overuse detected" },
      {
        id: "excessCacheRehydration",
        title: "No excess cache rehydration detected",
      },
    ])
  })

  it("names every check without a verdict, whatever the status", () => {
    expect(sessionHygieneChecks(PAYLOAD).map((check) => check.name)).toEqual([
      "Session overdepth",
      "Model overthinking",
      "Overpowered subagents",
      "Obsolete model",
      "Fast-mode overuse",
      "Excess context reprocessing",
    ])
  })

  it("keeps every name free of verdict wording, whatever the status", () => {
    for (const check of sessionHygieneChecks(PAYLOAD)) {
      expect(check.name.toLowerCase()).not.toContain("detected")
      expect(check.name.toLowerCase()).not.toContain("not assessed")
    }
  })

  it("names the not-assessed obsolete-model check without its verdict", () => {
    const notAssessedPayload: SessionHygienePayload = {
      ...PAYLOAD,
      badges: PAYLOAD.badges.map((badge) =>
        badge.id === "obsoleteModel"
          ? { id: "obsoleteModel" as const, status: "notAssessed" as const, notAssessedReason: "incompleteEvidence" as const }
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
