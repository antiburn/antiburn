import type {
  AllowanceUsageAccountPayload,
  AllowanceUtilizationPayload,
} from "../../../lib/providerUsageIpc"

import { HeroFigures, type HeroFigureCell } from "../../../components/ui/HeroFigures"
import { SegmentFigure } from "../../../components/ui/SegmentFigure"
import { Skeleton } from "../../../components/ui/Skeleton"
import { planLabel } from "../../../lib/presentation/liveUsage"
import { ChevronDown, LoaderCircle } from "lucide-react"

import { cn } from "../../../lib/cn"
import { useEntranceProps } from "./overviewEntrance"

export function OverviewAllowanceTotals({
  accounts: allAccounts,
  utilizationSpanDays,
  loading = false,
  error = false,
  collecting = false,
  selectedKey,
  onSelect,
}: {
  accounts: readonly AllowanceUsageAccountPayload[]
  utilizationSpanDays: number
  loading?: boolean
  error?: boolean
  /** True during the first run, while the providers have not yet reported
   *  any limit windows. Shows a collecting state, not "no history". */
  collecting?: boolean
  selectedKey?: string | null
  onSelect?: (key: string) => void
}) {
  const entranceProps = useEntranceProps("allowance-totals", "overview-figures-in", !loading)
  const accounts = allAccounts.filter(hasFigure)
  if (!loading && accounts.length === 0 && collecting && !error) {
    // Prototype: first-run collecting state. Copy and styling to be tuned.
    return (
      <section aria-label="Allowance" aria-busy="true" className="flex flex-col gap-1">
        <p role="status" className="flex items-center gap-2 type-body text-label-secondary">
          <LoaderCircle size={14} strokeWidth={2} aria-hidden="true" className="animate-spin" />
          Collecting usage from your providers…
        </p>
        <p className="type-caption text-label-tertiary">
          Subscription figures appear as your providers report your limits. This can take a few
          minutes.
        </p>
      </section>
    )
  }
  if (!loading && accounts.length === 0) {
    // This section carries no entrance class here, so its animation never
    // runs. The key stays free for the real figures to draw in later.
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
  const selectable = onSelect != null
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
        label: (
          <>
            <AccountLabel account={account} />
            {selectable && (
              <ChevronDown
                size={14}
                strokeWidth={2}
                aria-hidden="true"
                className={cn(
                  "ms-1 inline align-[-2px] text-label-tertiary transition-transform duration-fast",
                  `${account.provider}:${account.accountKey}` === selectedKey && "rotate-180",
                )}
              />
            )}
          </>
        ),
        figure: (
          <SegmentFigure>{`${Math.round(account.utilization.utilizationPercent)}%`}</SegmentFigure>
        ),
        caption: "Average subscription usage",
        tooltip: utilizationTooltip(account, utilizationSpanDays),
        ...(selectable
          ? {
              ...(selectedKey != null
                ? { selected: `${account.provider}:${account.accountKey}` === selectedKey }
                : {}),
              onSelect: () => onSelect(`${account.provider}:${account.accountKey}`),
            }
          : {}),
      }))
  return (
    <section aria-label="Allowance" aria-busy={loading} {...entranceProps}>
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
