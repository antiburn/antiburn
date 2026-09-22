import { useState, type KeyboardEvent } from "react"

import type {
  ProviderUsageDayPayload,
  ProviderUsageWindowPayload,
} from "../../../lib/providerUsageIpc"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { axisDayLabel, dayLabel } from "../../../lib/presentation/chartDates"
import {
  formatSpendFigure,
  formatTokenFigure,
  sessionCountLabel,
  windowTokens,
} from "../../../lib/presentation/providerUsage"
import { Tooltip } from "../../../components/presentation/Tooltip"
import { ChartLegend } from "../../../components/ui/ChartLegend"
import { SegmentFigure } from "../../../components/ui/SegmentFigure"
import { Skeleton } from "../../../components/ui/Skeleton"

import "./overview.css"

const LAYER_STYLES = [
  { fill: "fill-token-in", swatch: "bg-token-in" },
  { fill: "fill-token-in/55", swatch: "bg-token-in/55" },
  { fill: "fill-token-in/30", swatch: "bg-token-in/30" },
  { fill: "fill-token-in/[0.18]", swatch: "bg-token-in/[0.18]" },
] as const

function spendScale(max: number): { ceiling: number; guideFractions: number[] } {
  const peak = Number.isFinite(max) && max > 0 ? max : 1
  const targetStep = peak / 5
  const power = 10 ** Math.floor(Math.log10(targetStep))
  const multiple = [1, 2, 2.5, 5, 10].find((value) => value * power >= targetStep) ?? 10
  const step = multiple * power
  const tickCount = Math.ceil(peak / step)
  return {
    ceiling: tickCount * step,
    guideFractions: Array.from(
      { length: tickCount },
      (_, index) => (tickCount - index) / tickCount,
    ),
  }
}

function spendLabel(usage: ProviderUsageWindowPayload): string {
  if (usage.estimatedUsd != null) {
    return `${usage.costComplete ? "" : "at least "}${formatSpendFigure(usage.estimatedUsd)}`
  }
  return windowTokens(usage) > 0 ? "not priced" : "no sessions"
}

function dayDetail(day: ProviderUsageDayPayload, isToday: boolean): string {
  const parts = [isToday ? "Today" : dayLabel(day.localDate), spendLabel(day)]
  if (windowTokens(day) > 0) {
    parts.push(formatTokenFigure(windowTokens(day)), sessionCountLabel(day.sessionCount))
  }
  for (const usage of day.agents ?? []) {
    parts.push(`${agentDisplayName(usage.agent)}: ${spendLabel(usage)}`)
  }
  if (!day.agents?.length && windowTokens(day) > 0) parts.push("Agent breakdown unavailable")
  return parts.join(" · ")
}

