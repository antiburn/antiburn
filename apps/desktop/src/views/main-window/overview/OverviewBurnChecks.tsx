import { ArrowRight, CheckCircle2, CircleDashed } from "lucide-react"

import type { ChecksCategoryPayload, ChecksReportPayload } from "../../../lib/insightsIpc"
import { checksPresentation } from "../../../lib/presentation/checks"
import { sessionCountLabel } from "../../../lib/presentation/providerUsage"
import { checkRowPresentation } from "../../checks/checkUi"

import { Skeleton } from "../../../components/ui/Skeleton"

/** The most finding rows the panel lists. The full report has the rest. */
const OVERVIEW_FINDING_ROWS = 2

type OverviewChecksState = "findings" | "passed" | "pending"

interface OverviewChecksSummary {
  state: OverviewChecksState
  /** The closing line under the rows, or null when the rows say enough. */
  footer: string | null
  /** The finding rows to list, highest estimated burn first. */
  rows: ChecksCategoryPayload[]
}

/**
 * Rank the failed checks for the short list: the highest estimated burn
 * first, then the most affected sessions. `checksPresentation` already
 * sorts by estimate, so only the tie needs breaking here.
 */
function rankedFailures(failures: readonly ChecksCategoryPayload[]): ChecksCategoryPayload[] {
  return [...failures].sort(
    (left, right) =>
      (right.estimatedTokenBurnBasisPoints ?? -1) -
        (left.estimatedTokenBurnBasisPoints ?? -1) || right.finding - left.finding,
  )
}

/**
 * Reduce the report to the panel's three states. A report that still has
 * evidence in flight and no finding yet reads as pending, never as a clean
 * pass: an unsettled zero is not a result.
 */
function overviewChecksSummary(report: ChecksReportPayload): OverviewChecksSummary {
  const presentation = checksPresentation(report)
  const failures = rankedFailures(presentation.failures)
  const passed = presentation.wins.length
  if (failures.length > 0) {
    return {
      state: "findings",
      // An unsettled report still says so. The rows alone would read as
      // the complete result.
      footer: report.evidenceSettled
        ? null
        : `Still assessing ${sessionCountLabel(report.pendingEvidence)}`,
      rows: failures.slice(0, OVERVIEW_FINDING_ROWS),
    }
  }
  if (report.evidenceSettled && passed > 0) {
    return { state: "passed", footer: "Nothing to review right now.", rows: [] }
  }
  return {
    state: "pending",
    footer: report.evidenceSettled
      ? "Findings appear after the first scan."
      : "Results appear when the scan finishes.",
    rows: [],
  }
}

/**
 * The Burn checks panel: at most two finding rows, each a button that opens
 * the full Burn checks section. A state with no finding shows one closing
 * line instead. The header matches the recent sessions header above the
 * page's other panel. The panel draws no card of its own; the Overview
 * page's stack card holds it above the recent sessions.
 */
export function OverviewBurnChecks({
  report,
  loading = false,
  onOpen,
}: {
  report: ChecksReportPayload | null
  loading?: boolean
  onOpen: () => void
}) {
  const summary = report ? overviewChecksSummary(report) : null
  const FooterIcon = summary?.state === "passed" ? CheckCircle2 : CircleDashed
  return (
    <section
      aria-label="Burn checks"
      aria-busy={loading || undefined}
      className="flex min-w-0 flex-col gap-[var(--space-sm)]"
    >
      <div className="flex items-baseline justify-between">
        <h2 className="type-caption text-label-secondary">Checks</h2>
        <button
          type="button"
          onClick={onOpen}
          className="inline-flex items-center gap-1 type-caption text-label-secondary hover:text-label hover:underline hover:underline-offset-[3px]"
        >
          All checks
          <ArrowRight size={12} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>
      {!summary && (
        // The rows keep their height while the report loads, so the panel
        // does not jump when the result arrives.
        <div className="flex flex-col gap-1.5">
          <Skeleton className="h-[34px] w-full rounded-[var(--radius-popover)]" />
          <Skeleton className="h-[34px] w-full rounded-[var(--radius-popover)]" />
        </div>
      )}
      {summary && summary.rows.length > 0 && (
        <ul className="flex flex-col gap-1.5">
          {summary.rows.map((check) => {
            const row = checkRowPresentation(check)
            return (
              <li key={check.id}>
                <button
                  type="button"
                  onClick={onOpen}
                  className="session-card grid w-full grid-cols-[auto_minmax(0,1fr)_max-content] items-center gap-3 rounded-[var(--radius-popover)] bg-session-card px-3 py-2 text-left type-callout text-label hover:bg-surface-secondary/50"
                >
                  <row.Icon
                    size={14}
                    strokeWidth={1.75}
                    className="text-label-tertiary"
                    aria-hidden="true"
                  />
                  <span className="truncate">{row.label}</span>
                  <span className="type-caption tabular-nums text-burn-check-failure-text">
                    {sessionCountLabel(check.finding)}
                  </span>
                </button>
              </li>
            )
          })}
        </ul>
      )}
      {summary?.footer && (
        <p
          className={`flex items-center gap-[var(--space-sm)] type-callout ${
            summary.rows.length > 0 ? "border-t border-separator pt-[var(--space-sm)]" : ""
          } ${summary.state === "passed" ? "text-burn-check-pass-fill" : "text-label-secondary"}`}
        >
          <FooterIcon size={14} strokeWidth={2} aria-hidden="true" />
          {summary.footer}
        </p>
      )}
    </section>
  )
}
