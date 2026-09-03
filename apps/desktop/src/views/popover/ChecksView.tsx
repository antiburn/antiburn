import {
  Bot,
  Brain,
  CheckCircle2,
  CircleDashed,
  CircleX,
  Database,
  Gauge,
  History,
  Layers3,
  Server,
  Wrench,
  type LucideIcon,
} from "lucide-react"
import { useRef } from "react"

import { measureAnchorRegion } from "../../lib/anchorRegion"
import type { ChecksCategoryPayload } from "../../lib/insightsIpc"
import {
  CHECK_LABELS,
  formatTokenBurnPercent,
  tokenBurnTone,
  type ChecksPresentation,
} from "../../lib/presentation/checks"

const CHECK_ICONS: Record<string, LucideIcon> = {
  sessionsOverDepth: Layers3,
  modelOverthinking: Brain,
  overpoweredSubagents: Bot,
  unusedMcpServers: Server,
  unusedBuiltInTools: Wrench,
  unusedSkills: Wrench,
  oldModelUsage: History,
  overuseOfFastMode: Gauge,
  cacheChurn: Database,
}

function findingCount(category: ChecksCategoryPayload): string {
  const count = category.finding
  return `${count} failed session${count === 1 ? "" : "s"}`
}

function coverageSummary(category: ChecksCategoryPayload, includeFinding: boolean): string {
  const parts: string[] = []
  if (includeFinding) parts.push(findingCount(category))
  if (category.clean > 0) parts.push(`${category.clean} passed`)
  if (category.unavailable > 0) parts.push(`${category.unavailable} need evidence`)
  return parts.join(" · ")
}

function tokenEstimate(category: ChecksCategoryPayload): string | null {
  return category.estimatedTokenBurnBasisPoints == null
    ? null
    : `~${formatTokenBurnPercent(category.estimatedTokenBurnBasisPoints)} token burn`
}

function summaryEstimate(presentation: ChecksPresentation): string | null {
  const basisPoints = presentation.estimate.tokenBurnBasisPoints
  return basisPoints == null || presentation.failures.length === 0
    ? null
    : `~${formatTokenBurnPercent(basisPoints)} token burn`
}

function refreshFailureSuffix(presentation: ChecksPresentation): string {
  return presentation.refreshUnavailable ? " · refresh unavailable" : ""
}

export function ChecksSummary({
  active,
  presentation,
  reportUnavailable,
  onPreview,
  onLeave,
}: {
  active: boolean
  presentation: ChecksPresentation | null
  reportUnavailable: boolean
  onPreview: (anchor: ReturnType<typeof measureAnchorRegion>) => void
  onLeave: () => void
}) {
  const failures = presentation?.failures.length ?? 0
  const wins = presentation?.wins.length ?? 0
  const hasFindings = failures > 0
  const hasWins = wins > 0
  const checksNeedingEvidence = presentation
    ? [...presentation.failures, ...presentation.wins, ...presentation.unavailable].filter(
        (category) => category.unavailable > 0,
      ).length
    : 0
  const completePass =
    hasWins && presentation?.unavailable.length === 0 && checksNeedingEvidence === 0
  const StatusIcon =
    presentation == null
      ? CircleDashed
      : hasFindings
        ? CircleX
        : completePass
          ? CheckCircle2
          : CircleDashed
  const estimate = presentation ? summaryEstimate(presentation) : null
  const hovered = useRef(false)
  const focused = useRef(false)

  return (
    <div
      data-state={active ? "active" : "idle"}
      onMouseEnter={(event) => {
        hovered.current = true
        if (presentation) onPreview(measureAnchorRegion(event.currentTarget))
      }}
      onMouseLeave={() => {
        hovered.current = false
        if (!focused.current) onLeave()
      }}
      className="group flex items-center rounded-control hover:bg-surface-hover data-[state=active]:bg-surface-selected"
    >
      <div
        tabIndex={presentation ? 0 : undefined}
        aria-disabled={!presentation}
        aria-busy={!presentation && !reportUnavailable}
        onFocus={(event) => {
          focused.current = true
          if (presentation) onPreview(measureAnchorRegion(event.currentTarget))
        }}
        onBlur={() => {
          focused.current = false
          if (!hovered.current) onLeave()
        }}
        className="grid min-w-0 flex-1 grid-cols-[16px_minmax(0,1fr)_max-content] items-center gap-x-2 px-2 py-2 text-left"
      >
        <StatusIcon
          size={14}
          strokeWidth={presentation == null ? 2 : 2.5}
          className={`shrink-0 ${hasFindings ? "text-system-red-text" : "text-label-tertiary"}`}
          aria-hidden="true"
        />
        <span className="min-w-0">
          <span className="block type-body font-medium! text-label">All checks</span>
          <span className="block truncate type-footnote text-label-secondary">
            {presentation &&
              (hasFindings
                ? `${failures} check${failures === 1 ? "" : "s"} failed`
                : hasWins
                  ? `${wins} check${wins === 1 ? "" : "s"} passed${checksNeedingEvidence > 0 ? ` · ${checksNeedingEvidence} need evidence` : ""}`
                  : "More evidence needed")}
            {presentation && refreshFailureSuffix(presentation)}
            {!presentation &&
              (reportUnavailable ? "Checks unavailable" : "Checking local sessions…")}
          </span>
        </span>
        <span
          className={`type-footnote font-medium! tabular-nums ${presentation?.estimate.tokenBurnBasisPoints == null ? "text-label-secondary" : tokenBurnTone(presentation.estimate.tokenBurnBasisPoints)}`}
        >
          {presentation ? estimate : null}
        </span>
      </div>
    </div>
  )
}

