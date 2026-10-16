import { describe, expect, it } from "vitest"
import type { InstalledCheckReportPayload } from "../insightsIpc"
import { installedCheckFixture } from "../../views/main-window/burn-checks/tests/installedCheckFixture"
import { checkRows } from "./checkRows"
import { checkResultStatus, checkRowStatus } from "./checkRowStatus"

function status(overrides: Partial<InstalledCheckReportPayload> = {}, snoozed = false) {
  const [check] = checkRows({
    evidenceSettled: true,
    windowSessions: 1,
    pendingEvidence: 0,
    deferredEvidence: 0,
    estimatedTokenBurnBasisPoints: null,
    categories: [],
    installedChecks: [{ ...installedCheckFixture, ...overrides }],
  })
  return checkRowStatus(check!, snoozed)
}

describe("installed check status precedence", () => {
  it("shows a short wait with the actual remaining count only in detail", () => {
    expect(status()).toEqual({
      parts: [{ kind: "waiting", text: "Waiting for activity" }],
      text: "Waiting for activity",
      detail: "Needs 1 more tool result.",
    })
    expect(
      status({ waitingForEvents: { event: "toolResult", count: 3, catchingUp: false } }).detail,
    ).toBe("Needs 3 more tool results.")
  })

  it.each<{
    name: string
    overrides: Partial<InstalledCheckReportPayload>
    text: string
    kind: string
  }>([
    {
      name: "source catch-up",
      overrides: { waitingForEvents: { event: "toolResult", count: 3, catchingUp: true } },
      text: "Catching up",
      kind: "waiting",
    },
    {
      name: "initial source data without a trigger count",
      overrides: { waitingForEvents: null, waitingForDataCount: 1 },
      text: "Catching up",
      kind: "waiting",
    },
    {
      name: "source data before an otherwise ready trigger",
      overrides: { waitingForDataCount: 1 },
      text: "Catching up",
      kind: "waiting",
    },
    {
      name: "queued work before catch-up and older errors",
      overrides: { checkingCount: 2, waitingForDataCount: 1, reviewFailed: 1 },
      text: "Checking",
      kind: "checking",
    },
    {
      name: "execution failure before catch-up",
      overrides: { reviewFailed: 1, waitingForDataCount: 1 },
      text: "Could not check",
      kind: "neutral",
    },
    {
      name: "paused availability before stale queued work",
      overrides: { availability: { status: "paused" }, checkingCount: 1 },
      text: "Paused",
      kind: "neutral",
    },
    {
      name: "invalid definition without a diagnostic",
      overrides: { availability: { status: "invalid" } },
      text: "Check needs fixing",
      kind: "neutral",
    },
    {
      name: "unsupported definition",
      overrides: { availability: { status: "unsupported" } },
      text: "Not supported",
      kind: "neutral",
    },
    {
      name: "unassessed without trigger metadata",
      overrides: { finding: 0, clean: 0, waitingForEvents: null },
      text: "Not assessed",
      kind: "neutral",
    },
    {
      name: "retained failure without trigger metadata",
      overrides: { waitingForEvents: null },
      text: "1 failed",
      kind: "failed",
    },
    {
      name: "retained pass without trigger metadata",
      overrides: { finding: 0, waitingForEvents: null },
      text: "Passed",
      kind: "passed",
    },
  ])("handles $name without claiming a remaining count", ({ overrides, text, kind }) => {
    expect(status(overrides)).toEqual({ parts: [{ kind, text }], text })
  })

  it("shows specific diagnostics before catch-up and snooze before all work", () => {
    const overrides = {
      waitingForDataCount: 1,
      diagnostics: [
        {
          code: "input-too-large",
          severity: "error" as const,
          title: "Input is too large",
          message: "Select less input.",
          source: null,
          details: [],
          affectedSessionCount: 1,
        },
      ],
    }
    expect(status(overrides).text).toBe("Input too large")
    expect(status({ ...overrides, checkingCount: 1 }, true)).toEqual({
      parts: [{ kind: "neutral", text: "Snoozed" }],
      text: "Snoozed",
    })
  })

  it("keeps built-in checking counts and verification wording", () => {
    expect(
      checkResultStatus({ finding: 0, clean: 0, checking: true, checkingCount: 2 }).text,
    ).toBe("2 checking")
    expect(
      checkResultStatus({ finding: 1, clean: 0, lifecycle: "awaitingVerification" }).text,
    ).toBe("1 failed · Verifying")
  })
})
