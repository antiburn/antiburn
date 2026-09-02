import { cn } from "../../lib/cn"
import type { LiveUsageWindowPayload } from "../../lib/ipc"
import { liveMetricRows, liveWindowLabel } from "../../lib/presentation/liveUsage"

/**
 * What one limit window's history says about it — how fast, whether it lasts,
 * and how much was used today.
 *
 * The rows come from `liveMetricRows`, which keeps their order fixed and
 * gives an unavailable figure its reason rather than dropping the row.
 *
 * A description list carries no accessible name of its own. The explicit
 * group role is what tells a screen-reader user which window these rows
 * describe.
 */
export function LiveMetricRows({
  window,
  now,
  keys,
  className = "",
}: {
  window: LiveUsageWindowPayload
  /** Injected so the rendered output is a function of its inputs in tests. */
  now: number
  keys?: readonly string[]
  className?: string
}) {
  return (
    <dl
      role="group"
      aria-label={`${liveWindowLabel(window)} pace`}
      className={cn("space-y-1.5", className)}
    >
      {liveMetricRows(window, now)
        .filter((row) => keys == null || keys.includes(row.key))
        .map((row) => (
          <div key={row.key} className="flex items-baseline justify-between gap-3">
            <dt className="flex items-center gap-1.5 type-caption text-label-tertiary">
              {row.key === "pace" && <Gauge size={12} strokeWidth={1.75} aria-hidden="true" />}
              {row.key === "today" && (
                <Percent size={12} strokeWidth={1.75} aria-hidden="true" />
              )}
              {row.key === "runway" && (
                <Plane size={12} strokeWidth={1.75} aria-hidden="true" />
              )}
              {row.label}
            </dt>
            <dd className={cn("type-caption tabular-nums", row.toneClass)}>{row.value}</dd>
          </div>
        ))}
    </dl>
  )
}
import { Gauge, Percent, Plane } from "lucide-react"
