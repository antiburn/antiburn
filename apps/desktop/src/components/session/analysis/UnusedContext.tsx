import { formatCost } from "../../../lib/presentation/sessionAnalysis"
import {
  unusedContextTotalUsd,
  type UnusedContextRow,
} from "../../../lib/presentation/unusedContext"

export interface UnusedContextProps {
  rows: UnusedContextRow[]
}

/**
 * The Cost tab's informational "Loaded but not used" list. One row shows
 * an idle resource's name, kind, and replay cost, laid out like
 * `CostBreakdown`'s component rows. Carries no verdict.
 */
export function UnusedContext({ rows }: UnusedContextProps) {
  if (rows.length === 0) return null
  const totalUsd = unusedContextTotalUsd(rows)
  const pricedCount = rows.filter((row) => row.costUsd != null).length

  return (
    <div className="grid min-w-0 gap-y-1 w-full max-w-[640px] gap-x-6 justify-self-end justify-end grid-cols-[1fr_auto]">
      <p className="col-span-full type-callout text-label-tertiary mb-1">
        Each item sat in every request&apos;s context this session and was never called. The
        figure is what this session paid to replay it from the cache.
      </p>

      {rows.map((row) => (
        <div
          key={`${row.kind}-${row.name}`}
          className="col-span-full grid grid-cols-subgrid rounded-control -mx-1 px-1 py-0.5 transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover type-callout"
        >
          <span className="flex min-w-0 items-baseline gap-1.5">
            <span className="truncate text-label">{row.name}</span>
            <span className="truncate text-label-secondary">{row.kind}</span>
          </span>
          <span className="pr-1.5 text-right tabular-nums">
            {row.costUsd != null ? (
              <span className="text-label">{formatCost(row.costUsd)}</span>
            ) : (
              <span className="text-label-tertiary">Not priced</span>
            )}
          </span>
        </div>
      ))}

      {pricedCount >= 2 && totalUsd != null && (
        <div className="col-span-full grid grid-cols-subgrid rounded-control -mx-1 mt-1 border-t border-separator px-1 pt-1.5 type-callout font-semibold!">
          <span className="text-label">Total</span>
          <span className="pr-1.5 text-right tabular-nums text-label">
            {formatCost(totalUsd)}
          </span>
        </div>
      )}
    </div>
  )
}
