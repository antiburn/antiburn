import { cn } from "../../../lib/cn"
import {
  efficiencyBandWord,
  efficiencyThresholdGuidance,
  efficiencyThermometer,
  formatCostPerMTok,
  type EfficiencyBand,
  type EfficiencyMetric,
  type EfficiencyMetrics,
  type EfficiencyProfile,
} from "../../../lib/presentation/sessionEfficiency"
import { Tooltip } from "../../presentation/Tooltip"

export interface EfficiencyBreakdownProps {
  metrics: EfficiencyMetrics
}

/* The band word, as a small caption tag. Teal is the good verdict and red
   the bad one; the middle band stays neutral, because it is not a verdict. */
const BAND_TAG_CLASS: Record<EfficiencyBand, string> = {
  good: "bg-share-work/15 text-share-work-text",
  ok: "bg-surface-secondary text-label-secondary",
  bad: "bg-share-waste/15 text-share-waste-text",
}

/* The fill for one band of the cost ruler. The ruler shows all three bands
   at full width, so these are the fixed backdrop the needle moves across.
   The bad band is hatched, so it reads as "out of bounds" and not as a
   second solid. */
const BAND_SEGMENT_CLASS: Record<EfficiencyBand, string> = {
  good: "bg-share-work",
  ok: "bg-surface-tertiary",
  bad: "efficiency-ruler-hatch text-share-waste",
}

type MetricKey = "costPerMTok" | ShareMetricKey
type ShareMetricKey = "realWorkShare" | "rewriteShare" | "carryShare"

const METRIC_SUMMARY: Record<MetricKey, string[]> = {
  costPerMTok: [
    "The average cost for each million tokens of context growth and output in this session.",
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
    "Maximise this by minimising the other two dimensions.",
    "Usually a focused and short session will be best here.",
  ],
  rewriteShare: [
    "If you have long breaks in your session, the cache expires, so manually compact immediately after you return.",
    "Model or reasoning changes also cause rewrites.",
  ],
  carryShare: [
    "Keep context down by actively compacting when the session gets too big.",
    "It can also be handy to rewind if the session's gone in the wrong direction.",
    "Or even ask the agent for a summary then start a clean session with that.",
  ],
}

interface ShareRow {
  key: ShareMetricKey
  label: string
  /* The slice keeps one color for the life of the feature, so a reader
     recognizes it between sessions. Real work runs teal and rewrite waste
     red, which reads in the same language as the cost track above. Carry
     stays neutral, because carry is neither good nor bad. */
  inkClassName: string
}

const SHARE_ROWS: ShareRow[] = [
  {
    key: "realWorkShare",
    label: "Real Work %",
    inkClassName: "bg-share-work",
  },
  {
    key: "rewriteShare",
    label: "Rewrite Waste %",
    inkClassName: "bg-share-waste",
  },
  {
    key: "carryShare",
    label: "Carry %",
    inkClassName: "bg-share-carry",
  },
]

interface ShareSegment extends ShareRow {
  metric: EfficiencyMetric
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

