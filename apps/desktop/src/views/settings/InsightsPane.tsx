// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Check, CircleAlert, RefreshCw } from "lucide-react"
import { useState, useSyncExternalStore } from "react"

import { Card } from "../../components/ui/Card"
import { PaneHeader } from "../../components/ui/Pane"
import { PushButton } from "../../components/ui/PushButton"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { Skeleton } from "../../components/ui/Skeleton"
import { StatusText } from "../../components/ui/StatusText"
import type {
  InsightsCategoryPayload,
  InsightsCoveragePayload,
  InsightsNotAssessedReason,
  InsightsQuotaPressurePayload,
  InsightsStatusPayload,
} from "../../lib/ipc"
import { InsightsSession, type InsightsSnapshot } from "./InsightsSession"

/**
 * Insights: what thirty days of local session evidence supports saying
 * about the reader's own working habits — and, just as loudly, what it
 * does not support saying yet.
 *
 * Everything here is presentational. The report fetch, its in-flight and
 * error state, and the processing-status poll live in `InsightsSession`;
 * this component renders a snapshot and calls session methods. It has no
 * dependency on being inside the settings window, so a future standalone
 * window can mount it unchanged (the portability test proves that).
 *
 * The one presentation rule that outranks layout: the coverage denominator
 * is separate from the assessed cohort (FR-12). A session that is pending,
 * processing, failed, unsupported, stale, or missing a start time is named
 * as exactly that, and never allowed to read as assessed or as clean.
 *
 * The prior Insights pane the master plan cites as a visual reference
 * lives in a private repository and is not available in this tree, so the
 * layout here derives from the acceptance criteria alone.
 */

/** Reader-facing names for the nine category identifiers. */
const CATEGORY_LABELS: Record<string, string> = {
  sessionsOverDepth: "Sessions over depth",
  modelOverthinking: "Model overthinking",
  overpoweredSubagents: "Overpowered subagents",
  unusedMcpServers: "Unused MCP servers",
  unusedBuiltInTools: "Unused built-in tools",
  unusedSkills: "Unused skills",
  oldModelUsage: "Old model usage",
  overuseOfFastMode: "Overuse of fast mode",
  cacheChurn: "Cache churn",
}

/** Why a category was not assessed, in the reader's terms. Every reason is
 *  an honest "cannot say", never a soft "all good". */
const NOT_ASSESSED_WORDING: Record<InsightsNotAssessedReason, string> = {
  noSessionsInWindow: "Not assessed — no processed sessions in this window",
  capabilityMissing:
    "Not assessed — these sessions do not record the evidence this check needs",
  incompleteEvidence:
    "Not assessed — evidence is incomplete, so a clean result cannot be claimed",
  evidenceContractIncomplete: "Not assessed — stored evidence cannot express this check yet",
}

/** Reader-facing names for the quota limit-kind identifiers. */
const LIMIT_KIND_LABELS: Record<string, string> = {
  rollingWindow: "Rolling window",
  weekly: "Weekly",
  modelSpecific: "Model-specific",
  weightedUsage: "Weighted usage",
  rateLimit: "Rate limit",
}

export function InsightsPane() {
  const [session] = useState(() => new InsightsSession())
  const snapshot = useSyncExternalStore(session.subscribe, session.getSnapshot)

  return (
    <>
      <PaneHeader title="Insights" />
      <div className="space-y-6">
        <InsightsBody snapshot={snapshot} onRecalculate={() => void session.refresh()} />
        <p className="type-footnote text-label-secondary">
          Computed on this device from local session transcripts covering the last 30 days of
          this machine&apos;s native environment. Nothing leaves this device. The report
          reflects evidence processed so far; sessions still waiting are counted above, never
          assessed silently.
        </p>
      </div>
    </>
  )
}

