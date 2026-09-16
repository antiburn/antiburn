import { ChevronRight } from "lucide-react"

import { cn } from "../../../lib/cn"
import { formatCost, formatSharePct } from "../../../lib/presentation/sessionAnalysis"
import {
  unusedContextDisplayRows,
  unusedContextTotalUsd,
  type UnusedContextKind,
  type UnusedContextRow,
} from "../../../lib/presentation/unusedContext"
import { Tooltip } from "../../presentation/Tooltip"
import {
  toggleUnusedContextExpanded,
  useUnusedContextExpanded,
} from "./unusedContextExpandedStore"

export interface UnusedContextProps {
  rows: UnusedContextRow[]
  /** The session's total cost, for the share-of-total column and header. Null when unknown. */
  sessionTotalUsd: number | null
}

/** Lower-case plural noun for a rollup row's kind, e.g. "6 built-in tools". */
const ROLLUP_KIND_LABEL: Record<UnusedContextKind, string> = {
  "Built-in tool": "built-in tools",
  "MCP server": "MCP servers",
  Skill: "skills",
}

/**
 * The Cost tab's informational list of loaded but unused resources.
 * Collapsed by default behind a header that names the section and, once at
 * least one row carries a price, shows the total replay cost and its share
 * of the session. Open, each row shows an idle resource's name, kind, replay
 * cost, and share; a kind with several small-cost rows collapses into one
 * rollup row, its names available on hover. Carries no verdict.
 */
export function UnusedContext({ rows, sessionTotalUsd }: UnusedContextProps) {
  const expanded = useUnusedContextExpanded()
  if (rows.length === 0) return null
  const totalUsd = unusedContextTotalUsd(rows)
  const displayRows = unusedContextDisplayRows(rows)
  const shareTotalUsd = sessionTotalUsd ?? 0

  return (
    <div className="grid w-full min-w-0 gap-y-1 rounded-control bg-surface-card/50 px-3 py-2">
      <button
        type="button"
        onClick={toggleUnusedContextExpanded}
        aria-expanded={expanded}
        className="flex w-full items-center justify-between gap-x-3 rounded-control px-1 py-1 text-left type-body cursor-pointer! transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover active:transform-none active:opacity-100"
      >
        <span className="flex min-w-0 items-center gap-x-1.5">
          <ChevronRight
            size={14}
            aria-hidden="true"
            className={cn(
              "shrink-0 transition-transform duration-[var(--duration-fast)] ease-out",
              expanded && "rotate-90",
            )}
          />
          <span className="truncate text-label">
            Unused skills, MCP servers, and built-in tools
          </span>
        </span>
        {totalUsd != null && (
          <span className="flex shrink-0 items-baseline gap-1 text-right tabular-nums">
            <span className="text-label">{formatCost(totalUsd)}</span>
            {sessionTotalUsd != null && sessionTotalUsd > 0 && (
              <>
                <span aria-hidden="true" className="text-label-tertiary">
                  ·
                </span>
                <span className="text-label-tertiary">
                  {formatSharePct(totalUsd, sessionTotalUsd)}
                </span>
              </>
            )}
          </span>
        )}
      </button>

      {expanded && (
        <div className="grid min-w-0 gap-y-1 gap-x-6 grid-cols-[1fr_auto_auto]">
          <p className="col-span-full type-callout text-label-tertiary mb-1">
            These items sat in every request&apos;s context this session but were never called.
            They had to be read from the cache every time. This is the cost associated with
            that.
          </p>

          {displayRows.map((row) =>
            row.type === "item" ? (
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
                <span className="text-right text-label-tertiary tabular-nums">
                  {row.costUsd != null ? formatSharePct(row.costUsd, shareTotalUsd) : "—"}
                </span>
              </div>
            ) : (
              <Tooltip key={`rollup-${row.kind}`} label={row.names.join(", ")} delayMs={150}>
                <div
                  tabIndex={0}
                  className="col-span-full grid grid-cols-subgrid rounded-control -mx-1 px-1 py-0.5 transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover focus-visible:bg-surface-hover type-callout"
                >
                  <span className="flex min-w-0 items-baseline gap-1.5">
                    <span className="truncate text-label">
                      {row.count} {ROLLUP_KIND_LABEL[row.kind]}
                    </span>
                  </span>
                  <span className="pr-1.5 text-right tabular-nums text-label">
                    {formatCost(row.costUsd)}
                  </span>
                  <span className="text-right text-label-tertiary tabular-nums">
                    {formatSharePct(row.costUsd, shareTotalUsd)}
                  </span>
                </div>
              </Tooltip>
            ),
          )}
        </div>
      )}
    </div>
  )
}
