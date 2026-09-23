import { BellOff, CircleCheck, Hourglass, Wrench, type LucideIcon } from "lucide-react"
import { useCallback, type CSSProperties } from "react"

import type { BurnCheckTargetPayload, ChecksCategoryPayload } from "../../../lib/insightsIpc"
import {
  formatApiEquivalentUsd,
  formatTokenBurnPercent,
} from "../../../lib/presentation/checks"
import { formatTokensShort } from "../../../lib/presentation/sessionAnalysis"
import { checkRowPresentation } from "../../checks/checkUi"
import type { BurnChecksSession, BurnChecksSnapshot } from "../BurnChecksSession"
import { watchStatus } from "../burn-checks/BurnCheckTargetPresentation"
import { countUp } from "./countUp"
import { EnhanceCard, useChecks, Waiting } from "./EnhanceSteps"
import { startSmoke } from "./enhanceSmoke"
import { isAppliedFix, predictSavings, projectSavings, SAVINGS_MONTHS } from "./enhanceState"

/** Asks for a check's targets while it is mounted. It shows nothing. */
function TargetTracker({
  detector,
  session,
}: {
  detector: ChecksCategoryPayload["id"]
  session: BurnChecksSession
}) {
  const track = useCallback(
    (node: HTMLElement | null) => session.setTargetsVisible(detector, node !== null),
    [detector, session],
  )
  return <span ref={track} hidden />
}

function watchLine(check: ChecksCategoryPayload, watched: readonly BurnCheckTargetPayload[]) {
  const statuses = watched.map((target) => target.watch?.verification.status)
  if (statuses.includes("fixed")) return { text: "Confirmed. This fix works.", done: true }
  if (
    statuses.some(
      (status) => status === "verificationUnavailable" || status === "recoveryNeeded",
    )
  )
    return { text: "Applied. We can't confirm this one automatically.", done: false }
  const line = watched.map(watchStatus).find((value) => value != null)
  if (line) return { text: line, done: false }
  return check.lifecycle === "awaitingVerification"
    ? { text: "Waiting for a later complete session.", done: false }
    : null
}

export function WatchStep({
  session,
  state,
}: {
  session: BurnChecksSession
  state: BurnChecksSnapshot
}) {
  const checks = useChecks(state)
  if (!checks) return <Waiting>Running burn checks…</Waiting>
  const { presentation } = checks
  const candidates = [...(presentation.awaiting ?? []), ...presentation.failures]
  const rows = candidates.flatMap((check) => {
    const watched = (state.targets[check.id]?.data?.targets ?? []).filter(
      (target) => target.watch != null,
    )
    const line = watchLine(check, watched)
    return line ? [{ check, line }] : []
  })
  return (
    <div className="flex flex-col gap-(--space-sm)">
      {candidates.map((check) => (
        <TargetTracker key={check.id} detector={check.id} session={session} />
      ))}
      {rows.length === 0 ? (
        <p className="type-body text-label-secondary">
          Nothing to watch yet. Fixes you apply on the Fix step show up here.
        </p>
      ) : (
        <ul aria-label="Fixes being watched" className="flex flex-col gap-(--space-sm)">
          {rows.map(({ check, line }) => {
            const Icon = line.done ? CircleCheck : Hourglass
            return (
              <EnhanceCard
                key={check.id}
                tone={line.done ? "pass" : "wait"}
                icon={<Icon size={22} strokeWidth={2} />}
                title={checkRowPresentation(check).label}
                detail={line.text}
              />
            )
          })}
        </ul>
      )}
    </div>
  )
}

const USD_NOTE =
  "What these tokens would cost at the provider's public API prices. On a subscription you don't pay this directly, but it frees up your limits."

/** Draws LED smoke on the canvas while it is mounted. */
function mountSmoke(canvas: HTMLCanvasElement | null) {
  if (!canvas) return
  return startSmoke(canvas) ?? undefined
}

const whole = (value: number) => String(Math.round(value))

/** A figure that counts up from zero when it arrives. */
function Figure({
  value,
  format,
  unit,
  note,
}: {
  value: number
  format: (value: number) => string
  unit: string
  note?: string
}) {
  const count = useCallback(
    (node: HTMLSpanElement | null) => (node ? countUp(node, value, format) : undefined),
    [value, format],
  )
  return (
    <div className="flex flex-col" title={note}>
      <span ref={count} className="type-hero-figure font-semibold tabular-nums">
        {format(value)}
      </span>
      <span className="type-caption opacity-80">{unit}</span>
    </div>
  )
}

function Tile({
  value,
  label,
  tone,
  Icon,
  index,
}: {
  value: number
  label: string
  tone: "brand" | "pass" | "snooze"
  Icon: LucideIcon
  index: number
}) {
  const count = useCallback(
    (node: HTMLSpanElement | null) => (node ? countUp(node, value, whole) : undefined),
    [value],
  )
  return (
    <div
      data-tone={tone}
      style={{ "--enhance-stat-index": index } as CSSProperties}
      className="enhance-stat flex items-start justify-between gap-(--space-md) rounded-(--radius-popover) p-(--space-lg)"
    >
      <div className="flex flex-col">
        <span ref={count} className="type-title-2 font-semibold tabular-nums">
          {value}
        </span>
        <span className="type-caption opacity-80">{label}</span>
      </div>
      <Icon aria-hidden="true" size={22} strokeWidth={2} className="opacity-80" />
    </div>
  )
}

