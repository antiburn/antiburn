import { RotateCcw } from "lucide-react"

import { cn } from "../../lib/cn"
import type { LiveProviderUsagePayload } from "../../lib/ipc"
import {
  liveExtraUsageLabel,
  liveFreshnessToneClass,
  liveSourceNote,
  liveStalenessNote,
  livePlanAccountLabel,
  liveWindows,
} from "../../lib/presentation/liveUsage"
import { LiveMetricRows } from "./LiveMetricRows"
import { LiveUsageWindowRows } from "./LiveUsageWindowRows"

/**
 * The provider's own limits, with what their history says about them.
 *
 * Two blocks. The meters answer "how much is gone", which one reading can
 * say. The rows beneath answer "how fast" and "will it last", which only a
 * series can — and which are unavailable far more often than they are
 * available, because the source only moves when an agent runs.
 *
 * Those unavailable rows are still rendered, carrying their reason. A row
 * that disappears takes its question with it, and a reader who cannot find
 * "runway" concludes the app does not have the concept rather than that it
 * has not seen enough of their week yet.
 */
export function LiveUsageDetail({
  live,
  now,
  accountLabel,
  showPlan = false,
  showRunway = true,
  className = "",
}: {
  live: LiveProviderUsagePayload
  /** Injected so the rendered output is a function of its inputs in tests. */
  now: number
  accountLabel?: string
  showPlan?: boolean
  showRunway?: boolean
  className?: string
}) {
  const staleness = liveStalenessNote(live)
  const extra = liveExtraUsageLabel(live)
  const windows = liveWindows(live)
  const primary = windows[0]
  const plan = livePlanAccountLabel(live)
  if (!primary && !extra && !live.resetCredits && !live.plan) return null

  return (
    <section
      aria-label={`${live.displayName}${accountLabel ? ` ${accountLabel}` : ""} plan limits`}
      className={cn("space-y-2", className)}
    >
      <div className="flex items-baseline justify-between gap-2 group">
        <h4 className="type-caption font-medium tracking-wide uppercase text-label-tertiary">
          Plan limits
        </h4>
        <span className={cn("type-caption text-right", liveFreshnessToneClass(live.freshness))}>
          {[accountLabel, live.sourceLabel, liveSourceNote(live)].filter(Boolean).join(" · ")}
        </span>
      </div>

      {showPlan && plan && (
        <p className="type-caption text-label-secondary">
          Plan · <span className="text-label">{plan}</span>
        </p>
      )}

      {windows.length > 0 && <LiveUsageWindowRows provider={live} now={now} />}

      {live.resetCredits && live.resetCredits.availableCount > 0 && (
        <ResetCreditsNotice provider={live} availableCount={live.resetCredits.availableCount} />
      )}

      {/* Derived rows for the primary window only. Repeating pace and runway
          under every per-model limit would triple the panel's height to say
          the same thing three ways; the full picture is one tap away in the
          Usage view, which has the room for it. */}
      {primary && (
        <LiveMetricRows
          window={primary}
          now={now}
          {...(!showRunway ? { keys: ["pace", "today"] } : {})}
          className="border-t border-separator pt-1.5"
        />
      )}

      {extra && <p className="type-caption text-label-tertiary">{extra}</p>}
      {staleness && <p className="type-caption text-system-orange">{staleness}</p>}
    </section>
  )
}

function ResetCreditsNotice({
  provider,
  availableCount,
}: {
  provider: LiveProviderUsagePayload
  availableCount: number
}) {
  const noun = availableCount === 1 ? "reset" : "resets"
  const action =
    provider.provider === "openai" ? (
      <>
        Run <span className="font-mono">/usage</span> in Codex to use one.
      </>
    ) : (
      <>Use {provider.displayName} to apply one.</>
    )
  return (
    <div className="flex items-start gap-1.5 rounded-control bg-system-green/10 px-2 py-1.5">
      <RotateCcw
        size={13}
        strokeWidth={2}
        aria-hidden="true"
        className="mt-px shrink-0 text-system-green"
      />
      <p className="min-w-0 type-footnote text-label-secondary">
        <span className="font-medium text-system-green">
          {availableCount} usage limit {noun} available.
        </span>{" "}
        {action}
      </p>
    </div>
  )
}
