import { Info, Sparkles } from "lucide-react"
import { useId, useState } from "react"

import { Tooltip } from "../../../components/presentation/Tooltip"
import type {
  AggregateWinPayload,
  BurnCheckDetectorId,
  BurnCheckEstimatedValuePayload,
} from "../../../lib/insightsIpc"
import { CHECK_LABELS } from "../../../lib/presentation/checks"
import { DisclosureChevron } from "./BurnCheckTargetPresentation"

const ESTIMATED_SAVINGS_TOOLTIP =
  "Pre-remediation opportunity estimated from evidence observed before the fix. Actual results can vary."
const CONFIRMED_SAVINGS_TOOLTIP =
  "Savings observed across sessions that passed after remediation. This is still an estimate and may not match provider billing exactly."

type SavingsGroup = {
  detector: BurnCheckDetectorId
  wins: AggregateWinPayload[]
}

function groupWins(wins: readonly AggregateWinPayload[]): SavingsGroup[] {
  const groups = new Map<BurnCheckDetectorId, SavingsGroup>()
  for (const win of wins) {
    const group = groups.get(win.detector) ?? { detector: win.detector, wins: [] }
    group.wins.push(win)
    groups.set(win.detector, group)
  }
  return [...groups.values()]
}

function metricCoverage(known: number, total: number): string {
  return known === total ? "" : ` from ${known} of ${total} verified cycles`
}

function cycleKey(
  value: Pick<AggregateWinPayload, "detector" | "findingId" | "remediationCycleId">,
): string {
  return JSON.stringify([value.detector, value.findingId, value.remediationCycleId])
}

function costTotal(value: number): string {
  return `${value < 0 ? "-" : ""}~$${Math.abs(value).toFixed(2)}`
}

function opportunityLabel({ value, unit }: BurnCheckEstimatedValuePayload): string {
  const rounded = Math.round(value).toLocaleString()
  if (unit === "apiEquivalentUsd") return costTotal(value)
  if (unit === "improvements") return `~${rounded} improvement${value === 1 ? "" : "s"}`
  if (unit === "assumedOutputTokens") return `~${rounded} output tokens`
  if (unit === "cacheClassTokens") return `~${rounded} cache tokens`
  return `~${rounded} input tokens`
}

function SavingsLabel({ label, tooltip }: { label: string; tooltip: string }) {
  return (
    <div className="flex items-center gap-1">
      <p className="type-footnote font-medium! text-label-secondary">{label}</p>
      <Tooltip label={tooltip}>
        <button
          type="button"
          aria-label={`About ${label.toLowerCase()}`}
          className="rounded-control text-label-tertiary hover:text-label"
        >
          <Info size={14} aria-hidden="true" />
        </button>
      </Tooltip>
    </div>
  )
}

function EstimatedValue({ wins }: { wins: readonly AggregateWinPayload[] }) {
  const targets = [
    ...new Map(
      wins.map((win) => [JSON.stringify([win.detector, win.findingId]), win]),
    ).values(),
  ]
  const opportunities = targets.flatMap((win) =>
    win.display.estimatedOpportunity ? [win.display.estimatedOpportunity] : [],
  )
  if (opportunities.length === 0) {
    return <p className="type-callout text-label-tertiary">Unavailable for this check.</p>
  }
  const totals = new Map<BurnCheckEstimatedValuePayload["unit"], number>()
  for (const opportunity of opportunities) {
    totals.set(opportunity.unit, (totals.get(opportunity.unit) ?? 0) + opportunity.value)
  }
  return (
    <div className="type-callout tabular-nums text-label">
      {[...totals.entries()].map(([unit, value]) => (
        <p key={unit}>{opportunityLabel({ unit, value })} projected</p>
      ))}
    </div>
  )
}

