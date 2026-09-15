import { CheckCircle2, CircleDashed, Flame, LoaderCircle } from "lucide-react"
import { useRef } from "react"

import { TextRoll } from "../../components/ui/TextRoll"
import { BurnCheckSummary } from "../../components/burn-checks/BurnCheckSummary"
import { BurnCheckFlames } from "../../components/burn-checks/BurnCheckFlames"
import "../../components/burn-checks/burn-check-summary.css"
import { measureAnchorRegion } from "../../lib/anchorRegion"
import type { ChecksCategoryPayload } from "../../lib/insightsIpc"
import {
  checksHeroPresentation,
  formatTokenBurnPercent,
  type ChecksPresentation,
} from "../../lib/presentation/checks"
import { checkRowPresentation } from "../checks/checkUi"
import { emptyBurnCheckPresentation } from "../../lib/presentation/burnChecks"

function summaryEstimate(presentation: ChecksPresentation): string | null {
  const basisPoints = presentation.estimate.tokenBurnBasisPoints
  return basisPoints == null || presentation.failures.length === 0
    ? null
    : `${formatTokenBurnPercent(basisPoints)} token burn`
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
  const estimate = presentation ? summaryEstimate(presentation) : null
  const burnChecks =
    presentation?.burnChecks ??
    emptyBurnCheckPresentation(reportUnavailable ? "unavailable" : "pending")
  const accessibleLabel = estimate
    ? `${burnChecks.accessibleDescription} ${estimate}.`
    : burnChecks.accessibleDescription
  const tone =
    burnChecks.counts.failed > 0 ? "failure" : burnChecks.counts.passed > 0 ? "pass" : "neutral"
  const hovered = useRef(false)
  const focused = useRef(false)
  const summary = useRef<HTMLDivElement>(null)

  return (
    <div
      ref={summary}
      data-state={active ? "active" : "idle"}
      data-tone={tone}
      onMouseEnter={(event) => {
        hovered.current = true
        if (event.target instanceof Element && event.target.closest("[data-burn-check-flames]"))
          return
        if (presentation) onPreview(measureAnchorRegion(event.currentTarget))
      }}
      onMouseLeave={() => {
        hovered.current = false
        if (!focused.current) onLeave()
      }}
      className="burn-check-summary-surface group flex items-center rounded-control bg-surface-card/50 transition-colors duration-fast hover:bg-surface-secondary/70 focus-within:bg-surface-secondary/70 data-[state=active]:bg-surface-selected/40"
    >
      <button
        type="button"
        disabled={!presentation}
        data-burn-check-summary-trigger
        aria-label={`All burn checks. Last 30 days. ${accessibleLabel}`}
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
        className="min-w-0 flex-1 cursor-pointer rounded-control text-left disabled:opacity-100 active:transform-none active:opacity-100"
      >
        <BurnCheckSummary presentation={burnChecks} />
      </button>
      {presentation && estimate && presentation.estimate.tokenBurnBasisPoints != null && (
        <span
          className="pr-[var(--space-md)]"
          data-burn-check-flames
          onMouseEnter={onLeave}
          onMouseLeave={(event) => {
            if (
              summary.current &&
              event.relatedTarget instanceof Node &&
              summary.current.contains(event.relatedTarget) &&
              !event.currentTarget.contains(document.activeElement)
            )
              onPreview(measureAnchorRegion(summary.current))
          }}
          onFocus={(event) => {
            event.stopPropagation()
            onLeave()
          }}
        >
          <BurnCheckFlames basisPoints={presentation.estimate.tokenBurnBasisPoints} />
        </span>
      )}
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

export function ChecksPeek({
  presentation,
  pendingEvidence = 0,
}: {
  presentation: ChecksPresentation
  pendingEvidence?: number | undefined
}) {
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
        <div className="min-w-0">
          <span className={`block type-title-2 tabular-nums ${hero.tone}`}>
            {hasFindings && estimate.tokenBurnBasisPoints != null ? (
              <TextRoll text={hero.result} />
            ) : (
              hero.result
            )}
          </span>
          {(summaryStatus || pendingEvidence > 0) && (
            <div className="flex items-center gap-3 type-footnote text-label-secondary">
              {summaryStatus && <span>{summaryStatus}</span>}
              {pendingEvidence > 0 && (
                <p className="flex items-center gap-1.5 text-label-tertiary" role="status">
                  <LoaderCircle
                    size={12}
                    strokeWidth={2}
                    className="animate-spin"
                    aria-hidden="true"
                  />
                  {`${pendingEvidence} session${pendingEvidence === 1 ? "" : "s"} processing`}
                </p>
              )}
            </div>
          )}
        </div>
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
