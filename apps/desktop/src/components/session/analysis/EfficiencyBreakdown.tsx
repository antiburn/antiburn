import { useId, useState, type ReactNode } from "react"

import { cn } from "../../../lib/cn"
import {
  efficiencyBandWord,
  efficiencyThresholdGuidance,
  efficiencyThermometer,
  formatCostPerMTok,
  formatSharePercent,
  type EfficiencyBand,
  type EfficiencyMetric,
  type EfficiencyMetrics,
  type EfficiencyProfile,
} from "../../../lib/presentation/sessionEfficiency"

export interface EfficiencyBreakdownProps {
  metrics: EfficiencyMetrics
}

const BAND_TEXT_CLASS: Record<EfficiencyBand, string> = {
  good: "text-system-green",
  ok: "text-label-tertiary",
  bad: "text-system-orange",
}

const BAND_SEGMENT_CLASS: Record<EfficiencyBand, string> = {
  good: "bg-system-green/50",
  ok: "bg-separator",
  bad: "bg-system-orange/50",
}

type MetricKey = "costPerMTok" | ShareMetricKey
type ShareMetricKey = "realWorkShare" | "rewriteShare" | "carryShare"

const METRIC_SUMMARY: Record<MetricKey, string[]> = {
  costPerMTok: [
    "The avg cost for each million tokens of context growth and output in this session.",
  ],
  realWorkShare: ["The share of the session's cost spent on fresh input and output."],
  rewriteShare: [
    "The share of the session's cost spent rehydrating the cache with old context.",
  ],
  carryShare: ["The share of the session's cost spent resending cached context."],
}

const METRIC_GUIDANCE: Record<MetricKey, string[]> = {
  costPerMTok: [
    "Craft tight workflows, and use cheaper models when they're good enough.",
    "Pay attention to which of these next three categories are out of band.",
  ],
  realWorkShare: [
    "Maximise this by minimising the other two dimesions.",
    "Usually a focused and short session will be best here.",
  ],
  rewriteShare: [
    "If you have long breaks in your session, the cache expires, so manually compact immediately after you return.",
    "Model or reasoning changes also cause rewrites.",
  ],
  carryShare: [
    "Keep context down by actively compacting when the session gets too big.",
    "It also can be handy to rewind if the session's gone in the wrong direction.",
    "Or even ask the agent for a summary then start a clean session with that.",
  ],
}

interface ShareRow {
  key: ShareMetricKey
  label: string
  fillColor: string
}

const SHARE_ROWS: ShareRow[] = [
  {
    key: "realWorkShare",
    label: "Real Work %",
    fillColor: "bg-system-blue/50",
  },
  {
    key: "rewriteShare",
    label: "Rewrite Waste %",
    fillColor: "bg-system-indigo/50",
  },
  {
    key: "carryShare",
    label: "Carry %",
    fillColor: "bg-system-gold/50",
  },
]

interface ShareSegment extends ShareRow {
  metric: EfficiencyMetric
  start: number
  width: number
  displayPercent: string
}

function shareSegments(metrics: EfficiencyMetrics): ShareSegment[] {
  const rows = SHARE_ROWS.flatMap((row) => {
    const metric = metrics[row.key]
    return metric ? [{ ...row, metric }] : []
  })
  const total = rows.reduce((sum, row) => sum + row.metric.value, 0)
  if (rows.length !== SHARE_ROWS.length || total <= 0) return []

  let start = 0
  const segments = rows.map((row) => {
    const width = row.metric.value / total
    const segment = { ...row, start, width, displayPercent: "" }
    start += width
    return segment
  })

  // Round the shares as one composition so the displayed values total 100%.
  const smallestPercent = Math.min(
    ...segments.filter((segment) => segment.width > 0).map((segment) => segment.width * 100),
  )
  const decimalPlaces = Math.min(6, Math.max(0, 1 - Math.floor(Math.log10(smallestPercent))))
  const unitsPerPercent = 10 ** decimalPlaces
  const totalUnits = 100 * unitsPerPercent
  const exactUnits = segments.map((segment) => segment.width * totalUnits)
  const roundedUnits = exactUnits.map(Math.floor)
  const remainingUnits = totalUnits - roundedUnits.reduce((sum, value) => sum + value, 0)
  const remainderOrder = exactUnits
    .map((value, index) => ({ index, remainder: value - roundedUnits[index]! }))
    .sort((a, b) => b.remainder - a.remainder)

  for (let index = 0; index < remainingUnits; index += 1) {
    roundedUnits[remainderOrder[index]!.index]! += 1
  }

  return segments.map((segment, index) => ({
    ...segment,
    displayPercent: `${Number((roundedUnits[index]! / unitsPerPercent).toFixed(decimalPlaces))}%`,
  }))
}

function CostThermometer({ value, profile }: { value: number; profile: EfficiencyProfile }) {
  const scale = efficiencyThermometer(value, "costPerMTok", profile)
  return (
    <div
      className="relative flex h-full items-center"
      data-testid="thermometer-costPerMTok"
      data-position={scale.position.toFixed(3)}
      aria-hidden
    >
      {scale.segments.map((segment, index) => (
        <span
          key={index}
          className={cn(
            "h-[7px] flex-1",
            index === 0 && "rounded-s-full",
            index === scale.segments.length - 1 && "rounded-e-full",
            BAND_SEGMENT_CLASS[segment],
          )}
        />
      ))}
      <span
        className="absolute h-[10px] w-1.5 -translate-x-1/2 rounded-full border border-label bg-surface opacity-80 dark:bg-separator"
        style={{ left: `${scale.position * 100}%` }}
      />
    </div>
  )
}

