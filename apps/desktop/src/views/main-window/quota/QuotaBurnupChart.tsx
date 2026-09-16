import {
  Area,
  AreaChart,
  ReferenceLine,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts"

import { axisDayLabel } from "../../../lib/presentation/overviewChart"
import { slowAnimationDurationMs } from "../../../lib/popoverHeight"
import type { QuotaPeriodPayload, QuotaUsagePayload } from "../../../lib/providerUsageIpc"
import { ChartKey } from "../../../components/session/analysis/ChartKey"
import {
  AXIS_LABEL,
  AXIS_TICK,
  labeledIndices,
} from "../../../components/session/analysis/chartLabels"
import { GLASS_TOOLTIP_STYLE } from "../../../components/session/analysis/tooltip"
import { useChartResize } from "../../../components/session/analysis/useChartResize"
import { quotaBurnupSeries, type QuotaSeriesRow, type QuotaTopSession } from "./quotaSeries"

const DAY_SECS = 24 * 60 * 60
/** Switch from a daily to a weekly x-axis tick past this span. */
const DAILY_TICKS_MAX_SPAN_SECS = 8 * DAY_SECS
const CHART_MIN_HEIGHT = 200
const TIME_AXIS_HEIGHT = 16
const VALUE_AXIS_WIDTH = 32
/** How close two reset labels may sit, as a share of the visible x-domain. */
const RESET_LABEL_MIN_GAP_FRACTION = 0.04

const SESSION_SWATCHES = [
  "bg-quota-session-1",
  "bg-quota-session-2",
  "bg-quota-session-3",
  "bg-quota-session-4",
  "bg-quota-session-5",
] as const

/** A layer of the burnup key: `"meter"`, `"other"`, `"unattributed"`, or a top session's key. */
export type QuotaChartSeries = string

export interface QuotaBurnupChartProps {
  usage: QuotaUsagePayload
  rangeStartEpoch: number
  rangeEndEpoch: number
  nowEpoch: number
  /** The layer to isolate. Null draws every layer lit; a named layer rests every other one in grey. */
  highlight: QuotaChartSeries | null
  onHighlight: (series: QuotaChartSeries | null) => void
  onPin: (series: QuotaChartSeries) => void
}

/** "42%", or an em dash for a percent the lane cannot state. */
export function formatQuotaPercent(value: number | null | undefined): string {
  return value == null ? "—" : `${Math.round(value)}%`
}

/** The reader-local `YYYY-MM-DD` date an epoch-second instant falls on. */
function localDateOf(epoch: number): string {
  const date = new Date(epoch * 1000)
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, "0")
  const day = String(date.getDate()).padStart(2, "0")
  return `${year}-${month}-${day}`
}

/** Local midnight at or before `epoch`. */
function localMidnight(epoch: number): number {
  const date = new Date(epoch * 1000)
  date.setHours(0, 0, 0, 0)
  return Math.floor(date.getTime() / 1000)
}

/** One tick per day for a range up to eight days, else one tick per week. */
function xAxisTicks(rangeStart: number, rangeEnd: number): number[] {
  const stepSecs = rangeEnd - rangeStart <= DAILY_TICKS_MAX_SPAN_SECS ? DAY_SECS : 7 * DAY_SECS
  const ticks: number[] = []
  for (let t = localMidnight(rangeStart); t <= rangeEnd; t += stepSecs) {
    if (t >= rangeStart) ticks.push(t)
  }
  return ticks
}

/** A key entry. Omits `series` entirely, rather than setting it `undefined`,
 *  for a stat the key cannot highlight or pin. */
function keyStat(
  label: string,
  value: string,
  series: QuotaChartSeries | null,
): { label: string; value: string; series?: QuotaChartSeries } {
  return series == null ? { label, value } : { label, value, series }
}

function cumulativeAt(
  row: QuotaSeriesRow,
  topSessions: readonly QuotaTopSession[],
): number | null {
  if (row.other == null || row.unattributed == null) return null
  let total = row.other + row.unattributed
  for (const session of topSessions) {
    const value = row[session.key]
    if (value == null) return null
    total += value
  }
  return total
}

export interface QuotaBurnupTooltipProps {
  active?: boolean
  payload?: Array<{ payload?: QuotaSeriesRow }>
  topSessions: readonly QuotaTopSession[]
  hasFactor: boolean
}

