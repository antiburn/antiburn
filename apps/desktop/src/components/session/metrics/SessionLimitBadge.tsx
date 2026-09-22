import { Flame } from "lucide-react"

import { Tooltip } from "../../presentation/Tooltip"

/**
 * A session's estimated share of one provider limit lane.
 *
 * A null percent shows the missing limit label: "unknown" when a live
 * window exists for the session's provider but did not attribute a share
 * to it, "no limit" when the provider reports no such window at all.
 */
export interface SessionLimitBadgeInfo {
  label: string
  percent: number | null
  provider?: string
  windowId?: string
  unknown?: boolean
}

export interface SessionLimitBadgeProps {
  limitBadge: SessionLimitBadgeInfo
  /** Trailing words after the percent sign, e.g. "of week". Only the
   *  percent states take it; the missing-limit states never do. */
  suffix?: string
  /** True draws every share as plain text, with no pill and no flame at a
   *  high share. A list that ranks sessions by share needs no such mark. */
  plain?: boolean
}

/**
 * Show the share as a figure and a percent sign.
 *
 * English style puts no space before the percent sign, so the two stay one
 * text run: the figure and the sign are sibling text nodes, and the value
 * copies as "17.2%". The empty element between them is a hair space. The
 * monospace percent sign fills its cell with ink, and without that space it
 * looks joined to the last digit.
 */
function LimitPercent({ percent }: { percent: number }) {
  return (
    <>
      {roundedLimitPercent(percent)}
      <span aria-hidden="true" className="inline-block w-px" />%
    </>
  )
}

/** The percent figure rounded to one decimal, the same way the badge rounds it. */
export function roundedLimitPercent(percent: number): number {
  return Number(percent.toFixed(1))
}

/**
 * A session's limit-share pill: the percent figure at high share, plain text
 * below it, or the missing-limit label when the session has no percent.
 */
export function SessionLimitBadge({
  limitBadge,
  suffix,
  plain = false,
}: SessionLimitBadgeProps) {
  const isHighLimitShare = !plain && roundedLimitPercent(limitBadge.percent ?? 0) >= 5

  return limitBadge.percent !== null ? (
    <Tooltip label={limitBadge.label} delayMs={150}>
      <span
        className={
          isHighLimitShare
            ? // The pill keeps the tracking of type-footnote. Tighter
              // tracking moves the wide percent sign into the last digit,
              // because the monospace cell is already full.
              "flex shrink-0 items-center gap-0.5 rounded-full bg-brand-tint px-1.5 py-px font-mono type-footnote font-medium! leading-[13px] text-white tabular-nums"
            : // The same 13px line box as the pill, so the two states of
              // the badge occupy one box.
              "font-mono type-footnote leading-[13px] tabular-nums text-label-secondary"
        }
        data-session-limit-provider={limitBadge.provider}
        data-session-limit-window={limitBadge.windowId}
        data-session-limit-percent={limitBadge.percent.toFixed(4)}
        aria-label={
          isHighLimitShare
            ? `${limitBadge.label} This session uses 5% or more of your limit.`
            : limitBadge.label
        }
        tabIndex={0}
      >
        {isHighLimitShare && <Flame size={11} className="shrink-0" aria-hidden="true" />}
        <LimitPercent percent={limitBadge.percent} />
        {suffix && ` ${suffix}`}
      </span>
    </Tooltip>
  ) : (
    <Tooltip label={limitBadge.label} delayMs={150}>
      <span
        className="font-mono type-footnote leading-[13px] text-label-secondary opacity-50"
        aria-label={limitBadge.label}
        tabIndex={0}
      >
        {limitBadge.unknown ? "unknown" : "no limit"}
      </span>
    </Tooltip>
  )
}
