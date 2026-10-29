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
  it("keeps event waits out of the shared result label", () => {
    expect(status().text).toBe("1 failed · 1 passed")
    expect(
      status({ waitingForEvents: { event: "toolResult", count: 3, catchingUp: false } }).text,
    ).toBe("1 failed · 1 passed")
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
      text: "1 failed · 1 passed",
      kind: "passed",
    },
    {
      name: "initial source data without a trigger count",
      overrides: { waitingForEvents: null, waitingForDataCount: 1 },
      text: "1 failed · 1 passed",
      kind: "passed",
    },
    {
      name: "source data before an otherwise ready trigger",
      overrides: { waitingForDataCount: 1 },
      text: "1 failed · 1 passed",
      kind: "passed",
    },
    {
      name: "queued work before catch-up and older errors",
      overrides: { checkingCount: 2, waitingForDataCount: 1, reviewFailed: 1 },
      text: "1 failed · 1 passed · Checking",
      kind: "checking",
    },
    {
      name: "execution failure before catch-up",
      overrides: { reviewFailed: 1, waitingForDataCount: 1 },
      text: "1 failed · 1 passed",
      kind: "passed",
    },
    {
      name: "paused availability before stale queued work",
      overrides: { availability: { status: "paused" }, checkingCount: 1 },
      text: "1 failed · 1 passed",
      kind: "passed",
    },
    {
      name: "invalid definition without a diagnostic",
      overrides: { availability: { status: "invalid" } },
      text: "1 failed · 1 passed",
      kind: "passed",
    },
    {
      name: "unsupported definition",
      overrides: { availability: { status: "unsupported" } },
      text: "1 failed · 1 passed",
      kind: "passed",
    },
    {
      name: "unassessed without trigger metadata",
      overrides: { finding: 0, clean: 0, hasStarted: false, waitingForEvents: null },
      text: "Not assessed",
      kind: "neutral",
    },
    {
      name: "retained failure without trigger metadata",
      overrides: { clean: 0, waitingForEvents: null },
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
    expect(status(overrides).text).toBe(text)
    expect(status(overrides).parts.at(-1)?.kind).toBe(kind)
  })

  it.each(["paused", "unsupported"] as const)(
    "keeps accepted counts before %s diagnostics without queued or waiting progress",
    (availability) => {
      for (const [finding, clean, result] of [
        [1, 1, "1 failed · 1 passed"],
        [1, 0, "1 failed"],
        [0, 2, "2 passed"],
      ] as const) {
        const current = status({
          availability: { status: availability },
          finding,
          clean,
          checkingCount: 2,
          waitingForDataCount: 3,
          waitingForEvents: { event: "toolResult", count: 3, catchingUp: true },
          diagnostics: [
            {
              code: "provider-authentication",
              severity: "error",
              title: "Connection needed",
              message: "Update the active connection in Settings.",
              source: null,
              details: [],
              affectedSessionCount: 1,
            },
          ],
        })
        expect(current.text).toBe(result)
        expect(current.parts.some(({ kind }) => kind === "checking")).toBe(false)
      }
    },
  )

  it.each(["paused", "unsupported", "invalid"] as const)(
    "does not claim a pass from old activity while %s without accepted results",
    (availability) => {
      expect(
        status({
          availability: { status: availability },
          finding: 0,
          clean: 0,
          hasStarted: true,
          checkingCount: 2,
        }).text,
      ).toBe("Not assessed")
    },
  )

  it.each([
    { hasStarted: false, checkingCount: 1, clean: 0, text: "Not assessed · Checking" },
    { hasStarted: true, checkingCount: 0, clean: 0, text: "Not assessed" },
    { hasStarted: true, checkingCount: 1, clean: 2, text: "2 passed · Checking" },
  ])("uses saved results only: $text", ({ text, ...overrides }) => {
    expect(status({ finding: 0, waitingForEvents: null, ...overrides }).text).toBe(text)
  })

  it("keeps unstarted checks not assessed while waiting", () => {
    expect(status({ hasStarted: false, finding: 0, clean: 0 }).text).toBe("Not assessed")
  })

  it("keeps diagnostics out of result text and snooze before all work", () => {
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
    expect(status(overrides).text).toBe("1 failed · 1 passed")
    expect(status({ ...overrides, checkingCount: 1 }, true)).toEqual({
      parts: [{ kind: "neutral", text: "Snoozed" }],
      text: "Snoozed",
    })
  })

  it.each([
    { finding: 1, clean: 0, result: "1 failed" },
    { finding: 0, clean: 2, result: "2 passed" },
  ])("keeps $result after a failed recheck", ({ result, ...counts }) => {
    expect(status({ ...counts, reviewFailed: 1, waitingForEvents: null }).text).toBe(result)
  })

  it.each([
    { finding: 0, clean: 0, result: "Not assessed" },
    { finding: 1, clean: 0, result: "1 failed" },
    { finding: 0, clean: 1, result: "Passed" },
  ])("keeps $result during unavailable source work", ({ result, ...counts }) => {
    expect(
      status({
        ...counts,
        hasStarted: true,
        checkingCount: 1,
        diagnostics: [
          {
            code: "opencode-v2-capture-gap",
            severity: "error",
            title: "OpenCode live checks are not running",
            message:
              "Live activity is not available for this session. Keep OpenCode open, then rescan sessions. Live checks cannot recover missed activity. Saved results cover only their recorded input.",
            source: null,
            details: [],
            affectedSessionCount: 1,
          },
        ],
      }).text,
    ).toBe(`${result} · Checking`)
  })

  it("keeps deterministic verification wording", () => {
    expect(
      checkResultStatus({ finding: 1, clean: 0, lifecycle: "awaitingVerification" }).text,
    ).toBe("1 failed · Verifying")
  })
})
