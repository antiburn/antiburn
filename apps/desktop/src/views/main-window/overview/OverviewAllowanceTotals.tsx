import type {
  AllowanceUsageAccountPayload,
  AllowanceUtilizationPayload,
} from "../../../lib/providerUsageIpc"

import { HeroFigures, type HeroFigureCell } from "../../../components/ui/HeroFigures"
import { SegmentFigure } from "../../../components/ui/SegmentFigure"
import { Skeleton } from "../../../components/ui/Skeleton"
import { planLabel } from "../../../lib/presentation/liveUsage"

export function OverviewAllowanceTotals({
  accounts: allAccounts,
  utilizationSpanDays,
  loading = false,
  error = false,
}: {
  accounts: readonly AllowanceUsageAccountPayload[]
  utilizationSpanDays: number
  loading?: boolean
  error?: boolean
}) {
  const accounts = allAccounts.filter(hasFigure)
  if (!loading && accounts.length === 0) {
    return (
      <section aria-label="Allowance">
        <p role={error ? "alert" : undefined} className="type-body text-label-secondary">
          {error
            ? "antiburn cannot read the allowance figures now. They appear here after the next read."
            : "antiburn has no allowance history yet. Figures appear when a provider reports usage or local sessions provide an estimate."}
        </p>
      </section>
    )
  }

  // Each placeholder wraps a sample of the line it stands in for, so it takes
  // that line's own height. Fixed heights here were shorter than the real
  // figures, and everything below the section shifted down as they landed.
  const cells: HeroFigureCell[] = loading
    ? [
        {
          key: "loading",
          label: <Skeleton className="w-24">Provider</Skeleton>,
          figure: (
            <Skeleton className="w-28">
              <SegmentFigure>00%</SegmentFigure>
            </Skeleton>
          ),
          caption: <Skeleton className="w-36 max-w-full">Average subscription usage</Skeleton>,
        },
      ]
    : accounts.map((account) => ({
        key: `${account.provider}:${account.accountKey}`,
        label: <AccountLabel account={account} />,
        figure: (
          <SegmentFigure>{`${Math.round(account.utilization.utilizationPercent)}%`}</SegmentFigure>
        ),
        caption: "Average subscription usage",
        tooltip: utilizationTooltip(account, utilizationSpanDays),
      }))
  return (
    <section
      aria-label="Allowance"
      aria-busy={loading}
      className={loading ? undefined : "overview-figures-in"}
    >
      <HeroFigures cells={cells} />
    </section>
  )
}

type FiguredAccount = AllowanceUsageAccountPayload & {
  utilization: AllowanceUtilizationPayload
}

function hasFigure(account: AllowanceUsageAccountPayload): account is FiguredAccount {
  return account.utilization != null
}

function AccountLabel({ account }: { account: FiguredAccount }) {
  const plan = planLabel(account.provider, account.plan)
  return (
    <>
      {account.displayName}
      {plan && <span className="text-label-tertiary"> · {plan}</span>}
    </>
  )
}

function utilizationTooltip(account: FiguredAccount, spanDays: number): string {
  const { weeklyWindowCount, shortWindowCount, modelWindowCount } = account.utilization
  const clauses = [
    countClause(weeklyWindowCount, "weekly window", "weekly windows"),
    countClause(shortWindowCount, "5-hour window", "5-hour windows"),
    countClause(modelWindowCount, "specific model window", "specific model windows"),
  ].filter((clause): clause is string => clause != null)
  return `We estimate how much you used your plan in the last ${spanDays} days, covering ${joinClauses(clauses)}.`
}

function countClause(count: number, singular: string, plural: string): string | null {
  if (count === 0) return null
  return `${count} ${count === 1 ? singular : plural}`
}

function joinClauses(clauses: readonly string[]): string {
  if (clauses.length <= 1) return clauses.join("")
  if (clauses.length === 2) return clauses.join(" and ")
  return `${clauses.slice(0, -1).join(", ")} and ${clauses[clauses.length - 1]}`
}