function ShareSegmentBar({ segment }: { segment: ShareSegment }) {
  const segmentEnd = segment.start + segment.width

  const isLeftHandSegment = segment.start === 0
  const isRightHandSegment = segmentEnd === 1

  return (
    <div
      className="relative h-1.5"
      data-testid={`share-segment-${segment.key}`}
      data-start={segment.start.toFixed(3)}
      data-width={segment.width.toFixed(3)}
      aria-hidden
    >
      {!isLeftHandSegment && (
        <span
          className={cn(
            "absolute inset-y-0 border border-separator border-dotted rounded-s-full",
          )}
          style={{ left: 0, width: `${segment.start * 100}%` }}
        />
      )}
      <span
        className={cn(
          "absolute -inset-y-[1px]",
          segment.fillColor,
          isLeftHandSegment && "rounded-s-full",
          isRightHandSegment && "rounded-e-full",
        )}
        style={{ left: `${segment.start * 100}%`, width: `${segment.width * 100}%` }}
      />
      {!isRightHandSegment && (
        <span
          className={cn(
            "absolute inset-y-0 border border-separator border-dotted rounded-e-full",
          )}
          style={{ left: `${segmentEnd * 100}%`, width: `${(1 - segmentEnd) * 100}%` }}
        />
      )}
    </div>
  )
}

function MetricGuidance({
  metricKey,
  profile,
}: {
  metricKey: MetricKey
  profile: EfficiencyProfile
}) {
  return (
    <div className="space-y-1 text-pretty type-footnote text-label-secondary border-b border-x border-separator pb-3 px-3 rounded-lg">
      {METRIC_SUMMARY[metricKey].map((sentence) => (
        <p key={sentence} className="font-bold">
          {sentence}
        </p>
      ))}
      {efficiencyThresholdGuidance(metricKey, profile).map((sentence) => (
        <p key={sentence} className="italic">
          {sentence}
        </p>
      ))}
      {METRIC_GUIDANCE[metricKey].map((sentence) => (
        <p key={sentence}>{sentence}</p>
      ))}
    </div>
  )
}

function EfficiencyRowLine({
  label,
  metric,
  metricKey,
  profile,
  children,
  formattedValue,
  open,
  onToggle,
  separated = false,
}: {
  label: string
  metric: EfficiencyMetric | null
  metricKey: MetricKey
  profile: EfficiencyProfile
  children?: ReactNode
  formattedValue?: string
  open: boolean
  onToggle: () => void
  separated?: boolean
}) {
  const bodyId = useId()

  return (
    <>
      <button
        type="button"
        aria-label={`${label} details`}
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={onToggle}
        className="col-span-4 grid cursor-pointer! grid-cols-subgrid items-center gap-x-3 -mx-1 px-1 py-1 rounded-control text-left type-caption transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover active:transform-none active:opacity-100"
      >
        <span className="text-label-tertiary">{label}</span>
        {metric ? children : <span />}
        <span className="text-center text-label tabular-nums">
          {metric
            ? (formattedValue ??
              (metricKey === "costPerMTok" ? formatCostPerMTok : formatSharePercent)(
                metric.value,
              ))
            : "—"}
        </span>
        {metric && (
          <span
            className={cn(
              "type-caption text-center whitespace-nowrap",
              BAND_TEXT_CLASS[metric.band],
            )}
          >
            {efficiencyBandWord(metric.band, metricKey)}
          </span>
        )}
      </button>

      {open && (
        <div
          id={bodyId}
          role="region"
          aria-label={`${label} guidance`}
          className="col-span-full px-1 pb-2"
        >
          <MetricGuidance metricKey={metricKey} profile={profile} />
        </div>
      )}

      {separated && (
        <div className="col-span-full border-b border-separator border-dashed my-1" />
      )}
    </>
  )
}

export function EfficiencyBreakdown({ metrics }: EfficiencyBreakdownProps) {
  const segments = shareSegments(metrics)
  const [openMetric, setOpenMetric] = useState<MetricKey | null>(null)

  const toggleMetric = (metricKey: MetricKey) => {
    setOpenMetric((current) => (current === metricKey ? null : metricKey))
  }

  if (!metrics.costPerMTok) {
    return null
  }

  return (
    <div className="grid grid-cols-[max-content_1fr_max-content_max-content] gap-y-1">
      <EfficiencyRowLine
        label="$/MTok"
        metric={metrics.costPerMTok}
        metricKey="costPerMTok"
        profile={metrics.profile}
        open={openMetric === "costPerMTok"}
        onToggle={() => toggleMetric("costPerMTok")}
        separated
      >
        <CostThermometer value={metrics.costPerMTok.value} profile={metrics.profile} />
      </EfficiencyRowLine>
      {segments.map((segment) => (
        <EfficiencyRowLine
          key={segment.key}
          label={segment.label}
          metric={segment.metric}
          metricKey={segment.key}
          profile={metrics.profile}
          formattedValue={segment.displayPercent}
          open={openMetric === segment.key}
          onToggle={() => toggleMetric(segment.key)}
        >
          <ShareSegmentBar segment={segment} />
        </EfficiencyRowLine>
      ))}
      {segments.length === 0 &&
        SHARE_ROWS.map((row) => (
          <EfficiencyRowLine
            key={row.key}
            label={row.label}
            metric={metrics[row.key]}
            metricKey={row.key}
            profile={metrics.profile}
            open={openMetric === row.key}
            onToggle={() => toggleMetric(row.key)}
          />
        ))}
    </div>
  )
}
