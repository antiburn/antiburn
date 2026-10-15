import { useState, type KeyboardEvent } from "react"

import type {
  ProviderUsageDayPayload,
  ProviderUsageWindowPayload,
} from "../../../lib/providerUsageIpc"
import { cn } from "../../../lib/cn"
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
import { useEntranceProps } from "./overviewEntrance"
import { UsageBanner, UsageBannerTip } from "./UsageBanner"

import "./overview.css"

/* Each agent has its own hue, the same on every chart. An agent without its
   own hue takes the neutral. */
const AGENT_STYLES: Record<string, { fill: string; swatch: string }> = {
  "claude-code": { fill: "fill-agent-claude-code", swatch: "bg-agent-claude-code" },
  codex: { fill: "fill-agent-codex", swatch: "bg-agent-codex" },
}
const OTHER_AGENT_STYLE = { fill: "fill-agent-other", swatch: "bg-agent-other" }

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

/** The day's tooltip: total spend, then each agent's share of it. */
function dayTip(day: ProviderUsageDayPayload, isToday: boolean) {
  const agents = [...(day.agents ?? [])].sort(
    (left, right) => (right.estimatedUsd ?? 0) - (left.estimatedUsd ?? 0),
  )
  const total = day.estimatedUsd ?? 0
  const footnote = [
    windowTokens(day) > 0 ? formatTokenFigure(windowTokens(day)) : null,
    windowTokens(day) > 0 ? sessionCountLabel(day.sessionCount) : null,
    !day.agents?.length && windowTokens(day) > 0 ? "Agent breakdown unavailable" : null,
  ].filter((part) => part != null)
  return (
    <UsageBannerTip
      title={isToday ? "Today" : dayLabel(day.localDate)}
      figure={spendLabel(day)}
      rows={agents.map((usage) => ({
        key: usage.agent,
        label: agentDisplayName(usage.agent),
        value: spendLabel(usage),
        swatch: (AGENT_STYLES[usage.agent] ?? OTHER_AGENT_STYLE).swatch,
        ...(total > 0 ? { share: (usage.estimatedUsd ?? 0) / total } : {}),
      }))}
      footnote={footnote.length ? footnote.join(" · ") : undefined}
    />
  )
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

/**
 * `banner` draws the columns only, with no legend, axes, or day tooltips, to
 * fill the usage card behind the spend figures.
 */
export function OverviewSpendChart({
  days,
  loading = false,
  banner = false,
}: {
  days: ReadonlyArray<ProviderUsageDayPayload>
  loading?: boolean
  banner?: boolean
}) {
  const [focusDate, setFocusDate] = useState<string | null>(null)
  // The day under the pointer or the keyboard focus. The other days dim.
  const [litDate, setLitDate] = useState<string | null>(null)
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
  // Largest 30-day spend first: that agent takes the left column of each day.
  const agents = [...agentTotals.keys()].sort((left, right) => {
    const diff = (agentTotals.get(right) ?? 0) - (agentTotals.get(left) ?? 0)
    return diff !== 0 ? diff : left.localeCompare(right)
  })
  // Each agent has its own column, so the scale is the highest single column.
  const peaks = days.flatMap((day) =>
    (day.agents ?? []).map((usage) => usage.estimatedUsd ?? 0),
  )
  const { ceiling, guideFractions } = spendScale(Math.max(0, ...peaks))
  const wholeGuides = guideFractions.every((fraction) => Number.isInteger(ceiling * fraction))
  const slotWidth = 100 / dayCount
  // The agents' columns stand side by side in each day's slot.
  const columnWidth = (slotWidth * 0.8) / Math.max(1, agents.length)
  const columnInset = slotWidth * 0.2
  const series = agents.map((agent) => {
    const rects = days.flatMap((day, index) => {
      const usd = day.agents?.find((usage) => usage.agent === agent)?.estimatedUsd ?? 0
      if (usd <= 0) return []
      return [{ index, height: (usd / ceiling) * 100 }]
    })
    return {
      agent,
      rects,
      style: AGENT_STYLES[agent] ?? OTHER_AGENT_STYLE,
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

  const bars = (
    <svg
      className="pointer-events-none absolute inset-0 h-full w-full overflow-hidden"
      viewBox="0 0 100 100"
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      {series.map(({ agent, rects, style }, agentIndex) => (
        <g key={agent} data-agent={agent} className={style.fill}>
          {rects.map(({ index, height }) => (
            <rect
              key={index}
              x={index * slotWidth + columnInset + agentIndex * columnWidth}
              y={100 - height}
              width={columnWidth}
              height={height}
              className={cn(
                "transition-opacity duration-fast",
                litDate != null && days[index]?.localDate !== litDate
                  ? "opacity-25"
                  : "opacity-85",
              )}
            />
          ))}
        </g>
      ))}
    </svg>
  )

  const dayButtons = (className: string) => (
    <div
      role="group"
      aria-label="Estimated spend for the past 30 days"
      className={className}
      onMouseLeave={() => setLitDate(null)}
      onBlur={() => setLitDate(null)}
    >
      {days.map((day, index) => {
        const detail = dayDetail(day, index === lastIndex)
        const incomplete = !day.costComplete || (!day.agents?.length && windowTokens(day) > 0)
        return (
          <Tooltip key={day.localDate} label={dayTip(day, index === lastIndex)} delayMs={0}>
            <button
              type="button"
              data-day={day.localDate}
              aria-label={detail}
              tabIndex={index === focusIndex ? 0 : -1}
              className="group absolute inset-y-0 border-0 bg-transparent p-0"
              style={{ left: `${index * slotWidth}%`, width: `${slotWidth}%` }}
              onMouseEnter={() => setLitDate(day.localDate)}
              onFocus={() => {
                setFocusDate(day.localDate)
                setLitDate(day.localDate)
              }}
              onKeyDown={(event) => onKeyDown(event, index)}
            >
              <span
                aria-hidden="true"
                className="pointer-events-none absolute inset-0 rounded-control bg-label/0 group-hover:bg-label/5 group-focus-visible:bg-label/5"
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
  )

  const placeholder = loading || days.length === 0
  const entranceProps = useEntranceProps("spend-chart", "overview-chart-in", !placeholder)
  if (banner) {
    if (placeholder) return null
    return (
      <UsageBanner
        name="spend"
        plot={bars}
        keyItems={series.map(({ agent, style }) => ({
          key: agent,
          label: agentDisplayName(agent),
          swatch: style.swatch,
        }))}
        dates={days.flatMap((day, index) =>
          index === lastIndex || (index % 7 === 0 && index < lastIndex - 3)
            ? [
                {
                  key: day.localDate,
                  text: index === lastIndex ? "Today" : axisDayLabel(day.localDate),
                  at: index === lastIndex ? 1 : index / dayCount,
                },
              ]
            : [],
        )}
        hover={dayButtons}
      />
    )
  }

  return (
    <section
      {...entranceProps}
      className={cn("overview-chart", entranceProps.className)}
      aria-label="Estimated spend by day"
      aria-busy={loading || undefined}
    >
      {placeholder ? (
        <>
          {/* The same frame the chart draws in: the agent legend above, the
              value labels beside and the day labels below, all held open and
              invisible. The block then sits exactly where the plot will, and
              nothing on the page moves when the chart replaces it. */}
          <div aria-hidden="true" className="invisible mb-(--space-sm)">
            <ChartLegend
              ariaLabel="Agents"
              items={[{ key: "placeholder", label: "Agent", swatch: "bg-transparent" }]}
            />
          </div>
          <div
            aria-hidden="true"
            className="grid min-h-(--overview-chart-height) flex-auto grid-cols-[minmax(0,1fr)_auto] grid-rows-[minmax(0,1fr)_auto] gap-x-(--space-sm) gap-y-(--space-xs) pt-(--space-sm)"
          >
            <div className="overview-chart-placeholder" />
            <div className="type-metadata invisible">
              <SegmentFigure>$0.00</SegmentFigure>
            </div>
            <div className="type-caption invisible h-[1.4em]" />
          </div>
        </>
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
              {bars}
              {dayButtons("absolute inset-0")}
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