/** The hover card: local time, the meter reading, each stacked layer, and the estimate total. */
export function QuotaBurnupTooltip({
  active,
  payload,
  topSessions,
  hasFactor,
}: QuotaBurnupTooltipProps) {
  const row = payload?.[0]?.payload
  if (!active || !row) return null
  const time = new Date(row.t * 1000).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  })
  const total = cumulativeAt(row, topSessions)
  return (
    <div
      className="text-label"
      style={{
        ...GLASS_TOOLTIP_STYLE,
        lineHeight: 1.4,
        padding: "6px 9px",
        whiteSpace: "nowrap",
      }}
    >
      <div className="mb-1">{time}</div>
      <div className="flex flex-col gap-1 text-label-secondary">
        {row.meter != null && (
          <span>
            Meter · <span className="tabular-nums">{formatQuotaPercent(row.meter)}</span>
          </span>
        )}
        {hasFactor &&
          topSessions.map((session, index) => (
            <span key={session.key} className="flex items-center gap-1.5">
              <span
                aria-hidden="true"
                className={`size-2 shrink-0 rounded-full ${SESSION_SWATCHES[index % SESSION_SWATCHES.length]}`}
              />
              {session.title ?? `${session.agent} session`} ·{" "}
              <span className="tabular-nums">{formatQuotaPercent(row[session.key])}</span>
            </span>
          ))}
        {hasFactor && (
          <span>
            Other sessions ·{" "}
            <span className="tabular-nums">{formatQuotaPercent(row.other)}</span>
          </span>
        )}
        {hasFactor && (
          <span>
            Unattributed ·{" "}
            <span className="tabular-nums">{formatQuotaPercent(row.unattributed)}</span>
          </span>
        )}
        {total != null && (
          <span>
            Estimate total · <span className="tabular-nums">{formatQuotaPercent(total)}</span>
          </span>
        )}
      </div>
    </div>
  )
}

/** True when any period's reset in range was not stated directly by the provider. */
function hasInferredReset(periods: readonly QuotaPeriodPayload[]): boolean {
  return periods.some((period) => period.resetSource !== "reported")
}

/**
 * The Quota screen's burnup chart: the provider's own meter line over the
 * device's stacked estimate, one window at a time along a wall-clock x axis,
 * with a key below naming every layer the plot can highlight.
 */
