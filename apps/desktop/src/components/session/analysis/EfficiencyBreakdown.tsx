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
  /**
   * Which part to draw. The cost scale lives on the Cost tab and the
   * composition under the context chart. Omit it to draw both.
   */
  section?: "cost" | "composition"
}

/* The fill for one band of the cost scale. The three bands are a fixed
   qualitative backdrop, so they step through three greys and carry no
   verdict colour. The measure and the words say where the reading sits. */
const BAND_SEGMENT_CLASS: Record<EfficiencyBand, string> = {
  good: "bg-surface-secondary",
  ok: "bg-surface-tertiary",
  bad: "bg-separator",
}

/* The band word under a row. One quiet ink for every band: the word is the
   verdict, so it needs no colour to carry it. */
const BAND_WORD_CLASS = "type-caption text-label-tertiary whitespace-nowrap"

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
    "The Context tab shows which of work, rewrite, and carry is out of band.",
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

/** Neutral advice replaces text that requires a benchmark. */
const NEUTRAL_GUIDANCE_OVERRIDES: Partial<Record<MetricKey, string[]>> = {
  costPerMTok: [
    "Craft tight workflows, and use cheaper models when they're good enough.",
    "The Context tab shows how spend splits across work, rewrite, and carry.",
  ],
}

interface ShareRow {
  key: ShareMetricKey
  label: string
  /* The slice keeps one color for the life of the feature, so a reader
     recognizes it between sessions. Real work takes the measure blue, the
     same blue the cost scale draws its reading in. Rewrite waste takes the
     mid neutral and carry the brand orange. */
  inkClassName: string
}

