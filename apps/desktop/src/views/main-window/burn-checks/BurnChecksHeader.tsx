import { pluralize } from "../../../lib/presentation/text"
import { CountPill } from "../../../components/ui/CountPill"
import { CollectionHeader } from "../../../components/ui/CollectionHeader"
import { Tooltip } from "../../../components/presentation/Tooltip"
import { CollectionToolbar } from "../../../components/ui/CollectionToolbar"
import { BURN_CHECK_MARKS } from "../../../components/burn-checks/burnCheckMarks"

import type { ChecksReportPayload } from "../../../lib/insightsIpc"
import { checksPresentation } from "../../../lib/presentation/checkReport"
import { snoozedCheckIds, useSnoozedBurnChecks } from "../../../lib/snoozedBurnChecks"
import { checkRows } from "../../../lib/presentation/checkRows"

export function BurnChecksHeader({ report }: { report?: ChecksReportPayload }) {
  const snoozes = useSnoozedBurnChecks()
  const failures =
    report && snoozes.status === "ready"
      ? checksPresentation(report, false, snoozedCheckIds(snoozes.records)).failures.length
      : 0
  const total = report ? checkRows(report).length : 0
  const FailureIcon = BURN_CHECK_MARKS.finding.Icon
  return (
    <div className="burn-checks-collection-header">
      <CollectionHeader
        title="Checks"
        summary={
          report ? (
            <CountPill
              count={total}
              size="regular"
              aria-live="polite"
              aria-atomic="true"
              aria-label={`${total} ${pluralize(total, "check type", "check types")} in total, including snoozed checks`}
            />
          ) : null
        }
        actions={
          <Tooltip label="Checks use the last 30 days of sessions.">
            <span
              tabIndex={0}
              aria-label="30 days, fixed analysis period"
              className="shrink-0 whitespace-nowrap rounded-control px-1 py-1 type-caption text-label-secondary tabular-nums"
            >
              30 days
            </span>
          </Tooltip>
        }
      />
      <CollectionToolbar className="burn-checks-collection-toolbar" topPadding="space-sm">
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
      </CollectionToolbar>
    </div>
  )
}
