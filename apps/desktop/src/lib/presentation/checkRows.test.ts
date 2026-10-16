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

  it("does not turn an unassessed or checking installed row into a durable pass", () => {
    const presentation = checksPresentation({
      ...report,
      categories: [],
      installedChecks: [
        { ...installed, finding: 0, clean: 0, unavailable: 1, checkingCount: 1, targets: [] },
      ],
    })
    expect(presentation.burnChecks.counts).toEqual({ failed: 0, passed: 0, unassessed: 1 })
    expect(checksHeroPresentation(presentation).result).toBe("No checks assessed")
  })
})
