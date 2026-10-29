import { describe, expect, it } from "vitest"
import { installedCheckFixture as installed } from "../../views/main-window/burn-checks/tests/installedCheckFixture"
import { report } from "../../views/main-window/burn-checks/tests/burnChecksTestSupport"
import { activeChecksReport, snoozedDetectorIds } from "../snoozedBurnChecks"
import { checksPresentation, checksHeroPresentation } from "./checkReport"
import { checkRows, nativeCheckIds } from "./checkRows"

describe("check row projection", () => {
  it("keeps installed identity and unpriced results out of native masks and savings", () => {
    const mixed = { ...report, installedChecks: [installed] }
    const rows = checkRows(mixed)
    expect(nativeCheckIds(rows)).toEqual(new Set(report.categories.map((check) => check.id)))
    const presentation = checksPresentation(mixed)
    expect(presentation.failures).toHaveLength(2)
    expect(presentation.estimate.tokenBurnBasisPoints).toBeNull()
    expect(checksHeroPresentation(presentation).result).toBe("2 checks failed")
    expect(presentation.burnChecks.counts.failed).toBe(2)
    const snoozed = new Set([installed.effectiveId])
    const active = activeChecksReport(mixed, snoozed)
    expect(active.categories).toEqual(report.categories)
    expect(active.installedChecks).toEqual([])
    expect(active.estimatedTokenBurnBasisPoints).toBe(report.estimatedTokenBurnBasisPoints)
    expect(checksPresentation(mixed, false, snoozed).failures).toHaveLength(1)
    expect(
      snoozedDetectorIds([{ detector: installed.effectiveId, scope: "check", until: null }]),
    ).toEqual(new Set())
  })

  it.each([
    { hasStarted: false, checkingCount: 0, lifecycle: null, passed: 0 },
    { hasStarted: false, checkingCount: 1, lifecycle: null, passed: 0 },
    { hasStarted: true, checkingCount: 0, lifecycle: null, passed: 0 },
  ])("does not infer an installed result from started or queued work: %j", (facts) => {
    const presentation = checksPresentation({
      ...report,
      categories: [],
      installedChecks: [
        { ...installed, finding: 0, clean: 0, unavailable: 1, ...facts, targets: [] },
      ],
    })
    expect(presentation.burnChecks.counts).toEqual({
      failed: 0,
      passed: facts.passed,
      unassessed: 1 - facts.passed,
    })
    expect(presentation.wins).toHaveLength(facts.passed)
    const row = [...presentation.wins, ...presentation.unavailable][0]!
    expect(row.lifecycle).toBe(facts.lifecycle)
    expect(row.clean).toBe(0)
    expect(checksHeroPresentation(presentation).result).toBe("No checks assessed")
  })

  it.each([
    { finding: 0, clean: 0, hasStarted: false, checkingCount: 0, passed: 0, failed: 0 },
    { finding: 0, clean: 0, hasStarted: true, checkingCount: 1, passed: 0, failed: 0 },
    { finding: 0, clean: 1, hasStarted: false, checkingCount: 0, passed: 1, failed: 0 },
    { finding: 1, clean: 0, hasStarted: false, checkingCount: 0, passed: 0, failed: 1 },
  ])("keeps eligible results separate from a blocked source: %j", (facts) => {
    const presentation = checksPresentation({
      ...report,
      categories: [],
      installedChecks: [
        {
          ...installed,
          ...facts,
          diagnostics: [
            {
              code: "opencode-v2-source-mismatch",
              severity: "error",
              title: "Check the OpenCode source",
              message: "The service database could not be verified.",
              source: null,
              details: [],
              affectedSessionCount: 1,
            },
          ],
        },
      ],
    })
    expect(presentation.wins).toHaveLength(facts.passed)
    expect(presentation.failures).toHaveLength(facts.failed)
    expect(presentation.unavailable).toHaveLength(1 - facts.passed - facts.failed)
  })
})