const SHARE_ROWS: ShareRow[] = [
  {
    key: "realWorkShare",
    label: "Real Work %",
    inkClassName: "bg-measure",
  },
  {
    key: "rewriteShare",
    label: "Rewrite Waste %",
    inkClassName: "bg-share-carry",
  },
  {
    key: "carryShare",
    label: "Carry %",
    inkClassName: "bg-brand-tint",
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
 * The $/MTok scale as a bullet graph. Three grey bands mark good, middle,
 * and bad at fixed thirds, and a thin blue measure runs from zero to this
 * session's reading. Under each band sit its word and its dollar range, so
 * the scale labels itself and the reading needs no tag. The band steps and
 * the ranges already mark every edge, so no separate target line is drawn.
 *
 * A fill that grows to the reading was the wrong form for this metric: a low
 * $/MTok is a good result, so a good session drew almost nothing. The question
 * here is "where does this sit between good and bad", which a fixed scale with
 * a measure answers directly.
 */
function CostScaleBar({
  metric,
  profile,
}: {
  metric: EfficiencyMetric
  profile: EfficiencyProfile
}) {
  const scale = efficiencyThermometer(metric.value, "costPerMTok", profile)
  const [, low, high] = scale.ticks
  // The outer ranges show both middle boundaries and leave space for the floating section picker.
  const ranges = [`under ${low}`, null, `over ${high}`]
  const last = scale.segments.length - 1
  return (
    <div
      data-testid="thermometer-costPerMTok"
      data-position={scale.position.toFixed(3)}
      aria-hidden
    >
      <div className="relative flex h-6">
        {scale.segments.map((band, index) => (
          <span
            key={band}
            data-testid={`cost-band-${band}`}
            className={cn(
              "h-full flex-1",
              index === 0 && "rounded-s-sm",
              index === last && "rounded-e-sm",
              BAND_SEGMENT_CLASS[band],
            )}
          />
        ))}
        <span
          data-testid="cost-measure"
          className="absolute inset-y-1 left-0 rounded-e-sm bg-measure"
          style={{ width: `${scale.position * 100}%` }}
        />
      </div>
      <div className="mt-1 flex">
        {scale.segments.map((band, index) => (
          <span
            key={band}
            data-testid={`cost-band-word-${band}`}
            data-current={band === metric.band || undefined}
            className={cn(
              "flex flex-1 flex-col text-label-tertiary tabular-nums",
              "type-body",
              index === 0 && "items-start",
              index === last && "items-end",
              index > 0 && index < last && "items-center",
            )}
          >
            <span className={cn("font-medium", band === metric.band && "text-label")}>
              {efficiencyBandWord(band, "costPerMTok")}
            </span>
            {ranges[index] && <span>{ranges[index]}</span>}
          </span>
        ))}
      </div>
    </div>
  )
}

/** The track shows each share as part of the total cost. */
function CompositionTrack({ segments }: { segments: ShareSegment[] }) {
  return (
    <div
      data-testid="efficiency-composition"
      data-height="bar"
      aria-hidden
      className="flex gap-px overflow-hidden bg-surface-secondary h-6 rounded-control"
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

/** The tooltip explains the reading, its expected band, and the suggested changes. */
function MetricGuidance({
  metricKey,
  guidanceProfile,
}: {
  metricKey: MetricKey
  guidanceProfile: EfficiencyProfile | null
}) {
  const metricGuidance =
    guidanceProfile === null
      ? (NEUTRAL_GUIDANCE_OVERRIDES[metricKey] ?? METRIC_GUIDANCE[metricKey])
      : METRIC_GUIDANCE[metricKey]
  const advice = [...efficiencyThresholdGuidance(metricKey, guidanceProfile), ...metricGuidance]
  return (
    <div className="space-y-1 text-pretty">
      {METRIC_SUMMARY[metricKey].map((sentence) => (
        <p key={sentence} className="text-label">
          {sentence}
        </p>
      ))}
      {advice.map((sentence) => (
        <p key={sentence} className="text-label-secondary">
          {sentence}
        </p>
      ))}
    </div>
  )
}

const ROW_CLASS =
  "-mx-1.5 rounded-control px-1.5 py-1 type-body transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover focus-visible:bg-surface-hover"

/** The hero figure opens guidance in a tooltip. The scale shows the bands below it. */
function CostRowLine({
  metric,
  profile,
  guidanceProfile,
}: {
  metric: EfficiencyMetric
  profile: EfficiencyProfile
  guidanceProfile: EfficiencyProfile | null
}) {
  return (
    <div className="type-body" data-testid="cost-row">
      <Tooltip
        label={<MetricGuidance metricKey="costPerMTok" guidanceProfile={guidanceProfile} />}
        side="bottom"
        delayMs={150}
      >
        <div
          data-testid="cost-hero"
          tabIndex={0}
          className={cn(ROW_CLASS, "my-2 flex flex-wrap items-center gap-x-4 gap-y-1 pb-3")}
        >
          <span
            className={cn(
              "type-display font-semibold! tabular-nums",
              metric.band === "bad" ? "text-brand" : "text-label",
            )}
          >
            {formatCostPerMTok(metric.value)}
          </span>
          <span className="flex flex-col type-callout">
            <span className="font-semibold text-label">$/MTOK</span>
            <span className="text-label-secondary">
              per million tokens of work: context growth and output only
            </span>
          </span>
        </div>
      </Tooltip>
      <CostScaleBar metric={metric} profile={profile} />
    </div>
  )
}

/**
 * One composition line shows the slice color, name, share, and band word.
 * The track shows the size, so the row has no bar.
 */
function ShareRowLine({
  segment,
  guidanceProfile,
}: {
  segment: ShareSegment
  guidanceProfile: EfficiencyProfile | null
}) {
  return (
    <Tooltip
      label={<MetricGuidance metricKey={segment.key} guidanceProfile={guidanceProfile} />}
      delayMs={150}
    >
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
        <span className={cn("w-12 shrink-0 text-end", BAND_WORD_CLASS)}>
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
        className="size-2 shrink-0 self-center rounded-full bg-surface-tertiary"
      />
      <span className="min-w-0 flex-1 truncate text-label-secondary">{row.label}</span>
      <span className="shrink-0 text-label tabular-nums">—</span>
      <span className="w-12 shrink-0" />
    </div>
  )
}

export function EfficiencyBreakdown({ metrics, section }: EfficiencyBreakdownProps) {
  const segments = shareSegments(metrics)
  const showCost = section !== "composition"
  const showComposition = section !== "cost"

  if (!metrics.costPerMTok) {
    return null
  }

  return (
    <div className="flex flex-col" data-testid="efficiency-block">
      {showCost && (
        <CostRowLine
          metric={metrics.costPerMTok}
          profile={metrics.profile}
          guidanceProfile={metrics.guidanceProfile}
        />
      )}
      {showCost && showComposition && (
        <div className="my-3 border-b border-dashed border-separator" />
      )}
      {!showComposition ? null : segments.length > 0 ? (
        <>
          <CompositionTrack segments={segments} />
          <div data-testid="composition-legend" className="flex flex-col mt-3">
            {segments.map((segment) => (
              <ShareRowLine
                key={segment.key}
                segment={segment}
                guidanceProfile={metrics.guidanceProfile}
              />
            ))}
          </div>
        </>
      ) : (
        <div data-testid="composition-legend" className="flex flex-col">
          {SHARE_ROWS.map((row) => (
            <ShareRowPlaceholder key={row.key} row={row} />
          ))}
        </div>
      )}
    </div>
  )
}
