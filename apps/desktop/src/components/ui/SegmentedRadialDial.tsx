export interface SegmentedRadialDialSegment {
  id: string
  value: number
  className: string
}

export interface SegmentedRadialDialProps {
  segments: readonly SegmentedRadialDialSegment[]
  size: number
  strokeWidth?: number
  gapAngle?: number
  startAngle?: number
  label?: string
}

const FULL_CIRCLE = 360
const MIN_DRAWABLE_ANGLE = 0.001

function finiteOr(value: number | undefined, fallback: number): number {
  return value !== undefined && Number.isFinite(value) ? value : fallback
}

function stableNumber(value: number): number {
  return Number(value.toFixed(6))
}

function stablePositiveNumber(value: number): number {
  const rounded = stableNumber(value)
  if (rounded > 0) return rounded
  return value > 0 ? value : Number.MIN_VALUE
}

function roundCapClearanceAngle(strokeWidth: number, radius: number): number {
  const chordRatio = Math.min(1, strokeWidth / (2 * radius))
  return (2 * Math.asin(chordRatio) * FULL_CIRCLE) / (2 * Math.PI)
}

export function SegmentedRadialDial({
  segments,
  size,
  strokeWidth = 2.5,
  gapAngle = 5,
  startAngle = -90,
  label,
}: SegmentedRadialDialProps) {
  const renderedSize = Math.max(1, finiteOr(size, 16))
  const renderedStroke = Math.min(
    renderedSize - MIN_DRAWABLE_ANGLE,
    Math.max(MIN_DRAWABLE_ANGLE, finiteOr(strokeWidth, 2.5)),
  )
  const radius = (renderedSize - renderedStroke) / 2
  const center = renderedSize / 2
  const validSegments = segments.filter(
    (segment) => Number.isFinite(segment.value) && segment.value > 0,
  )
  const maximum = validSegments.reduce(
    (currentMaximum, segment) => Math.max(currentMaximum, segment.value),
    0,
  )
  const normalizedTotal = validSegments.reduce(
    (total, segment) => total + segment.value / maximum,
    0,
  )
  const capClearanceAngle = roundCapClearanceAngle(renderedStroke, radius)
  const maximumGap =
    validSegments.length > 1 ? (FULL_CIRCLE - MIN_DRAWABLE_ANGLE) / validSegments.length : 0
  const requestedGap = Math.max(0, finiteOr(gapAngle, 0))
  const renderedGap = Math.min(
    maximumGap,
    validSegments.length > 1 ? requestedGap + capClearanceAngle : 0,
  )
  const drawableAngle = FULL_CIRCLE - renderedGap * validSegments.length
  const arcAngles = validSegments.map((segment) =>
    stablePositiveNumber(drawableAngle * (segment.value / maximum / normalizedTotal)),
  )
  const segmentStarts = arcAngles.map((_, index) =>
    stableNumber(
      arcAngles.slice(0, index).reduce((total, arcAngle) => total + arcAngle, 0) +
        renderedGap * index,
    ),
  )

  return (
    <svg
      data-segmented-radial-dial=""
      viewBox={`0 0 ${renderedSize} ${renderedSize}`}
      width={renderedSize}
      height={renderedSize}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      focusable="false"
    >
      <g transform={`rotate(${finiteOr(startAngle, -90)} ${center} ${center})`}>
        {validSegments.map((segment, index) => {
          if (validSegments.length === 1) {
            return (
              <circle
                key={segment.id}
                data-segment-id={segment.id}
                data-arc-angle={FULL_CIRCLE}
                cx={center}
                cy={center}
                r={radius}
                fill="none"
                stroke="currentColor"
                strokeWidth={renderedStroke}
                className={segment.className}
              />
            )
          }

          const arcAngle = arcAngles[index] ?? 0
          const segmentStart = segmentStarts[index] ?? 0
          return (
            <circle
              key={segment.id}
              data-segment-id={segment.id}
              data-arc-angle={arcAngle}
              data-start-angle={segmentStart}
              cx={center}
              cy={center}
              r={radius}
              pathLength={FULL_CIRCLE}
              fill="none"
              stroke="currentColor"
              strokeWidth={renderedStroke}
              strokeLinecap="round"
              strokeDasharray={`${arcAngle} ${stableNumber(FULL_CIRCLE - arcAngle)}`}
              strokeDashoffset={-segmentStart}
              className={segment.className}
            />
          )
        })}
      </g>
    </svg>
  )
}
