import type {
  ChecksCategoryPayload,
  ChecksReportPayload,
  InstalledCheckReportPayload,
  InstalledCheckId,
} from "../insightsIpc"
import { CHECK_LABELS } from "./checkDefinitions"

export interface InstalledCheckRow extends Omit<ChecksCategoryPayload, "id"> {
  id: InstalledCheckId
  installed: InstalledCheckReportPayload
  reviewFailed: number
  reviewedWithoutStatus: number
}

export type CheckRow = ChecksCategoryPayload | InstalledCheckRow

export function isInstalledCheck(check: CheckRow): check is InstalledCheckRow {
  return "installed" in check
}

export function checkLabel(check: CheckRow): string {
  return isInstalledCheck(check) ? check.installed.name : CHECK_LABELS[check.id]
}

export function checkRows(report: ChecksReportPayload): CheckRow[] {
  return [
    ...report.categories,
    ...(report.installedChecks ?? []).map((installed): InstalledCheckRow => ({
      id: installed.effectiveId,
      installed,
      finding: installed.finding,
      clean: installed.clean,
      unavailable: installed.unavailable,
      reviewFailed: installed.reviewFailed,
      reviewedWithoutStatus: installed.reviewedWithoutStatus,
      lifecycle: installed.finding > 0 ? "failing" : installed.clean > 0 ? "passing" : null,
      checking: installed.checkingCount > 0,
      checkingCount: installed.checkingCount,
      agents: installed.agents,
      estimatedTokenBurnBasisPoints: null,
    })),
  ]
}

export function nativeCheckIds(checks: readonly CheckRow[]): Set<ChecksCategoryPayload["id"]> {
  return new Set(checks.filter((check) => !isInstalledCheck(check)).map((check) => check.id))
}