function FailureRows({ failures }: { failures: readonly ChecksCategoryPayload[] }) {
  return (
    <div className="mt-2 overflow-hidden rounded-control border border-separator">
      {failures.map((check) => {
        const Icon = CHECK_ICONS[check.id] ?? CircleX
        const estimate = tokenEstimate(check)
        return (
          <div
            key={check.id}
            className="group grid grid-cols-[28px_minmax(0,1fr)_max-content] items-center gap-x-2 border-b border-separator bg-surface-card px-2 py-2.5 last:border-b-0"
          >
            <span className="flex h-7 w-7 items-center justify-center rounded-control bg-system-red/10 text-system-red-text">
              <Icon size={15} strokeWidth={2} aria-hidden="true" />
            </span>
            <span className="min-w-0">
              <span className="block truncate type-body font-medium! text-label">
                {CHECK_LABELS[check.id] ?? check.id}
              </span>
              <span className="block type-footnote tabular-nums text-label-tertiary">
                {coverageSummary(check, true)}
              </span>
            </span>
            {estimate && (
              <span
                className={`flex items-center gap-1 type-footnote font-medium! tabular-nums ${tokenBurnTone(check.estimatedTokenBurnBasisPoints!)}`}
              >
                {estimate}
              </span>
            )}
          </div>
        )
      })}
    </div>
  )
}

function WinRows({ wins }: { wins: readonly ChecksCategoryPayload[] }) {
  return (
    <div className="mt-2 overflow-hidden rounded-control border border-separator">
      {wins.map((win) => {
        const Icon = CHECK_ICONS[win.id] ?? CheckCircle2
        return (
          <div
            key={win.id}
            className="grid grid-cols-[28px_minmax(0,1fr)_max-content] items-center gap-x-2 border-b border-separator bg-surface-card px-2 py-2 last:border-b-0"
          >
            <span className="flex h-7 w-7 items-center justify-center rounded-control bg-system-green/10 text-system-green">
              <Icon size={15} strokeWidth={2} aria-hidden="true" />
            </span>
            <span className="min-w-0">
              <span className="block truncate type-body text-label">
                {CHECK_LABELS[win.id] ?? win.id}
              </span>
              <span className="block truncate type-footnote tabular-nums text-label-tertiary">
                {coverageSummary(win, false)}
              </span>
            </span>
            <span className="type-footnote font-medium! text-label-secondary">Passed</span>
          </div>
        )
      })}
    </div>
  )
}

export function ChecksPeek({ presentation }: { presentation: ChecksPresentation }) {
  const { failures, wins, unavailable, estimate } = presentation
  const hasFindings = failures.length > 0
  const hasWins = wins.length > 0
  const checksNeedingEvidence = [...failures, ...wins, ...unavailable].filter(
    (category) => category.unavailable > 0,
  ).length
  const completePass = hasWins && unavailable.length === 0 && checksNeedingEvidence === 0
  const summaryDetail = hasFindings
    ? estimate.tokenBurnBasisPoints == null
      ? null
      : "Estimated share of tokens spent on avoidable work."
    : completePass
      ? `${wins.length} check${wins.length === 1 ? "" : "s"} passed`
      : hasWins
        ? `${wins.length} check${wins.length === 1 ? "" : "s"} passed · ${checksNeedingEvidence} need evidence`
        : "More evidence is needed"
  const summaryStatus = [
    summaryDetail,
    presentation.refreshUnavailable ? "Refresh unavailable" : null,
  ]
    .filter(Boolean)
    .join(" · ")

  return (
    <div className="px-3 py-3 text-label">
      <div className="flex items-baseline justify-between gap-3 px-1">
        <h1 className="type-headline text-label">All checks</h1>
        <span className="type-footnote text-label-tertiary">Last 30 days</span>
      </div>

      <section className="mt-3 flex items-center gap-3 rounded-control border border-separator bg-surface-card p-3">
        <span
          className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-full ${hasFindings ? "bg-system-red/10 text-system-red-text" : "bg-surface-secondary text-label-tertiary"}`}
        >
          {hasFindings ? (
            <CircleX size={20} strokeWidth={2.5} aria-hidden="true" />
          ) : completePass ? (
            <CheckCircle2 size={20} strokeWidth={2.5} aria-hidden="true" />
          ) : (
            <CircleDashed size={20} strokeWidth={2} aria-hidden="true" />
          )}
        </span>
        <span className="min-w-0">
          <span
            className={`block type-title-2 tabular-nums ${hasFindings && estimate.tokenBurnBasisPoints != null ? tokenBurnTone(estimate.tokenBurnBasisPoints) : "text-label"}`}
          >
            {hasFindings
              ? estimate.tokenBurnBasisPoints == null
                ? `${failures.length} check${failures.length === 1 ? "" : "s"} failed`
                : `~${formatTokenBurnPercent(estimate.tokenBurnBasisPoints)} token burn`
              : completePass
                ? "All checks passed"
                : hasWins
                  ? "No issues found where assessed"
                  : "More evidence is needed"}
          </span>
          {summaryStatus && (
            <span className="block type-footnote text-label-secondary">{summaryStatus}</span>
          )}
        </span>
      </section>

      {failures.length > 0 && (
        <section className="mt-4" aria-labelledby="checks-attention">
          <h2 id="checks-attention" className="px-1 type-caption text-label-tertiary">
            Failed checks
          </h2>
          <FailureRows failures={failures} />
        </section>
      )}

      {wins.length > 0 && (
        <section className="mt-4" aria-labelledby="passing-checks">
          <h2 id="passing-checks" className="px-1 type-caption text-label-tertiary">
            Passed checks
          </h2>
          <WinRows wins={wins} />
        </section>
      )}
    </div>
  )
}
