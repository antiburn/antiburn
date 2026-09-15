import { CollectionToolbar } from "../../../components/ui/CollectionToolbar"
import { Info } from "lucide-react"
import { BURN_CHECK_MARKS } from "../../../components/burn-checks/burnCheckMarks"
import { InfoPopover } from "../../../components/presentation/InfoPopover"

import type { ChecksReportPayload } from "../../../lib/insightsIpc"
import { openSettingsWindow } from "../../../lib/ipc"
import { isMacOS } from "../../../lib/platform"
import { checksPresentation, formatTokenBurnPercent } from "../../../lib/presentation/checks"

export function BurnChecksHeader({ report }: { report?: ChecksReportPayload }) {
  const failures = report ? checksPresentation(report).failures.length : 0
  const FailureIcon = BURN_CHECK_MARKS.finding.Icon
  const assessment = !report
    ? null
    : report.pendingEvidence > 0
      ? `${report.pendingEvidence} session${report.pendingEvidence === 1 ? "" : "s"} processing.`
      : report.evidenceSettled
        ? "Assessment complete for available evidence."
        : "Assessment is updating."
  return (
    <header
      className="burn-checks-collection-header"
      data-tauri-drag-region={isMacOS() ? "deep" : undefined}
    >
      <CollectionToolbar className="burn-checks-collection-toolbar" dragRegion={isMacOS()}>
        {failures > 0 && (
          <h2
            id="burn-checks-failed"
            className="mr-auto flex items-center gap-2 type-caption font-medium! text-label-tertiary"
          >
            <FailureIcon
              size={14}
              strokeWidth={BURN_CHECK_MARKS.finding.strokeWidth}
              className={BURN_CHECK_MARKS.finding.iconClass}
              aria-hidden="true"
            />
            <span>Failed checks</span>{" "}
            <span className="burn-check-group-count tabular-nums">{failures}</span>
          </h2>
        )}
        <p className="ml-auto type-caption text-label-tertiary">30 days</p>
        {report && (
          <InfoPopover label="Assessment details" icon={<Info size={15} aria-hidden="true" />}>
            {(close) => (
              <>
                <h3 className="type-headline text-label">Assessment details</h3>
                <p className="mt-2 type-callout text-label-secondary">{assessment}</p>
                <p className="mt-1 type-footnote text-label-tertiary">30 days</p>
                <div className="mt-3 border-t border-separator pt-3">
                  <p className="type-title-2 tabular-nums text-label">
                    {report.estimatedTokenBurnBasisPoints == null
                      ? "Unavailable"
                      : formatTokenBurnPercent(report.estimatedTokenBurnBasisPoints)}
                  </p>
                  <p className="mt-1 type-callout text-label-secondary">Estimated token burn</p>
                  <p className="mt-2 type-footnote text-label-tertiary">
                    Estimate includes only checks with available token-burn estimates.
                  </p>
                </div>
                <div className="mt-3 border-t border-separator pt-2">
                  <button
                    type="button"
                    onClick={() => {
                      close()
                      void openSettingsWindow("insights")
                    }}
                    className="burn-check-action type-callout"
                  >
                    Coverage details
                  </button>
                </div>
              </>
            )}
          </InfoPopover>
        )}
      </CollectionToolbar>
    </header>
  )
}
