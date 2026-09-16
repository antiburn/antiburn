import { CollectionToolbar } from "../../../components/ui/CollectionToolbar"
import { BURN_CHECK_MARKS } from "../../../components/burn-checks/burnCheckMarks"

import type { ChecksReportPayload } from "../../../lib/insightsIpc"
import { isMacOS } from "../../../lib/platform"
import { checksPresentation } from "../../../lib/presentation/checks"

export function BurnChecksHeader({ report }: { report?: ChecksReportPayload }) {
  const failures = report ? checksPresentation(report).failures.length : 0
  const FailureIcon = BURN_CHECK_MARKS.finding.Icon
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
      </CollectionToolbar>
    </header>
  )
}
