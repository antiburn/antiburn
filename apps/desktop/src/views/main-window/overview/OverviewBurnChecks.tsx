import { CheckCircle2, CircleDashed } from "lucide-react"

import type { ChecksCategoryPayload, ChecksReportPayload } from "../../../lib/insightsIpc"
import {
  aggregateBurnCheckPresentation,
  emptyBurnCheckPresentation,
} from "../../../lib/presentation/burnChecks"
import { checksPresentation, formatTokenBurnPercent } from "../../../lib/presentation/checks"
import { sessionCountLabel } from "../../../lib/presentation/providerUsage"
import { checkRowPresentation } from "../../checks/checkUi"

import { BurnCheckIndicator } from "../../../components/burn-checks/BurnCheckIndicator"
import { BURN_CHECK_MARKS } from "../../../components/burn-checks/burnCheckMarks"
import { SegmentedRadialDial } from "../../../components/ui/SegmentedRadialDial"
import { Skeleton } from "../../../components/ui/Skeleton"

/** The most finding rows the panel lists. The full report has the rest. */
const OVERVIEW_FINDING_ROWS = 2

/** The session list's check dial, at a size that carries a two-line title. */
const OVERVIEW_DIAL_SIZE = 44

/** A whole burn gauge, in basis points. */
const BURN_GAUGE_FULL_BASIS_POINTS = 10_000
/** The smallest arc the gauge draws, so a trace of burn still shows. */
const BURN_GAUGE_MIN_BASIS_POINTS = 100

type OverviewChecksState = "findings" | "passed" | "pending"

interface OverviewChecksSummary {
  state: OverviewChecksState
  /** The estimated burn share in basis points, or null when unknown. */
  burn: number | null
  /** The prominent line: the burn estimate when known, else the result. */
  headline: string
  /** The muted line under the headline, or null when nothing adds to it. */
  detail: string | null
  /** The finding rows to list, highest estimated burn first. */
  rows: ChecksCategoryPayload[]
}

function countLabel(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`
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
    const burn = report.estimatedTokenBurnBasisPoints
    const result = [
      countLabel(failures.length, "finding"),
      passed > 0 ? `${passed} passed` : null,
    ]
      .filter(Boolean)
      .join(" · ")
    if (burn != null) {
      return {
        state: "findings",
        burn,
        headline: `${formatTokenBurnPercent(burn).replace("<", "Less than ")} estimated burn`,
        detail: result,
        rows: failures.slice(0, OVERVIEW_FINDING_ROWS),
      }
    }
    return {
      state: "findings",
      burn: null,
      headline: result,
      detail: report.evidenceSettled
        ? null
        : `Still assessing ${sessionCountLabel(report.pendingEvidence)}`,
      rows: failures.slice(0, OVERVIEW_FINDING_ROWS),
    }
  }
  if (report.evidenceSettled && passed > 0) {
    return {
      state: "passed",
      burn: null,
      headline: `All ${countLabel(passed, "check")} passed`,
      detail: null,
      rows: [],
    }
  }
  return {
    state: "pending",
    burn: null,
    headline: report.evidenceSettled ? "No checks assessed" : "Assessing sessions",
    detail: report.evidenceSettled
      ? "Findings appear after the first scan."
      : "Results appear when the scan finishes.",
    rows: [],
  }
}

/**
 * The dial beside the headline. With a burn estimate it is a gauge: the
 * finding colour fills the estimated share of the ring and the neutral
 * colour the rest, so it reads with the "estimated burn" line. Without an
 * estimate it is the session list's check dial, one arc per check. A
 * missing report draws the pending mark.
 */
function ChecksDial({
  report,
  burn,
}: {
  report: ChecksReportPayload | null
  burn: number | null
}) {
  if (burn != null) {
    const lit = Math.min(
      BURN_GAUGE_FULL_BASIS_POINTS,
      Math.max(BURN_GAUGE_MIN_BASIS_POINTS, burn),
    )
    return (
      <SegmentedRadialDial
        size={OVERVIEW_DIAL_SIZE}
        strokeWidth={3}
        segments={[
          { id: "burn", value: lit, className: BURN_CHECK_MARKS.finding.iconClass },
          {
            id: "rest",
            value: BURN_GAUGE_FULL_BASIS_POINTS - lit,
            className: BURN_CHECK_MARKS.notAssessed.iconClass,
          },
        ]}
      />
    )
  }
  const presentation = report
    ? aggregateBurnCheckPresentation(report)
    : emptyBurnCheckPresentation("pending")
  return <BurnCheckIndicator presentation={presentation} size={OVERVIEW_DIAL_SIZE} />
}

/**
 * The Burn checks panel: the burn gauge beside the burn estimate, then at
 * most two finding rows. The header and every row are buttons that
 * open the full Burn checks section. The panel draws no card of its own;
 * the Overview page's stack card holds it above the recent sessions.
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
    <section aria-label="Burn checks" aria-busy={loading || undefined} className="min-w-0">
      <button
        type="button"
        onClick={onOpen}
        aria-label={summary ? `Open Burn checks: ${summary.headline}` : "Open Burn checks"}
        className="group flex w-full items-center gap-[var(--space-md)] rounded-control text-left"
      >
        <span aria-hidden="true" className="grid shrink-0 place-items-center">
          <ChecksDial report={report} burn={summary?.burn ?? null} />
        </span>
        <span className="min-w-0 flex-1">
          {summary ? (
            <>
              <span className="block truncate type-title-2 text-label group-hover:text-brand">
                {summary.headline}
              </span>
              {summary.detail && (
                <span className="block truncate type-callout text-label-secondary">
                  {summary.detail}
                </span>
              )}
            </>
          ) : (
            <>
              <Skeleton className="h-4 w-36" />
              <Skeleton className="mt-1.5 h-3 w-24" />
            </>
          )}
        </span>
      </button>
      {summary?.state === "findings" && (
        <ul className="mt-[var(--space-md)] flex flex-col gap-1.5">
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
      {summary && summary.state !== "findings" && (
        <p
          className={`mt-[var(--space-md)] flex items-center gap-[var(--space-sm)] border-t border-separator pt-[var(--space-sm)] type-callout ${
            summary.state === "passed" ? "text-burn-check-pass-fill" : "text-label-secondary"
          }`}
        >
          <FooterIcon size={14} strokeWidth={2} aria-hidden="true" />
          {summary.state === "passed"
            ? "Nothing to review right now."
            : "Findings appear here once the scan finishes."}
        </p>
      )}
    </section>
  )
}