export function OverviewSpendChart({
  days,
  loading = false,
}: {
  days: ReadonlyArray<ProviderUsageDayPayload>
  loading?: boolean
}) {
  const [focusDate, setFocusDate] = useState<string | null>(null)
  const lastIndex = days.length - 1
  const foundFocus =
    focusDate == null ? -1 : days.findIndex((day) => day.localDate === focusDate)
  const focusIndex = foundFocus >= 0 ? foundFocus : lastIndex
  const dayCount = days.length
  const agentTotals = new Map<string, number>()
  for (const day of days) {
    for (const usage of day.agents ?? []) {
      agentTotals.set(
        usage.agent,
        (agentTotals.get(usage.agent) ?? 0) + (usage.estimatedUsd ?? 0),
      )
    }
  }
  // Largest 30-day spend first: that agent is the solid foot of every column.
  const agents = [...agentTotals.keys()].sort((left, right) => {
    const diff = (agentTotals.get(right) ?? 0) - (agentTotals.get(left) ?? 0)
    return diff !== 0 ? diff : left.localeCompare(right)
  })
  const totals = days.map((day) =>
    (day.agents ?? []).reduce((sum, usage) => sum + (usage.estimatedUsd ?? 0), 0),
  )
  const { ceiling, guideFractions } = spendScale(Math.max(0, ...totals))
  const wholeGuides = guideFractions.every((fraction) => Number.isInteger(ceiling * fraction))
  const slotWidth = 100 / dayCount
  const columnWidth = slotWidth * 0.8
  const columnInset = slotWidth * 0.2
  const lower = days.map(() => 0)
  const series = agents.map((agent, agentIndex) => {
    const rects = days.flatMap((day, index) => {
      const usd = day.agents?.find((usage) => usage.agent === agent)?.estimatedUsd ?? 0
      const bottom = lower[index]!
      lower[index] = bottom + usd
      if (usd <= 0) return []
      return [
        {
          index,
          lower: (bottom / ceiling) * 100,
          upper: ((bottom + usd) / ceiling) * 100,
        },
      ]
    })
    return {
      agent,
      rects,
      style: LAYER_STYLES[Math.min(agentIndex, LAYER_STYLES.length - 1)]!,
    }
  })

  function onKeyDown(event: KeyboardEvent<HTMLButtonElement>, index: number): void {
    const target =
      event.key === "ArrowLeft"
        ? index - 1
        : event.key === "ArrowRight"
          ? index + 1
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? lastIndex
              : null
    if (target == null) return
    event.preventDefault()
    const day = days[Math.max(0, Math.min(lastIndex, target))]
    if (!day) return
    setFocusDate(day.localDate)
    event.currentTarget.parentElement
      ?.querySelector<HTMLButtonElement>(`[data-day="${day.localDate}"]`)
      ?.focus()
  }

  return (
    <section
      className="overview-chart"
      aria-label="Estimated spend by day"
      aria-busy={loading || undefined}
    >
      {loading || days.length === 0 ? (
        <Skeleton className="block min-h-(--overview-chart-height) w-full flex-1" />
      ) : (
        <>
          <ChartLegend
            ariaLabel="Agents"
            className="mb-(--space-sm)"
            items={series.map(({ agent, style }) => ({
              key: agent,
              label: agentDisplayName(agent),
              swatch: style.swatch,
            }))}
          />
          <div className="grid min-h-(--overview-chart-height) flex-auto grid-cols-[minmax(0,1fr)_auto] grid-rows-[minmax(0,1fr)_auto] gap-x-(--space-sm) gap-y-(--space-xs) pt-(--space-sm)">
            <div className="relative min-h-(--overview-chart-height)">
              <div aria-hidden="true" className="pointer-events-none absolute inset-0">
                {guideFractions.map((fraction) => (
                  <div
                    key={fraction}
                    className="absolute inset-x-0 border-t border-separator/60"
                    style={{ top: `${(1 - fraction) * 100}%` }}
                  />
                ))}
              </div>
              <svg
                className="pointer-events-none absolute inset-0 h-full w-full overflow-hidden"
                viewBox="0 0 100 100"
                preserveAspectRatio="none"
                aria-hidden="true"
              >
                {series.map(({ agent, rects, style }) => (
                  <g key={agent} data-agent={agent}>
                    {rects.map(({ index, lower: segmentLower, upper }) => (
                      <rect
                        key={index}
                        x={index * slotWidth + columnInset}
                        y={100 - upper}
                        width={columnWidth}
                        height={upper - segmentLower}
                        className={style.fill}
                      />
                    ))}
                  </g>
                ))}
              </svg>
              <div
                role="group"
                aria-label="Estimated spend for the past 30 days"
                className="absolute inset-0"
              >
                {days.map((day, index) => {
                  const detail = dayDetail(day, index === lastIndex)
                  const incomplete =
                    !day.costComplete || (!day.agents?.length && windowTokens(day) > 0)
                  return (
                    <Tooltip
                      key={day.localDate}
                      label={<SegmentFigure>{detail}</SegmentFigure>}
                      delayMs={100}
                    >
                      <button
                        type="button"
                        data-day={day.localDate}
                        aria-label={detail}
                        tabIndex={index === focusIndex ? 0 : -1}
                        className="group absolute inset-y-0 border-0 bg-transparent p-0"
                        style={{ left: `${index * slotWidth}%`, width: `${slotWidth}%` }}
                        onFocus={() => setFocusDate(day.localDate)}
                        onKeyDown={(event) => onKeyDown(event, index)}
                      >
                        <span
                          aria-hidden="true"
                          className="pointer-events-none absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-label/0 group-hover:bg-label/25 group-focus-visible:bg-label/25"
                        />
                        {incomplete && (
                          <span
                            aria-hidden="true"
                            data-unpriced
                            className="absolute inset-x-0 bottom-0 border-t-2 border-dashed border-label-secondary"
                          />
                        )}
                      </button>
                    </Tooltip>
                  )
                })}
              </div>
            </div>
            <div aria-hidden="true" className="type-metadata relative text-label-tertiary">
              <span className="invisible">
                {wholeGuides
                  ? `$${ceiling.toLocaleString("en-US")}`
                  : formatSpendFigure(ceiling)}
              </span>
              {guideFractions.map((fraction) => (
                <span
                  key={fraction}
                  className="absolute right-0 -translate-y-1/2 whitespace-nowrap"
                  style={{ top: `${(1 - fraction) * 100}%` }}
                >
                  <SegmentFigure>
                    {wholeGuides
                      ? `$${(ceiling * fraction).toLocaleString("en-US")}`
                      : formatSpendFigure(ceiling * fraction)}
                  </SegmentFigure>
                </span>
              ))}
            </div>
            <div
              aria-hidden="true"
              className="type-caption relative h-[1.4em] text-label-tertiary"
            >
              {days.map((day, index) =>
                index === lastIndex || (index % 7 === 0 && index < lastIndex - 3) ? (
                  <span
                    key={day.localDate}
                    className={`absolute whitespace-nowrap ${index === lastIndex ? "right-0" : ""}`}
                    style={index === lastIndex ? undefined : { left: `${index * slotWidth}%` }}
                  >
                    {index === lastIndex ? "Today" : axisDayLabel(day.localDate)}
                  </span>
                ) : null,
              )}
            </div>
          </div>
        </>
      )}
    </section>
  )
}