export function DoneStep({
  session,
  state,
}: {
  session: BurnChecksSession
  state: BurnChecksSnapshot
}) {
  const checks = useChecks(state)
  if (!checks) return <Waiting>Running burn checks…</Waiting>
  const { presentation } = checks
  const tracked = [...presentation.failures, ...(presentation.awaiting ?? [])]
  const loading = tracked.some((check) => !state.targets[check.id]?.data)
  const byCheck = tracked.map((check) => {
    const targets = state.targets[check.id]?.data?.targets ?? []
    return {
      check,
      applied: targets.filter(isAppliedFix),
      open: targets.filter((target) => target.watch == null),
    }
  })
  const applied = byCheck.flatMap((row) => row.applied)
  // With no applied fix, the page shows what the open fixes can save.
  const counted = applied.length > 0 ? "applied" : "open"
  const savings = projectSavings(byCheck.flatMap((row) => row[counted]))
  // With no reviewed estimate, predict from the burn of the fixed checks.
  const prediction =
    savings.estimated === 0 && state.report
      ? predictSavings(
          state.report,
          new Set(byCheck.filter((row) => row[counted].length > 0).map((row) => row.check.id)),
        )
      : null
  const breakdown = byCheck
    .map((row) => ({ check: row.check, savings: projectSavings(row[counted]) }))
    .filter((row) => row.savings.estimated > 0)
  return (
    <div className="flex flex-1 flex-col gap-(--space-xl)">
      {tracked.map((check) => (
        <TargetTracker key={check.id} detector={check.id} session={session} />
      ))}
      {loading ? (
        <Waiting>Adding up your savings…</Waiting>
      ) : (
        <section
          aria-label="Projected savings"
          className="enhance-done-hero flex flex-1 items-center rounded-(--radius-popover) px-(--space-2xl) py-(--space-2xl)"
        >
          <canvas ref={mountSmoke} aria-hidden="true" className="enhance-done-smoke" />
          {prediction ? (
            <div className="flex min-w-0 flex-col gap-(--space-md)">
              <p className="type-body">
                {counted === "applied"
                  ? "Your fixes could save up to"
                  : "Apply the open fixes to save up to"}
              </p>
              <Figure value={prediction.tokens} format={formatTokensShort} unit="tokens" />
              <p className="type-callout opacity-80">
                Over the next {SAVINGS_MONTHS} months, at your last 30 days&apos; pace. These
                checks burned {formatTokenBurnPercent(prediction.basisPoints)} of your tokens.
                The real saving depends on how much of each check your fixes cover.
              </p>
            </div>
          ) : savings.estimated === 0 ? (
            <div className="flex min-w-0 flex-col gap-(--space-xs)">
              <span className="type-title-2 font-semibold tabular-nums">
                {applied.length} {applied.length === 1 ? "fix" : "fixes"} applied
              </span>
              <p className="type-body opacity-80">
                No savings estimate yet. Estimates show once a fix has enough sessions behind
                it.
              </p>
            </div>
          ) : (
            <div className="flex min-w-0 flex-col gap-(--space-md)">
              <p className="type-body">
                {counted === "applied"
                  ? "Your fixes save about"
                  : "Apply the open fixes to save about"}
              </p>
              <div className="flex flex-wrap items-end gap-(--space-2xl)">
                {savings.tokens > 0 && (
                  <Figure value={savings.tokens} format={formatTokensShort} unit="tokens" />
                )}
                {savings.usd > 0 && (
                  <Figure
                    value={savings.usd}
                    format={formatApiEquivalentUsd}
                    unit="API-equivalent USD"
                    note={USD_NOTE}
                  />
                )}
              </div>
              <p className="type-callout opacity-80">
                Over the next {SAVINGS_MONTHS} months, at your last 30 days&apos; pace.
                {savings.usd > 0 && ` ${USD_NOTE}`}
              </p>
            </div>
          )}
        </section>
      )}
      <div className="grid grid-cols-3 gap-(--space-md)">
        <Tile
          value={applied.length}
          label="Fixes applied"
          tone="brand"
          Icon={Wrench}
          index={0}
        />
        <Tile
          value={presentation.awaiting?.length ?? 0}
          label="Being watched"
          tone="pass"
          Icon={Hourglass}
          index={1}
        />
        <Tile
          value={presentation.snoozed.length}
          label="Snoozed, not counted"
          tone="snooze"
          Icon={BellOff}
          index={2}
        />
      </div>
      {!loading && breakdown.length > 0 && (
        <ul aria-label="Savings by fix" className="flex flex-col gap-(--space-sm)">
          {breakdown.map(({ check, savings: row }) => {
            const presentation = checkRowPresentation(check)
            return (
              <EnhanceCard
                key={check.id}
                tone="pass"
                icon={<presentation.Icon size={22} strokeWidth={2} />}
                title={presentation.label}
                detail={[
                  row.tokens > 0 && `${formatTokensShort(row.tokens)} tokens`,
                  row.usd > 0 && formatApiEquivalentUsd(row.usd),
                ]
                  .filter(Boolean)
                  .join(" · ")}
              />
            )
          })}
        </ul>
      )}
    </div>
  )
}
