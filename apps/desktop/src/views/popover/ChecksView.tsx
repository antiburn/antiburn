import { CheckCircle2, CircleDashed, CircleX, Flame } from "lucide-react"
import { useRef } from "react"

import { TextRoll } from "../../components/ui/TextRoll"
import { measureAnchorRegion } from "../../lib/anchorRegion"
import type { ChecksCategoryPayload } from "../../lib/insightsIpc"
import {
  checksHeroPresentation,
  formatTokenBurnPercent,
  tokenBurnTone,
  type ChecksPresentation,
} from "../../lib/presentation/checks"
import { checkRowPresentation } from "../checks/checkUi"

function summaryEstimate(presentation: ChecksPresentation): string | null {
  const basisPoints = presentation.estimate.tokenBurnBasisPoints
  return basisPoints == null || presentation.failures.length === 0
    ? null
    : `${formatTokenBurnPercent(basisPoints)} token burn`
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
  onOpen = () => undefined,
}: {
  active: boolean
  presentation: ChecksPresentation | null
  reportUnavailable: boolean
  onPreview: (anchor: ReturnType<typeof measureAnchorRegion>) => void
  onLeave: () => void
  onOpen?: () => void
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
      <button
        type="button"
        disabled={!presentation}
        aria-busy={!presentation && !reportUnavailable}
        onClick={onOpen}
        onFocus={(event) => {
          focused.current = true
          if (presentation) onPreview(measureAnchorRegion(event.currentTarget))
        }}
        onBlur={() => {
          focused.current = false
          if (!hovered.current) onLeave()
        }}
        className="grid min-w-0 flex-1 grid-cols-[16px_minmax(0,1fr)_max-content] items-center gap-x-2 rounded-control px-2 py-2 text-left disabled:opacity-100 active:transform-none active:opacity-100"
      >
        <StatusIcon
          size={14}
          strokeWidth={presentation == null ? 2 : 2.5}
          className={`shrink-0 ${hasFindings ? "text-system-red-text" : completePass ? "text-system-green" : "text-label-tertiary"}`}
          aria-hidden="true"
        />
        <span className="min-w-0">
          <span className="block type-body font-medium! text-label">Burn checks</span>
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
          {presentation && estimate ? <TextRoll text={estimate} /> : null}
        </span>
      </button>
    </div>
  )
}

function CheckRows({ checks }: { checks: readonly ChecksCategoryPayload[] }) {
  return (
    <div className="mt-2 overflow-hidden rounded-control border border-separator">
      {checks.map((check) => {
        const row = checkRowPresentation(check)
        const { Icon } = row
        return (
          <div
            key={check.id}
            className="grid grid-cols-[28px_minmax(0,1fr)_max-content] items-center gap-x-2 border-b border-separator bg-surface-card px-2 py-2.5 last:border-b-0"
          >
            <span
              className={`flex h-7 w-7 items-center justify-center rounded-control ${row.iconTone}`}
            >
              <Icon size={15} strokeWidth={2} aria-hidden="true" />
            </span>
            <span className="min-w-0">
              <span className="block truncate type-body font-medium! text-label">
                {row.label}
              </span>
              <span className="block truncate type-footnote tabular-nums text-label-tertiary">
                {row.summary}
              </span>
            </span>
            {row.metric && (
              <span
                className={`flex items-center gap-1 type-footnote font-medium! tabular-nums ${row.metricTone}`}
              >
                <TextRoll text={row.metric} />
              </span>
            )}
          </div>
        )
      })}
    </div>
  )
}

export function ChecksPeek({ presentation }: { presentation: ChecksPresentation }) {
  const { failures, wins, estimate } = presentation
  const hero = checksHeroPresentation(presentation)
  const hasFindings = hero.state === "failed"
  const completePass = hero.state === "passed"
  const summaryStatus = [
    hero.summary,
    presentation.refreshUnavailable ? "Refresh unavailable" : null,
  ]
    .filter(Boolean)
    .join(" · ")

  return (
    <div className="px-3 py-3 text-label">
      <div className="flex items-baseline justify-between gap-3 px-1">
        <h1 className="type-headline text-label">Burn checks</h1>
        <span className="type-footnote text-label-tertiary">Last 30 days</span>
      </div>

      <section className="mt-3 flex items-center gap-3 rounded-control border border-separator bg-surface-card p-3">
        <span
          className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-full ${hasFindings ? "bg-system-red/10 text-system-red-text" : completePass ? "bg-system-green/10 text-system-green" : "bg-surface-secondary text-label-tertiary"}`}
        >
          {hasFindings ? (
            <Flame size={24} strokeWidth={2.5} aria-hidden="true" />
          ) : completePass ? (
            <CheckCircle2 size={24} strokeWidth={2.5} aria-hidden="true" />
          ) : (
            <CircleDashed size={24} strokeWidth={2} aria-hidden="true" />
          )}
        </span>
        <span className="min-w-0">
          <span className={`block type-title-2 tabular-nums ${hero.tone}`}>
            {hasFindings && estimate.tokenBurnBasisPoints != null ? (
              <TextRoll text={hero.result} />
            ) : (
              hero.result
            )}
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
          <CheckRows checks={failures} />
        </section>
      )}

      {wins.length > 0 && (
        <section className="mt-4" aria-labelledby="passing-checks">
          <h2 id="passing-checks" className="px-1 type-caption text-label-tertiary">
            Passed checks
          </h2>
          <CheckRows checks={wins} />
        </section>
      )}
    </div>
  )
}
