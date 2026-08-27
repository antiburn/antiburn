import { cn } from "../../lib/cn"
import type { LiveProviderUsagePayload } from "../../lib/ipc"
import {
  liveResetLabel,
  liveWindowElapsed,
  liveWindowLabel,
  liveWindowValueLabel,
  liveWindows,
} from "../../lib/presentation/liveUsage"

/**
 * The provider's own limits, one row each.
 *
 * Unlike the estimate surface's bars — whose only denominator is the reader's
 * own month — these really are meters against an allowance, because the
 * provider stated the allowance. That difference is the whole reason this is
 * a separate component rather than a prop on the other one: the two look
 * similar and mean opposite things, and a shared component would be an
 * invitation to render one with the other's data.
 *
 * Each bar carries a second mark: how far through the window's own period the
 * clock has travelled. It is the most useful thing here. 60% used at 30%
 * elapsed and 60% used at 90% elapsed are opposite situations, and the fill
 * alone cannot tell them apart. When the provider did not state both ends of
 * the window there is no marker, rather than one drawn from an assumption.
 */
export function LiveUsageWindowRows({
  provider,
  now,
  className = "",
}: {
  provider: LiveProviderUsagePayload
  /** Injected so the rendered output is a function of its inputs in tests. */
  now: number
  className?: string
}) {
  const windows = liveWindows(provider)
  if (windows.length === 0) return null

  return (
    <div className={cn("space-y-1.5", className)}>
      {windows.map((window) => {
        const used = window.usedPercent
        const elapsed = liveWindowElapsed(window, now)
        return (
          <div key={window.id} className="group">
            <div className="flex items-baseline justify-between gap-x-2">
              <span className="type-footnote text-label-secondary">
                {liveWindowLabel(window)}
                <span className="hidden group-hover:inline text-label-tertiary">
                  {" "}
                  ({liveResetLabel(window, now)})
                </span>
              </span>
              <span className="type-footnote tabular-nums text-label text-right">
                {liveWindowValueLabel(window)}
              </span>
            </div>

            <div className="mt-0.5 h-[6px] py-[1.5px] relative">
              <div
                role="progressbar"
                aria-label={liveWindowLabel(window)}
                aria-valuemin={0}
                aria-valuemax={100}
                // Absent rather than zero when unknown: a progressbar with no
                // value is announced as indeterminate, which is the truth.
                aria-valuenow={used ?? undefined}
                aria-valuetext={
                  used == null
                    ? "Usage unknown"
                    : elapsed == null
                      ? `${Math.round(used)}% used`
                      : `${Math.round(used)}% used; ${Math.round(elapsed * 100)}% of the period elapsed`
                }
                className="h-full w-full bg-(--color-separator) rounded-full overflow-hidden"
              >
                {used != null && (
                  <div
                    className="h-full"
                    data-testid={`live-usage-fill-${window.id}`}
                    style={{
                      width: `${Math.min(100, Math.max(0, used))}%`,
                      // accent-fill, not accent: the live macOS system-accent
                      // token breaks as a background-color (see tokens.css).
                      backgroundColor: "var(--color-accent-fill, var(--color-accent-fill-val))",
                    }}
                  />
                )}
              </div>

              {elapsed != null && (
                <div
                  aria-hidden="true"
                  data-testid={`live-usage-elapsed-${window.id}`}
                  className="absolute inset-y-0 w-[1.5px] z-1 bg-(--color-label)"
                  style={{ left: `${Math.min(100, Math.max(0, elapsed * 100))}%` }}
                />
              )}
            </div>
          </div>
        )
      })}
    </div>
  )
}
