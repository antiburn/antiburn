import { Sparkles } from "lucide-react"
import { useId, useState } from "react"

import type { AggregateWinPayload, BurnCheckDetectorId } from "../../../lib/insightsIpc"
import { CHECK_LABELS } from "../../../lib/presentation/checks"
import { DisclosureChevron } from "./BurnCheckTargetPresentation"

type SavingsGroup = {
  detector: BurnCheckDetectorId
  wins: AggregateWinPayload[]
  tokenSavings: number
  tokenWins: number
  costSavings: number
  costWins: number
  improvements: number
  improvementWins: number
}

function groupWins(wins: readonly AggregateWinPayload[]): SavingsGroup[] {
  const groups = new Map<BurnCheckDetectorId, SavingsGroup>()
  for (const win of wins) {
    let group = groups.get(win.detector)
    if (!group) {
      group = {
        detector: win.detector,
        wins: [],
        tokenSavings: 0,
        tokenWins: 0,
        costSavings: 0,
        costWins: 0,
        improvements: 0,
        improvementWins: 0,
      }
      groups.set(win.detector, group)
    }
    group.wins.push(win)
    if (win.savings.tokenSavings != null) {
      group.tokenSavings += win.savings.tokenSavings
      group.tokenWins += 1
    }
    if (win.savings.apiEquivalentCostAvoidedUsd != null) {
      group.costSavings += win.savings.apiEquivalentCostAvoidedUsd
      group.costWins += 1
    }
    if (win.savings.improvementCount != null) {
      group.improvements += win.savings.improvementCount
      group.improvementWins += 1
    }
  }
  return [...groups.values()]
}

function metricCoverage(known: number, total: number): string {
  return known === total ? "" : ` from ${known} of ${total} wins`
}

function tokenTotal(value: number): string {
  return `~${value.toLocaleString()} tokens`
}

function costTotal(value: number): string {
  return `${value < 0 ? "-" : ""}~$${Math.abs(value).toFixed(2)}`
}

export function BurnChecksSavings({ wins }: { wins: readonly AggregateWinPayload[] }) {
  const [open, setOpen] = useState(true)
  const bodyId = useId()
  const supported = wins.filter(
    (win) =>
      win.savings.tokenSavings != null ||
      win.savings.apiEquivalentCostAvoidedUsd != null ||
      win.savings.improvementCount != null,
  )
  if (supported.length === 0) return null
  const groups = groupWins(supported)
  const tokenSavings = supported.reduce((sum, win) => sum + (win.savings.tokenSavings ?? 0), 0)
  const costSavings = supported.reduce(
    (sum, win) => sum + (win.savings.apiEquivalentCostAvoidedUsd ?? 0),
    0,
  )
  const improvements = supported.reduce(
    (sum, win) => sum + (win.savings.improvementCount ?? 0),
    0,
  )
  const tokenWins = supported.filter((win) => win.savings.tokenSavings != null).length
  const costWins = supported.filter(
    (win) => win.savings.apiEquivalentCostAvoidedUsd != null,
  ).length
  const improvementWins = supported.filter((win) => win.savings.improvementCount != null).length
  const paired =
    tokenWins > 0 &&
    tokenWins === costWins &&
    supported.every(
      (win) =>
        (win.savings.tokenSavings == null) ===
        (win.savings.apiEquivalentCostAvoidedUsd == null),
    )
  return (
    <section aria-labelledby="burn-checks-savings" className="mt-4">
      <div className="overflow-hidden rounded-control border border-separator bg-surface-card/50">
        <button
          type="button"
          aria-expanded={open}
          aria-controls={bodyId}
          onClick={() => setOpen((value) => !value)}
          className="flex w-full flex-wrap items-center gap-4 px-4 py-3.5 text-left hover:bg-surface-hover active:transform-none active:opacity-100"
        >
          <span className="flex h-8 w-8 items-center justify-center rounded-control bg-system-green/10 text-system-green">
            <Sparkles size={16} aria-hidden="true" />
          </span>
          <div className="min-w-0 flex-1">
            <h2 id="burn-checks-savings" className="type-headline text-label">
              Your savings
            </h2>
            <p className="type-footnote text-label-tertiary">
              {improvementWins > 0
                ? `${improvements.toLocaleString()} improvement${improvements === 1 ? "" : "s"} across ${groups.length} check${groups.length === 1 ? "" : "s"}`
                : `${supported.length} verified ${supported.length === 1 ? "win" : "wins"} across ${groups.length} check${groups.length === 1 ? "" : "s"}`}
            </p>
          </div>
          <div className="text-right type-footnote tabular-nums text-label-secondary">
            {paired ? (
              <p className="type-title-3 text-label">
                {tokenTotal(tokenSavings)} · {costTotal(costSavings)} saved
                {metricCoverage(tokenWins, supported.length)}
              </p>
            ) : (
              <>
                {tokenWins > 0 && (
                  <p className={costWins === 0 ? "type-title-3 text-label" : undefined}>
                    {tokenTotal(tokenSavings)}
                    {metricCoverage(tokenWins, supported.length)}
                  </p>
                )}
                {costWins > 0 && (
                  <p className={tokenWins === 0 ? "type-title-3 text-label" : undefined}>
                    {costTotal(costSavings)} saved
                    {metricCoverage(costWins, supported.length)}
                  </p>
                )}
                {tokenWins === 0 && costWins === 0 && improvementWins > 0 && (
                  <p className="type-title-3 text-label">
                    {improvements.toLocaleString()} improvement
                    {improvements === 1 ? "" : "s"}
                  </p>
                )}
              </>
            )}
            {improvementWins > 0 && improvementWins < supported.length && (
              <p>
                Count known for {improvementWins} of {supported.length} wins
              </p>
            )}
          </div>
          <DisclosureChevron open={open} />
        </button>
        <div id={bodyId} hidden={!open} className="border-t border-separator px-4">
          {groups.map((group) => (
            <div
              key={group.detector}
              className="flex flex-wrap items-start justify-between gap-x-4 gap-y-1 border-b border-separator py-2.5 last:border-b-0"
            >
              <div>
                <p className="type-callout font-medium! text-label">
                  {CHECK_LABELS[group.detector]}
                </p>
                <p className="type-footnote text-label-tertiary">
                  {group.wins.length} verified {group.wins.length === 1 ? "win" : "wins"}
                </p>
              </div>
              <div className="text-right type-footnote tabular-nums text-label-secondary">
                {group.tokenWins > 0 && (
                  <p>
                    {tokenTotal(group.tokenSavings)}
                    {metricCoverage(group.tokenWins, group.wins.length)}
                  </p>
                )}
                {group.costWins > 0 && (
                  <p>
                    {costTotal(group.costSavings)} saved
                    {metricCoverage(group.costWins, group.wins.length)}
                  </p>
                )}
                {group.improvementWins > 0 && (
                  <p>
                    {group.improvements.toLocaleString()} improvement
                    {group.improvements === 1 ? "" : "s"}
                    {metricCoverage(group.improvementWins, group.wins.length)}
                  </p>
                )}
              </div>
            </div>
          ))}
        </div>
      </div>
    </section>
  )
}
