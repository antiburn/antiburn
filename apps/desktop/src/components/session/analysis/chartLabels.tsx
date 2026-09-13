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
/** Gap a rotated label keeps between its near edge and the line it names. */
const VERTICAL_LABEL_LINE_GAP = 3

/**
 * Where the text sits relative to the point recharts computes for each label
 * position. Recharts gives a custom label the point but not the anchors, so
 * the chart states the same anchors the built-in label uses. `angle` rotates
 * the pill and text about that point, for a label that reads bottom to top.
 */
const LABEL_ANCHORS: Record<
  string,
  { textAnchor: "start" | "middle" | "end"; verticalAnchor: "start" | "end"; angle?: number }
> = {
  insideTop: { textAnchor: "middle", verticalAnchor: "start" },
  top: { textAnchor: "middle", verticalAnchor: "end" },
  insideTopLeftVertical: { textAnchor: "end", verticalAnchor: "end", angle: -90 },
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
  /** Recharts' own label rotation prop. A non-zero value selects the vertical anchor. */
  angle?: string | number | undefined
}

/**
 * A label drawn inside the plot, on a translucent pill. The pill is the
 * opposite of the surface, so the text stays legible over the fill, the
 * line, and the marker bars it can land on. A non-zero `angle` rotates the
 * pill and text about the label point, so the label reads bottom to top
 * along the near side of a line instead of sitting flat above it.
 */
function PillLabel({
  x,
  y,
  dy = 0,
  fontSize = 11,
  fill,
  value,
  position = "insideTop",
  angle,
}: PillLabelProps) {
  const rawX = Number(x)
  const rawY = Number(y)
  const offsetY = Number(dy)
  const size = Number(fontSize)
  const rotation = Number(angle) || 0
  if (value == null || value === false || !Number.isFinite(rawX) || !Number.isFinite(rawY)) {
    return null
  }
  const text = String(value)
  const anchors =
    (rotation !== 0 ? LABEL_ANCHORS.insideTopLeftVertical : undefined) ??
    (typeof position === "string" ? LABEL_ANCHORS[position] : undefined) ??
    LABEL_ANCHORS.insideTop!
  // A rotated label pivots on its point. Move the point off the line, so the
  // upright text stays clear of the line.
  const originX = anchors.angle != null ? rawX - VERTICAL_LABEL_LINE_GAP : rawX
  const originY = rawY
  const width = text.length * PILL_CHAR_WIDTH + PILL_PAD_X * 2
  const height = size + PILL_PAD_Y * 2
  const left =
    anchors.textAnchor === "start"
      ? originX - PILL_PAD_X
      : anchors.textAnchor === "end"
        ? originX + PILL_PAD_X - width
        : originX - width / 2
  // A "start" anchor puts the text's top edge on the point, an "end" anchor
  // puts its bottom edge there.
  const top =
    anchors.verticalAnchor === "start" ? originY - PILL_PAD_Y : originY + PILL_PAD_Y - height
  const pill = (
    <>
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
    </>
  )
  return anchors.angle != null ? (
    <g transform={`rotate(${anchors.angle}, ${originX}, ${originY})`}>{pill}</g>
  ) : (
    <g>{pill}</g>
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

/**
 * Greedy min-gap labelling: walk `indices` in ascending order and keep one
 * whenever it sits at least `minGapFraction` of the x-domain past the last
 * kept index. `domainLength` is the series length the marks share, so the
 * gap scales with the same x-axis every mark draws on. Callers pass
 * `minGapFraction` because the label shape sets how close two labels can sit:
 * a wide horizontal pill needs a bigger gap than a narrow rotated label.
 */
export function labeledIndices(
  indices: readonly number[],
  domainLength: number,
  minGapFraction: number,
): Set<number> {
  const minGap = Math.max(1, domainLength - 1) * minGapFraction
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