export function QuotaBurnupChart({
  usage,
  rangeStartEpoch,
  rangeEndEpoch,
  nowEpoch,
  highlight,
  onHighlight,
  onPin,
}: QuotaBurnupChartProps) {
  const hasFactor = usage.factor != null
  const { rows, topSessions } = quotaBurnupSeries(usage, rangeStartEpoch, rangeEndEpoch)
  const { onResize, resizing, animate } = useChartResize(rows)
  const animationDurationMs = slowAnimationDurationMs()

  const ticks = xAxisTicks(rangeStartEpoch, rangeEndEpoch)
  const tickLabels = new Map(ticks.map((t) => [t, axisDayLabel(localDateOf(t))]))

  const periodsInRange = usage.periods.filter(
    (period) => period.resetsAtEpoch > rangeStartEpoch && period.startsAtEpoch < rangeEndEpoch,
  )
  const resetEpochs = periodsInRange
    .map((period) => period.resetsAtEpoch)
    .filter((t) => t >= rangeStartEpoch && t <= rangeEndEpoch)
  const labeledResets = labeledIndices(
    resetEpochs,
    Math.max(1, rangeEndEpoch - rangeStartEpoch),
    RESET_LABEL_MIN_GAP_FRACTION,
  )
  const showNowLine = nowEpoch >= rangeStartEpoch && nowEpoch <= rangeEndEpoch
  const showInferredCaption = hasInferredReset(periodsInRange)

  const latestRow = rows.reduce<QuotaSeriesRow | null>((closest, row) => {
    if (!closest) return row
    return Math.abs(row.t - nowEpoch) < Math.abs(closest.t - nowEpoch) ? row : closest
  }, null)

  const keyStats = [
    keyStat("Meter", latestRow ? formatQuotaPercent(latestRow.meter) : "—", "meter"),
    ...topSessions.map((session) =>
      keyStat(
        session.title ?? `${session.agent} session`,
        hasFactor
          ? formatQuotaPercent(latestRow ? latestRow[session.key] : null)
          : "No estimate yet",
        hasFactor ? session.key : null,
      ),
    ),
    keyStat(
      "Other sessions",
      hasFactor ? formatQuotaPercent(latestRow?.other ?? null) : "No estimate yet",
      hasFactor ? "other" : null,
    ),
    keyStat(
      "Unattributed",
      hasFactor ? formatQuotaPercent(latestRow?.unattributed ?? null) : "No estimate yet",
      hasFactor ? "unattributed" : null,
    ),
  ]
  const swatchClass: Record<QuotaChartSeries, string> = { meter: "bg-quota-meter" }
  topSessions.forEach((session, index) => {
    swatchClass[session.key] = SESSION_SWATCHES[index % SESSION_SWATCHES.length]!
  })
  swatchClass.other = "bg-quota-other"
  swatchClass.unattributed = "bg-quota-unattributed"

  function seriesColor(series: QuotaChartSeries, litColor: string): string {
    const lit = highlight == null || highlight === series
    return lit ? litColor : "var(--color-chart-rest-faint)"
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <ResponsiveContainer
        width="100%"
        height="100%"
        minHeight={CHART_MIN_HEIGHT}
        onResize={onResize}
      >
        <AreaChart data={rows} margin={{ top: 6, right: 12, bottom: 0, left: 0 }}>
          <XAxis
            dataKey="t"
            type="number"
            domain={[rangeStartEpoch, rangeEndEpoch]}
            ticks={ticks}
            tickFormatter={(value: number) => tickLabels.get(value) ?? ""}
            axisLine={false}
            tickLine={false}
            tickMargin={2}
            height={TIME_AXIS_HEIGHT}
            tick={AXIS_TICK}
          />
          <YAxis
            domain={[0, 100]}
            ticks={[0, 25, 50, 75, 100]}
            tickFormatter={(value: number) => `${value}%`}
            axisLine={false}
            tickLine={false}
            tickMargin={4}
            width={VALUE_AXIS_WIDTH}
            tick={AXIS_TICK}
            label={{ value: "% of limit", angle: -90, position: "insideLeft", ...AXIS_TICK }}
          />
          <Tooltip
            cursor={{ stroke: "var(--color-separator)" }}
            isAnimationActive={false}
            content={<QuotaBurnupTooltip topSessions={topSessions} hasFactor={hasFactor} />}
          />
          {hasFactor &&
            topSessions.map((session, index) => (
              <Area
                key={session.key}
                type="stepAfter"
                dataKey={session.key}
                stackId="estimate"
                stroke="none"
                connectNulls={false}
                fill={seriesColor(session.key, `var(--color-quota-session-${(index % 5) + 1})`)}
                isAnimationActive={animate}
                animationDuration={animationDurationMs}
              />
            ))}
          {hasFactor && (
            <Area
              type="stepAfter"
              dataKey="other"
              stackId="estimate"
              stroke="none"
              connectNulls={false}
              fill={seriesColor("other", "var(--color-quota-other)")}
              isAnimationActive={animate}
              animationDuration={animationDurationMs}
            />
          )}
          {hasFactor && (
            <Area
              type="stepAfter"
              dataKey="unattributed"
              stackId="estimate"
              stroke="none"
              connectNulls={false}
              fill={seriesColor("unattributed", "var(--color-quota-unattributed)")}
              isAnimationActive={animate}
              animationDuration={animationDurationMs}
            />
          )}
          <Area
            type="linear"
            dataKey="meter"
            stroke={seriesColor("meter", "var(--color-quota-meter)")}
            fill="none"
            fillOpacity={0}
            dot={false}
            connectNulls={false}
            isAnimationActive={animate}
            animationDuration={animationDurationMs}
          />
          <ReferenceLine y={100} stroke="var(--color-context-critical)" strokeDasharray="2 2" />
          {periodsInRange.map((period) => {
            if (
              period.resetsAtEpoch < rangeStartEpoch ||
              period.resetsAtEpoch > rangeEndEpoch
            ) {
              return null
            }
            const inferred = period.resetSource !== "reported"
            const labeled = labeledResets.has(period.resetsAtEpoch)
            return (
              <ReferenceLine
                key={`reset-${period.periodId ?? period.resetsAtEpoch}`}
                x={period.resetsAtEpoch}
                stroke="var(--color-chart-rest-mark)"
                {...(inferred ? { strokeDasharray: "4 3" } : {})}
                {...(labeled
                  ? { label: { ...AXIS_LABEL, value: "reset", position: "insideTop" as const } }
                  : {})}
              />
            )
          })}
          {showNowLine && (
            <ReferenceLine
              x={nowEpoch}
              stroke="var(--color-label)"
              label={{ ...AXIS_LABEL, value: "now", position: "insideTop" as const }}
            />
          )}
        </AreaChart>
      </ResponsiveContainer>
      <ChartKey
        stats={keyStats}
        pinned={highlight}
        onHighlight={onHighlight}
        onPin={onPin}
        swatchClass={swatchClass}
      />
      {showInferredCaption && (
        <p className="type-caption text-label-tertiary">
          Dashed reset lines are inferred, not stated by the provider.
        </p>
      )}
      {resizing && <span className="sr-only">Resizing.</span>}
    </div>
  )
}
