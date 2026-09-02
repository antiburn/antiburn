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
  type EfficiencyShareZone,
} from "../../../lib/presentation/sessionEfficiency"
import { Tooltip } from "../../presentation/Tooltip"
import {
  METER_INK,
  SegmentedMeter,
  type MeterInk,
  type MeterZone,
} from "../../ui/SegmentedMeter"

export interface EfficiencyBreakdownProps {
  metrics: EfficiencyMetrics
}

/* Red marks a bad reading, in the word, because orange is the brand color
   here and cannot also mean trouble. */
const BAND_TEXT_CLASS: Record<EfficiencyBand, string> = {
  good: "text-system-green",
  ok: "text-context-warning",
  bad: "text-system-red-text",
}

/* The efficiency meters draw as VU meters, on the shared meter palette:
   the track holds a fixed zone for each band, and the fill lights whichever
   zones the reading crosses. The zones always run left to
   right along the metric's own scale, so orange marks the good end of every
   meter and red the bad end, whichever end the fill comes from. */
const BAND_INK: Record<EfficiencyBand, MeterInk> = {
  good: METER_INK.normal,
  ok: METER_INK.warning,
  bad: METER_INK.critical,
}

/** Ink each band zone for the segmented meter. */
function meterZones(zones: EfficiencyShareZone[]): MeterZone[] {
  return zones.map((zone) => ({ from: zone.from, ...BAND_INK[zone.band] }))
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
     recognizes it between sessions. The colors come from the composition
     ramp, which no meaning color uses: the band word carries the judgment,
     and a slice must not look like a verdict. */
  inkClassName: string
}

const SHARE_ROWS: ShareRow[] = [
  { key: "realWorkShare", label: "Real Work %", inkClassName: "bg-share-work" },
  { key: "rewriteShare", label: "Rewrite Waste %", inkClassName: "bg-share-waste" },
  { key: "carryShare", label: "Carry %", inkClassName: "bg-share-carry" },
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
 * The $/MTok scale in the usage meter's silhouette. The thermometer splits
 * the track into thirds, one per band, so the zones sit at fixed positions
 * and the fill lights up to the reading. The notch marks the same position
 * on the track.
 */
function CostScaleBar({ value, profile }: { value: number; profile: EfficiencyProfile }) {
  const scale = efficiencyThermometer(value, "costPerMTok", profile)
  const zones = meterZones(scale.segments.map((band, index) => ({ from: index / 3, band })))
  return (
    <div
      data-testid="thermometer-costPerMTok"
      data-position={scale.position.toFixed(3)}
      aria-hidden
    >
      <SegmentedMeter
        percent={scale.position * 100}
        expectedFraction={scale.position}
        zones={zones}
      />
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
      className="flex h-3 gap-px overflow-hidden rounded-full bg-surface-secondary"
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
  "-mx-1 rounded-control px-1 py-1.5 type-body transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover focus-visible:bg-surface-hover"

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
            <span className={cn("whitespace-nowrap", BAND_TEXT_CLASS[metric.band])}>
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
        <span
          className={cn(
            "w-10 shrink-0 text-right whitespace-nowrap",
            BAND_TEXT_CLASS[segment.metric.band],
          )}
        >
          {efficiencyBandWord(segment.metric.band, segment.key)}
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
      <span className="w-10 shrink-0" />
    </div>
  )
}

export function EfficiencyBreakdown({ metrics }: EfficiencyBreakdownProps) {
  const segments = shareSegments(metrics)

  if (!metrics.costPerMTok) {
    return null
  }

  return (
    <div className="flex flex-col">
      <CostRowLine metric={metrics.costPerMTok} profile={metrics.profile} />
      <div className="my-2 border-b border-dashed border-separator" />
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
