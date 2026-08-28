import { describe, expect, it } from "vitest"

import type { SessionHygienePayload } from "../insightsIpc"
import {
  INITIAL_SESSION_HYGIENE,
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
      "Session overdepth detected",
      "Model overthinking detected",
      "Overpowered subagents detected",
      "Obsolete model detected",
      "Fast mode overuse detected",
      "Excess cache rehydration detected",
    ])
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

  it("names every non-ready state and leaves ready evidence unlabeled", () => {
    expect(sessionHygieneStateLabel("processing")).toBe("Computing")
    expect(sessionHygieneStateLabel("stale")).toBe("Refreshing")
    expect(sessionHygieneStateLabel("activelyGrowing")).toBe("Still writing")
    expect(sessionHygieneStateLabel("unsupported")).toBe("Unsupported")
    expect(sessionHygieneStateLabel("failed")).toBe("Unavailable")
    expect(sessionHygieneStateLabel("ready")).toBeNull()
  })
})
