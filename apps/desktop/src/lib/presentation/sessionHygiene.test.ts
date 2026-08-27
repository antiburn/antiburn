import { describe, expect, it } from "vitest"

import type { SessionHygienePayload } from "../insightsIpc"
import {
  INITIAL_SESSION_HYGIENE,
  sessionHygieneChecks,
  sessionHygieneStateLabel,
} from "./sessionHygiene"

const PAYLOAD: SessionHygienePayload = {
  badges: [
    { id: "reasoningOverkill", status: "finding", notAssessedReason: null },
    { id: "excessCacheRehydration", status: "clean", notAssessedReason: null },
    {
      id: "bloatedInitialContext",
      status: "notAssessed",
      notAssessedReason: "incompleteEvidence",
    },
  ],
  evidenceState: "ready",
}

describe("sessionHygieneChecks", () => {
  it("maps every engine identifier to reader copy in a stable order", () => {
    expect(sessionHygieneChecks(PAYLOAD).map(({ id, title }) => ({ id, title }))).toEqual([
      { id: "reasoningOverkill", title: "Reasoning overkill detected" },
      {
        id: "excessCacheRehydration",
        title: "No excess cache rehydration detected",
      },
      {
        id: "bloatedInitialContext",
        title: "Bloated initial context not assessed",
      },
    ])
  })

  it("maps finding, clean, and not-assessed states to semantic ink", () => {
    expect(sessionHygieneChecks(PAYLOAD).map((check) => check.ink)).toEqual([
      "system-red-text",
      "system-green",
      "label-tertiary",
    ])
  })

  it("starts every badge as not assessed while evidence is pending", () => {
    const checks = sessionHygieneChecks(INITIAL_SESSION_HYGIENE)
    expect(checks).toHaveLength(3)
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