function ConfirmedValue({ wins }: { wins: readonly AggregateWinPayload[] }) {
  const knownCosts = wins.flatMap((win) =>
    win.savings.status.status === "known"
      ? [win.savings.status.apiEquivalentCostAvoidedUsd]
      : [],
  )
  const costSavings = knownCosts.reduce((sum, value) => sum + value, 0)
  const costWins = knownCosts.length
  const pending = wins.some((win) => win.savings.status.status === "pending")
  const unavailable = wins.some((win) => win.savings.status.status === "unavailable")
  const unknown = wins.some((win) => win.savings.status.status === "unknown")
  const messages: string[] = []
  if (pending) messages.push("Pending recent usage.")
  if (unavailable) messages.push("Unavailable for this check.")
  if (unknown) messages.push("Savings are unknown for this check.")
  if (costWins === 0 && messages.length === 0) messages.push("Unavailable for this check.")
  return (
    <div className="type-callout tabular-nums text-label">
      {costWins > 0 && (
        <p>
          {costTotal(costSavings)} confirmed{metricCoverage(costWins, wins.length)}
        </p>
      )}
      {messages.map((message) => (
        <p key={message}>{message}</p>
      ))}
    </div>
  )
}

export function BurnChecksSavings({
  wins,
  passedDetectors,
}: {
  wins: readonly AggregateWinPayload[]
  passedDetectors: ReadonlySet<BurnCheckDetectorId>
}) {
  const [open, setOpen] = useState(false)
  const bodyId = useId()
  const supported = [
    ...new Map(
      wins
        .filter((win) => passedDetectors.has(win.detector))
        .map((win) => [cycleKey(win), win]),
    ).values(),
  ]
  if (supported.length === 0) return null
  const groups = groupWins(supported)
  return (
    <section aria-label="Savings" className="mt-4">
      <div className="overflow-hidden rounded-control bg-surface-card/50">
        <div className="flex flex-wrap items-start gap-4 px-4 py-3.5">
          <span className="flex h-8 w-8 items-center justify-center rounded-control bg-token-in/10 text-token-in">
            <Sparkles size={16} aria-hidden="true" />
          </span>
          <div className="min-w-0 flex-1">
            <h2 id="burn-checks-savings" className="type-headline text-label">
              Savings
            </h2>
            <p className="type-callout text-label-tertiary">
              {supported.length} verified remediation{" "}
              {supported.length === 1 ? "cycle" : "cycles"}
            </p>
          </div>
          <button
            type="button"
            aria-expanded={open}
            aria-controls={bodyId}
            onClick={() => setOpen((value) => !value)}
            className="burn-check-action type-callout"
          >
            Details
            <DisclosureChevron open={open} />
          </button>
        </div>
        <div className="grid gap-3 border-t border-separator px-4 py-3 sm:grid-cols-2">
          <div>
            <SavingsLabel label="Estimated savings" tooltip={ESTIMATED_SAVINGS_TOOLTIP} />
            <EstimatedValue wins={supported} />
          </div>
          <div>
            <SavingsLabel label="Confirmed savings" tooltip={CONFIRMED_SAVINGS_TOOLTIP} />
            <ConfirmedValue wins={supported} />
          </div>
        </div>
        <div id={bodyId} hidden={!open} className="border-t border-separator px-4">
          {groups.map((group) => (
            <div
              key={group.detector}
              className="border-b border-separator py-3 last:border-b-0"
            >
              <p className="type-callout font-medium! text-label">
                {CHECK_LABELS[group.detector]}
              </p>
              <div className="mt-2 grid gap-3 sm:grid-cols-2">
                <div>
                  <SavingsLabel label="Estimated savings" tooltip={ESTIMATED_SAVINGS_TOOLTIP} />
                  <EstimatedValue wins={group.wins} />
                </div>
                <div>
                  <SavingsLabel label="Confirmed savings" tooltip={CONFIRMED_SAVINGS_TOOLTIP} />
                  <ConfirmedValue wins={group.wins} />
                </div>
              </div>
            </div>
          ))}
        </div>
      </div>
    </section>
  )
}
