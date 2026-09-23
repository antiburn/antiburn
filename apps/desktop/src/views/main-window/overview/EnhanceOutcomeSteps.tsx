import { CircleCheck, Hourglass } from "lucide-react"
import { useCallback } from "react"

import type { BurnCheckTargetPayload, ChecksCategoryPayload } from "../../../lib/insightsIpc"
import { formatApiEquivalentUsd } from "../../../lib/presentation/checks"
import { formatTokensShort } from "../../../lib/presentation/sessionAnalysis"
import { checkRowPresentation } from "../../checks/checkUi"
import type { BurnChecksSession, BurnChecksSnapshot } from "../BurnChecksSession"
import { watchStatus } from "../burn-checks/BurnCheckTargetPresentation"
import { EnhanceCard, useChecks, Waiting } from "./EnhanceSteps"
import { isAppliedFix, projectSavings, SAVINGS_MONTHS } from "./enhanceState"

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
                icon={<Icon size={28} />}
                iconClassName={
                  line.done
                    ? "bg-system-green/10 text-system-green"
                    : "bg-system-orange/10 text-system-orange"
                }
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

function Figure({ value, unit, note }: { value: string; unit: string; note?: string }) {
  return (
    <div className="flex flex-col" title={note}>
      <span className="type-hero-figure tabular-nums text-label">{value}</span>
      <span className="type-caption text-label-secondary">{unit}</span>
    </div>
  )
}

function Tile({ value, label }: { value: number; label: string }) {
  return (
    <div className="flex flex-col rounded-control bg-surface-card p-(--space-md)">
      <span className="type-title-2 tabular-nums text-label">{value}</span>
      <span className="type-caption text-label-secondary">{label}</span>
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
  const breakdown = byCheck
    .map((row) => ({ check: row.check, savings: projectSavings(row[counted]) }))
    .filter((row) => row.savings.estimated > 0)
  return (
    <div className="flex max-w-3xl flex-col gap-(--space-xl)">
      {tracked.map((check) => (
        <TargetTracker key={check.id} detector={check.id} session={session} />
      ))}
      {loading ? (
        <Waiting>Adding up your savings…</Waiting>
      ) : savings.estimated === 0 ? (
        <p className="type-body text-label-secondary">
          No savings estimate yet. Estimates show once a fix has enough sessions behind it.
        </p>
      ) : (
        <section aria-label="Projected savings" className="flex flex-col gap-(--space-md)">
          <p className="type-body text-label">
            {counted === "applied"
              ? "Your fixes save about"
              : "Apply the open fixes to save about"}
          </p>
          <div className="flex flex-wrap items-end gap-(--space-2xl)">
            {savings.tokens > 0 && (
              <Figure value={formatTokensShort(savings.tokens)} unit="tokens" />
            )}
            {savings.usd > 0 && (
              <Figure
                value={formatApiEquivalentUsd(savings.usd)}
                unit="API-equivalent USD"
                note={USD_NOTE}
              />
            )}
          </div>
          <p className="type-callout text-label-secondary">
            Over the next {SAVINGS_MONTHS} months, at your last 30 days&apos; pace.
            {savings.usd > 0 && ` ${USD_NOTE}`}
          </p>
        </section>
      )}
      <div className="grid grid-cols-3 gap-(--space-sm)">
        <Tile value={applied.length} label="Fixes applied" />
        <Tile value={presentation.awaiting?.length ?? 0} label="Being watched" />
        <Tile value={presentation.snoozed.length} label="Snoozed, not counted" />
      </div>
      {!loading && breakdown.length > 0 && (
        <ul aria-label="Savings by fix" className="flex flex-col divide-y divide-separator">
          {breakdown.map(({ check, savings: row }) => (
            <li
              key={check.id}
              className="flex items-center justify-between gap-(--space-md) py-(--space-sm)"
            >
              <span className="type-callout text-label">
                {checkRowPresentation(check).label}
              </span>
              <span className="type-callout tabular-nums text-label-secondary">
                {[
                  row.tokens > 0 && `${formatTokensShort(row.tokens)} tokens`,
                  row.usd > 0 && formatApiEquivalentUsd(row.usd),
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