function InsightsBody({
  snapshot,
  onRecalculate,
}: {
  snapshot: InsightsSnapshot
  onRecalculate: () => void
}) {
  if (snapshot.phase === "loading") {
    // While the report computes nothing below may render: an in-flight
    // report must never look like an empty or clean one.
    return (
      <SectionGroup
        title="Coverage"
        trailing={<StatusText tone="secondary">Computing the report…</StatusText>}
      >
        <Card>
          <div className="space-y-2 px-4 py-3" aria-label="Computing the insights report">
            <Skeleton className="h-4 w-48" />
            <Skeleton className="h-3 w-64" />
            <Skeleton className="h-3 w-56" />
          </div>
        </Card>
      </SectionGroup>
    )
  }

  if (snapshot.phase === "error") {
    return (
      <SectionGroup title="Coverage">
        <Card>
          <div className="flex items-center justify-between gap-3 px-4 py-3">
            <StatusText icon={CircleAlert} iconClassName="text-system-red" tone="secondary">
              The report could not be computed. Nothing was assessed.
            </StatusText>
            <PushButton className="shrink-0 gap-1.5" onClick={onRecalculate}>
              <RefreshCw size={12} aria-hidden="true" />
              Try again
            </PushButton>
          </div>
        </Card>
      </SectionGroup>
    )
  }

  const report = snapshot.report
  if (!report) {
    return (
      <SectionGroup title="Coverage">
        <Card>
          <p className="type-footnote px-4 py-3 text-label-secondary">
            The insights report is only available inside the antiburn app.
          </p>
        </Card>
      </SectionGroup>
    )
  }

  const coverage = report.coverage
  const nothingProcessedYet = coverage.discovered > 0 && coverage.ready === 0

  return (
    <>
      <CoverageSection
        coverage={coverage}
        assessedSessions={report.assessedSessions}
        status={snapshot.status}
        nothingProcessedYet={nothingProcessedYet}
        onRecalculate={onRecalculate}
      />
      <CategoriesSection categories={report.categories} />
      <QuotaPressureSection quota={report.quotaPressure} />
    </>
  )
}

/** One denominator row outside the assessed cohort: a count and the reason
 *  it is not assessed. The wording is the FR-12 guarantee — none of these
 *  rows may read as assessed or as clean. */
const COVERAGE_ROWS: readonly {
  key: keyof InsightsCoveragePayload
  wording: string
}[] = [
  { key: "pending", wording: "waiting to be processed — not assessed yet" },
  { key: "processing", wording: "being processed now — not assessed yet" },
  { key: "failed", wording: "could not be processed — not assessed" },
  { key: "unsupported", wording: "do not carry readable evidence — not assessed" },
  { key: "stale", wording: "have out-of-date evidence — not assessed until reprocessed" },
  { key: "unknownStart", wording: "have no trustworthy start time — not assessed" },
]

function CoverageSection({
  coverage,
  assessedSessions,
  status,
  nothingProcessedYet,
  onRecalculate,
}: {
  coverage: InsightsCoveragePayload
  assessedSessions: number
  status: InsightsStatusPayload | null
  nothingProcessedYet: boolean
  onRecalculate: () => void
}) {
  const backlog = status ? status.pending + status.processing : 0
  return (
    <SectionGroup
      title="Coverage"
      trailing={
        <StatusText tone="secondary">
          {coverage.discovered} discovered · {assessedSessions} assessed
        </StatusText>
      }
    >
      <Card>
        <div className="space-y-2 px-4 py-3">
          {nothingProcessedYet && (
            // The cold-open case, named explicitly: sessions exist and
            // none has finished processing. Deliberately its own state
            // rather than an empty findings list.
            <div>
              <p className="type-body text-label">Nothing has been processed yet</p>
              <p className="type-footnote mt-0.5 text-label-secondary">
                antiburn found {coverage.discovered}{" "}
                {coverage.discovered === 1 ? "session" : "sessions"} in the last 30 days and has
                not finished reading any of them. Findings appear here as evidence processing
                completes — nothing below is a verdict yet.
              </p>
            </div>
          )}
          <p className="type-footnote text-label">
            {coverage.discovered} {coverage.discovered === 1 ? "session" : "sessions"}{" "}
            discovered in the last 30 days. {assessedSessions} in the assessed cohort
            {coverage.activelyGrowing > 0 ? ` (${coverage.activelyGrowing} still growing)` : ""}
            .
          </p>
          {coverage.discovered === 0 && (
            <p className="type-footnote text-label-secondary">
              No sessions were found in the last 30 days, so there is nothing to assess.
            </p>
          )}
          <ul className="space-y-1">
            {COVERAGE_ROWS.filter(({ key }) => coverage[key] > 0).map(({ key, wording }) => (
              <li key={key} className="type-footnote text-label-secondary">
                {coverage[key]} {wording}
              </li>
            ))}
          </ul>
          <div className="flex items-center justify-between gap-3">
            <StatusText tone="secondary">
              {status?.calculating
                ? "Recomputing the report…"
                : backlog > 0
                  ? `${status?.pending ?? 0} waiting and ${status?.processing ?? 0} processing now`
                  : "Evidence processing is caught up"}
            </StatusText>
            <PushButton className="shrink-0 gap-1.5" onClick={onRecalculate}>
              <RefreshCw size={12} aria-hidden="true" />
              Recalculate
            </PushButton>
          </div>
        </div>
      </Card>
    </SectionGroup>
  )
}

