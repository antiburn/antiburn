import { Text } from "recharts"

/**
 * Shared label geometry for the session-analysis charts: the pill a label
 * draws on, the axis text styles, and the greedy min-gap rule that keeps two
 * nearby marks from drawing overlapping labels.
 */

/** Horizontal padding inside a label pill. */
const PILL_PAD_X = 5
/** Vertical padding above and below the label text inside its pill. */
const PILL_PAD_Y = 2
/** Mean glyph width at the label size, for sizing a pill to its text. */
const PILL_CHAR_WIDTH = 6.1

/**
 * Where the text sits relative to the point recharts computes for each label
 * position. Recharts gives a custom label the point but not the anchors, so
 * the chart states the same anchors the built-in label uses.
 */
export const LABEL_ANCHORS: Record<
  string,
  { textAnchor: "start" | "middle"; verticalAnchor: "start" | "end" }
> = {
  insideTop: { textAnchor: "middle", verticalAnchor: "start" },
  top: { textAnchor: "middle", verticalAnchor: "end" },
}

/* Recharts states a label's geometry as string-or-number, so the pill takes
   the same shape and converts once. */
interface PillLabelProps {
  x?: string | number | undefined
  y?: string | number | undefined
  dy?: string | number | undefined
  fontSize?: string | number | undefined
  fill?: string | undefined
  value?: string | number | boolean | null | undefined
  position?: unknown
}

/**
 * A label drawn inside the plot, on a translucent pill. The pill is the
 * opposite of the surface, so the text stays legible over the fill, the
 * line, and the marker bars it can land on.
 */
export function PillLabel({
  x,
  y,
  dy = 0,
  fontSize = 11,
  fill,
  value,
  position = "insideTop",
}: PillLabelProps) {
  const originX = Number(x)
  const originY = Number(y)
  const offsetY = Number(dy)
  const size = Number(fontSize)
  if (
    value == null ||
    value === false ||
    !Number.isFinite(originX) ||
    !Number.isFinite(originY)
  ) {
    return null
  }
  const text = String(value)
  const anchors =
    (typeof position === "string" ? LABEL_ANCHORS[position] : undefined) ??
    LABEL_ANCHORS.insideTop!
  const width = text.length * PILL_CHAR_WIDTH + PILL_PAD_X * 2
  const height = size + PILL_PAD_Y * 2
  const left = anchors.textAnchor === "start" ? originX - PILL_PAD_X : originX - width / 2
  // A "start" anchor puts the text's top edge on the point, an "end" anchor
  // puts its bottom edge there.
  const top =
    anchors.verticalAnchor === "start" ? originY - PILL_PAD_Y : originY + PILL_PAD_Y - height
  return (
    <g>
      <rect
        x={left}
        y={top + offsetY}
        width={width}
        height={height}
        rx={height / 2}
        fill="var(--color-chart-label-pill)"
      />
      <Text
        x={originX}
        y={originY + offsetY}
        textAnchor={anchors.textAnchor}
        verticalAnchor={anchors.verticalAnchor}
        fontSize={size}
        fill={fill}
      >
        {text}
      </Text>
    </g>
  )
}

/* Band label text, drawn inside the plot. The size matches the caption
   step of the type scale, which is the legibility floor. */
export const AXIS_LABEL = {
  fontSize: 11,
  fill: "var(--color-label-tertiary)",
  content: PillLabel,
}
/* Axis tick text, drawn outside the plot in the caption grey. */
export const AXIS_TICK = { fontSize: 11, fill: "var(--color-label-tertiary)" }

/** Nearer than this fraction of the x-domain, two labels would collide. */
export const LABEL_MIN_GAP_FRACTION = 0.18

/**
 * Greedy min-gap labelling: walk `indices` in ascending order and keep one
 * whenever it sits at least `LABEL_MIN_GAP_FRACTION` of the x-domain past the
 * last kept index. `domainLength` is the series length the marks share, so
 * the gap scales with the same x-axis every mark draws on.
 */
export function labeledIndices(indices: readonly number[], domainLength: number): Set<number> {
  const minGap = Math.max(1, domainLength - 1) * LABEL_MIN_GAP_FRACTION
  let lastLabeled = Number.NEGATIVE_INFINITY
  const labeled = new Set<number>()
  for (const index of indices) {
    if (index - lastLabeled >= minGap) {
      labeled.add(index)
      lastLabeled = index
    }
  }
  return labeled
}