  const segments = rows.map((row) => ({
    ...row,
    width: row.metric.value / total,
    displayPercent: "",
  }))

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

/**
 * The $/MTok scale as a ruler. The strip shows the good, middle, and bad
 * bands at their fixed thirds, the tick values under it name the band edges
 * in dollars, and the needle marks where this session sits. The ruler never
 * changes length.
 *
 * A fill that grows to the reading was the wrong form for this metric: a low
 * $/MTok is a good result, so a good session drew almost nothing. The question
 * here is "where does this sit between good and bad", which a fixed scale with
 * a moving mark answers directly.
 */
function CostScaleBar({ value, profile }: { value: number; profile: EfficiencyProfile }) {
  const scale = efficiencyThermometer(value, "costPerMTok", profile)
  const last = scale.ticks.length - 1
  return (
    <div
      data-testid="thermometer-costPerMTok"
      data-position={scale.position.toFixed(3)}
      aria-hidden
    >
      <div className="relative flex h-1.5 items-center">
        {scale.segments.map((band, index) => (
          <span
            key={index}
            data-testid={`cost-band-${band}`}
            className={cn(
              "h-full flex-1",
              index === 0 && "rounded-s-sm",
              index === scale.segments.length - 1 && "rounded-e-sm",
              BAND_SEGMENT_CLASS[band],
            )}
          />
        ))}
        {/* The ring separates the needle from whichever band it lands on, so
            the mark stays legible over the teal and the hatch alike. */}
        <span
          data-testid="cost-needle"
          className="absolute -inset-y-1 w-[3px] -translate-x-1/2 rounded-full bg-label ring-2 ring-surface"
          style={{ left: `${scale.position * 100}%` }}
        />
      </div>
      <div className="relative mt-0.5 h-4">
        {scale.ticks.map((label, index) => (
          <span
            key={label}
            className={cn(
              "absolute top-0 flex flex-col type-caption text-label-tertiary tabular-nums",
              index === 0 && "items-start",
              index === last && "-translate-x-full items-end",
              index > 0 && index < last && "-translate-x-1/2 items-center",
            )}
            style={{ left: `${(index / last) * 100}%` }}
          >
            <span className="h-1 w-px bg-separator" />
            {label}
          </span>
        ))}
      </div>
    </div>
  )
}

/**
 * The three shares as one composition. They are parts of a single whole, so
 * they draw as one track: each run takes its slice of the width, in that
 * slice's own color. Three separate meters hid the only fact that matters,
 * which is how the session divided its cost.
 */
function CompositionTrack({ segments }: { segments: ShareSegment[] }) {
  return (
    <div
      data-testid="efficiency-composition"
      aria-hidden
      className="flex h-1 gap-px overflow-hidden rounded-full bg-surface-secondary"
    >
      {segments.map((segment) => (
        <span
          key={segment.key}
          data-testid={`composition-run-${segment.key}`}
          data-width={segment.width.toFixed(3)}
          className={cn("h-full", segment.inkClassName)}
          style={{ width: `${segment.width * 100}%` }}
        />
      ))}
    </div>
  )
}

/**
 * The guidance for one reading, as the body of its tooltip. It names what the
 * reading measures, states the band it should sit in, and says what to change.
 *
 * The guidance lives in a tooltip so the block stays the height of its
 * readings. A panel that grows and shrinks under the rows moves everything
 * below it every time the pointer crosses a row.
 */
function MetricGuidance({
  metricKey,
  profile,
}: {
  metricKey: MetricKey
  profile: EfficiencyProfile
}) {
  return (
    <div className="space-y-1 text-pretty">
      {METRIC_SUMMARY[metricKey].map((sentence) => (
        <p key={sentence} className="text-label">
          {sentence}
        </p>
      ))}
      {efficiencyThresholdGuidance(metricKey, profile).map((sentence) => (
        <p key={sentence} className="text-label-secondary">
          {sentence}
        </p>
      ))}
      {METRIC_GUIDANCE[metricKey].map((sentence) => (
        <p key={sentence} className="text-label-secondary">
          {sentence}
        </p>
      ))}
    </div>
  )
}

const ROW_CLASS =
  "-mx-1.5 rounded-control px-1.5 py-1 type-body transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover focus-visible:bg-surface-hover"

const BAND_TAG_BASE = "rounded px-1.5 py-px type-caption font-medium whitespace-nowrap"

/**
 * The $/MTok reading: label and figure on one baseline, with its VU meter
 * underneath. This metric is not part of the composition, so it keeps the
 * meter the other readings gave up.
 */
function CostRowLine({
  metric,
  profile,
}: {
  metric: EfficiencyMetric
  profile: EfficiencyProfile
}) {
  return (
    <Tooltip label={<MetricGuidance metricKey="costPerMTok" profile={profile} />} delayMs={150}>
      <div className={cn(ROW_CLASS, "block")} tabIndex={0} data-testid="cost-row">
        <span className="flex items-baseline justify-between gap-2 pb-1">
          <span className="truncate text-label-secondary">$/MTok</span>
          <span className="flex shrink-0 items-baseline gap-2">
            <span className="text-label tabular-nums">{formatCostPerMTok(metric.value)}</span>
            <span className={cn(BAND_TAG_BASE, BAND_TAG_CLASS[metric.band])}>
              {efficiencyBandWord(metric.band, "costPerMTok")}
            </span>
          </span>
        </span>
        <CostScaleBar value={metric.value} profile={profile} />
      </div>
    </Tooltip>
  )
}

/**
 * One line of the composition legend: the slice's color, its name, its share,
 * and the band word. The run in the track above carries the size, so the row
 * carries no bar of its own.
 */
function ShareRowLine({
  segment,
  profile,
}: {
  segment: ShareSegment
  profile: EfficiencyProfile
}) {
  return (
    <Tooltip label={<MetricGuidance metricKey={segment.key} profile={profile} />} delayMs={150}>
      <div
        data-testid={`share-row-${segment.key}`}
        className={cn(ROW_CLASS, "flex items-baseline gap-2")}
        tabIndex={0}
      >
        <span
          aria-hidden
          className={cn("size-2 shrink-0 self-center rounded-full", segment.inkClassName)}
        />
        <span className="min-w-0 flex-1 truncate text-label-secondary">{segment.label}</span>
        <span className="shrink-0 text-label tabular-nums">{segment.displayPercent}</span>
        <span className="flex w-12 shrink-0 justify-end">
          <span className={cn(BAND_TAG_BASE, BAND_TAG_CLASS[segment.metric.band])}>
            {efficiencyBandWord(segment.metric.band, segment.key)}
          </span>
        </span>
      </div>
    </Tooltip>
  )
}

/** A share with no reading yet: the name and a dash, on the same grid. */
function ShareRowPlaceholder({ row }: { row: ShareRow }) {
  return (
    <div className={cn(ROW_CLASS, "flex items-baseline gap-2")}>
      <span
        aria-hidden
        className={cn("size-2 shrink-0 self-center rounded-full bg-surface-tertiary")}
      />
      <span className="min-w-0 flex-1 truncate text-label-secondary">{row.label}</span>
      <span className="shrink-0 text-label tabular-nums">—</span>
      <span className="w-12 shrink-0" />
    </div>
  )
}

export function EfficiencyBreakdown({ metrics }: EfficiencyBreakdownProps) {
  const segments = shareSegments(metrics)

  if (!metrics.costPerMTok) {
    return null
  }

  return (
    <div className="flex flex-col" data-testid="efficiency-block">
      <CostRowLine metric={metrics.costPerMTok} profile={metrics.profile} />
      <div className="my-3 border-b border-dashed border-separator" />
      {segments.length > 0 ? (
        <>
          <CompositionTrack segments={segments} />
          <div className="mt-2 flex flex-col">
            {segments.map((segment) => (
              <ShareRowLine key={segment.key} segment={segment} profile={metrics.profile} />
            ))}
          </div>
        </>
      ) : (
        <div className="flex flex-col">
          {SHARE_ROWS.map((row) => (
            <ShareRowPlaceholder key={row.key} row={row} />
          ))}
        </div>
      )}
    </div>
  )
}