function CategoriesSection({ categories }: { categories: InsightsCategoryPayload[] }) {
  return (
    <SectionGroup title="Categories">
      <Card>
        {categories.map((category) => (
          <div
            key={category.id}
            className="flex min-h-[44px] items-center justify-between gap-3 px-4 py-2.5"
          >
            <p className="type-body min-w-0 truncate text-label">
              {CATEGORY_LABELS[category.id] ?? category.id}
            </p>
            <CategoryStatus category={category} />
          </div>
        ))}
      </Card>
    </SectionGroup>
  )
}

function CategoryStatus({ category }: { category: InsightsCategoryPayload }) {
  if (category.status === "findings") {
    const count = category.findingSessions ?? 0
    return (
      <StatusText icon={CircleAlert} iconClassName="text-system-orange">
        {count} {count === 1 ? "session" : "sessions"} with findings
      </StatusText>
    )
  }
  if (category.status === "clean") {
    // Clean is a positive claim the engine only makes over complete
    // evidence, so it names the cohort it covers.
    return (
      <StatusText icon={Check} iconClassName="text-system-green" iconStrokeWidth={2.5}>
        Clean across {category.assessed} assessed{" "}
        {category.assessed === 1 ? "session" : "sessions"}
      </StatusText>
    )
  }
  return (
    <StatusText tone="secondary" className="text-right">
      {category.notAssessedReason
        ? NOT_ASSESSED_WORDING[category.notAssessedReason]
        : "Not assessed"}
    </StatusText>
  )
}

function QuotaPressureSection({ quota }: { quota: InsightsQuotaPressurePayload }) {
  return (
    <SectionGroup title="Quota pressure">
      <Card>
        {!quota.assessed || !quota.findings ? (
          <p className="type-footnote px-4 py-3 text-label-secondary">
            Not assessed — the sessions in this window carry no quota evidence.
          </p>
        ) : (
          <div className="space-y-2 px-4 py-3">
            <StatusText icon={CircleAlert} iconClassName="text-system-orange">
              {quota.findings.totalHits} limit {quota.findings.totalHits === 1 ? "hit" : "hits"}{" "}
              across {quota.findings.affectedSessionCount}{" "}
              {quota.findings.affectedSessionCount === 1 ? "session" : "sessions"}
            </StatusText>
            <p className="type-footnote text-label-secondary">
              {quota.findings.hardHits} hard {quota.findings.hardHits === 1 ? "hit" : "hits"} ·{" "}
              {quota.findings.warnings} {quota.findings.warnings === 1 ? "warning" : "warnings"}
            </p>
            <ul className="space-y-1">
              {quota.findings.hitsByLimitKind.map(({ kind, hits }) => (
                <li key={kind} className="type-footnote text-label-secondary">
                  {LIMIT_KIND_LABELS[kind] ?? kind}: {hits} {hits === 1 ? "hit" : "hits"}
                </li>
              ))}
            </ul>
            {quota.findings.affectedModels.length > 0 && (
              <p className="type-footnote text-label-secondary">
                Models: {quota.findings.affectedModels.join(", ")}
                {quota.findings.affectedModelsTruncated ? " and more" : ""}
              </p>
            )}
          </div>
        )}
      </Card>
    </SectionGroup>
  )
}
