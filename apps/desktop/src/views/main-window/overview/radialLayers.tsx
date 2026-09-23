import { AXIS_TICK } from "../../../components/session/analysis/chartLabels"
import type { RadialFocus, Spoke } from "./radialFocus"
import {
  DAYS_PER_WEEK,
  LIMIT_PERCENT,
  arcPath,
  dayPath,
  polar,
  radius,
  type Geometry,
  type LimitStretch,
  type Point,
} from "./radialGeometry"

// The usage data fades to this share while another part is in focus.
const DATA_DIM = 0.35
// A limit hit in a past week draws at this share.
const PAST_LIMIT_OPACITY = 0.55

export type Petal = {
  start: number
  current: boolean
  path: { area: string; edge: string }
}
export type SpokeLine = Spoke & { from: Point; to: Point }

function hitsLimit(spoke: Spoke): boolean {
  return spoke.peakPercent >= LIMIT_PERCENT
}

/** The rings, the day spokes and the axis. They stay still while the data
 *  sweeps in. */
export function GridLayer({ g, side, height }: { g: Geometry; side: number; height: number }) {
  return (
    <svg
      width={side}
      height={height}
      className="absolute inset-0 overflow-visible"
      aria-hidden="true"
    >
      {[50, 100].map((percent) => (
        <circle
          key={percent}
          cx={g.cx}
          cy={g.cy}
          r={radius(g, percent)}
          fill="none"
          stroke="var(--color-separator)"
          strokeWidth={1}
        />
      ))}
      {Array.from({ length: DAYS_PER_WEEK }, (_, day) => {
        const from = polar(g, day / DAYS_PER_WEEK, g.inner)
        const to = polar(g, day / DAYS_PER_WEEK, g.outer)
        return (
          <line
            key={day}
            x1={from.x}
            y1={from.y}
            x2={to.x}
            y2={to.y}
            stroke="var(--color-separator)"
            strokeWidth={1}
          />
        )
      })}
      <text x={g.cx + 5} y={g.cy - radius(g, 100) + 12} textAnchor="start" {...AXIS_TICK}>
        100% · reset
      </text>
      <text x={g.cx + 5} y={g.cy - radius(g, 50) + 12} textAnchor="start" {...AXIS_TICK}>
        50%
      </text>
    </svg>
  )
}

/** A limit hit: a red arc on the rim for the time the week sat at 100%. */
function LimitArc({
  g,
  limit,
  strong = false,
}: {
  g: Geometry
  limit: LimitStretch
  strong?: boolean
}) {
  return (
    <path
      d={arcPath(g, limit.from, limit.to, g.outer)}
      fill="none"
      className="stroke-system-red"
      strokeWidth={strong ? 6 : 4}
      strokeLinecap="round"
      style={{ opacity: strong || limit.current ? 1 : PAST_LIMIT_OPACITY }}
    />
  )
}

/** The usage data: 5-hour spokes, past petals, the average ring, this week
 *  and the limit hits. It fades while the focus is on another part. */
export function DataLayer({
  g,
  side,
  height,
  petals,
  spokes,
  rolling,
  limits,
  tip,
  dimmed,
}: {
  g: Geometry
  side: number
  height: number
  petals: readonly Petal[]
  spokes: readonly SpokeLine[]
  rolling: number | null
  limits: readonly LimitStretch[]
  tip: Point | null
  dimmed: boolean
}) {
  const currentPetal = petals.find((petal) => petal.current)
  return (
    <svg
      width={side}
      height={height}
      className="overview-radial-data overview-radial-focus absolute inset-0 overflow-visible"
      style={{ opacity: dimmed ? DATA_DIM : 1 }}
      aria-hidden="true"
    >
      {spokes.map((spoke) => (
        <line
          key={spoke.key}
          x1={spoke.from.x}
          y1={spoke.from.y}
          x2={spoke.to.x}
          y2={spoke.to.y}
          className={hitsLimit(spoke) ? "stroke-system-red/60" : "stroke-context-stroke/20"}
          strokeWidth={2.5}
          strokeLinecap="round"
        />
      ))}
      {petals
        .filter((petal) => !petal.current)
        .map((petal) => (
          <g key={petal.start}>
            <path d={petal.path.area} className="fill-context-stroke/[0.07]" />
            <path
              d={petal.path.edge}
              fill="none"
              className="stroke-context-stroke/35"
              strokeWidth={1}
            />
          </g>
        ))}
      {rolling != null && (
        <circle
          cx={g.cx}
          cy={g.cy}
          r={radius(g, rolling)}
          fill="none"
          className="stroke-gray-500"
          strokeWidth={1}
        />
      )}
      {currentPetal && (
        <g>
          <path d={currentPetal.path.area} className="fill-context-stroke/25" />
          <path
            d={currentPetal.path.edge}
            fill="none"
            className="stroke-context-stroke"
            strokeWidth={2}
            strokeLinejoin="round"
          />
        </g>
      )}
      {limits.map((limit) => (
        <LimitArc key={limit.weekStart} g={g} limit={limit} />
      ))}
      {tip && <circle cx={tip.x} cy={tip.y} r={3.5} className="fill-context-stroke" />}
    </svg>
  )
}

function PetalMark({ petal, strong }: { petal: Petal; strong: boolean }) {
  return (
    <g>
      <path
        d={petal.path.area}
        className={strong ? "fill-context-stroke/25" : "fill-context-stroke/10"}
      />
      <path
        d={petal.path.edge}
        fill="none"
        className={strong ? "stroke-context-stroke" : "stroke-context-stroke/70"}
        strokeWidth={strong ? 2.5 : 1.5}
        strokeLinejoin="round"
      />
    </g>
  )
}

function SpokeMark({ spoke, strong }: { spoke: SpokeLine; strong: boolean }) {
  const limit = hitsLimit(spoke)
  return (
    <line
      x1={spoke.from.x}
      y1={spoke.from.y}
      x2={spoke.to.x}
      y2={spoke.to.y}
      className={
        limit
          ? "stroke-system-red"
          : strong
            ? "stroke-context-stroke"
            : "stroke-context-stroke/60"
      }
      strokeWidth={strong ? 4 : 3}
      strokeLinecap="round"
    />
  )
}

function LayerMark({
  g,
  layer,
  petals,
  spokes,
  limits,
}: {
  g: Geometry
  layer: string
  petals: readonly Petal[]
  spokes: readonly SpokeLine[]
  limits: readonly LimitStretch[]
}) {
  if (layer === "short")
    return spokes.map((spoke) => <SpokeMark key={spoke.key} spoke={spoke} strong={false} />)
  if (layer === "limit")
    return (
      <>
        {limits.map((limit) => (
          <LimitArc key={limit.weekStart} g={g} limit={limit} strong />
        ))}
        {spokes.filter(hitsLimit).map((spoke) => (
          <SpokeMark key={spoke.key} spoke={spoke} strong />
        ))}
      </>
    )
  if (layer === "week" || layer === "past")
    return petals
      .filter((petal) => petal.current === (layer === "week"))
      .map((petal) => <PetalMark key={petal.start} petal={petal} strong={layer === "week"} />)
  return null
}

/** The part in focus, drawn again at full strength above the faded data. */
export function FocusMark({
  g,
  focus,
  petals,
  spokes,
  rolling,
  limits,
}: {
  g: Geometry
  focus: RadialFocus | null
  petals: readonly Petal[]
  spokes: readonly SpokeLine[]
  rolling: number | null
  limits: readonly LimitStretch[]
}) {
  const ring = rolling != null && (
    <circle
      cx={g.cx}
      cy={g.cy}
      r={radius(g, rolling)}
      fill="none"
      className="stroke-label"
      strokeWidth={2}
    />
  )
  switch (focus?.kind) {
    case "week": {
      const petal = petals.find((item) => item.start === focus.start)
      return petal ? <PetalMark petal={petal} strong /> : null
    }
    case "limit": {
      const petal = petals.find((item) => item.start === focus.weekStart)
      const limit = limits.find((item) => item.weekStart === focus.weekStart)
      return (
        <>
          {petal && <PetalMark petal={petal} strong={false} />}
          {limit && <LimitArc g={g} limit={limit} strong />}
        </>
      )
    }
    case "short": {
      const spoke = spokes.find((item) => item.key === focus.key)
      return spoke ? <SpokeMark spoke={spoke} strong /> : null
    }
    case "rolling":
      return ring
    case "day":
      return <path d={dayPath(g, focus.day)} className="fill-context-stroke/[0.08]" />
    case "layer":
      if (focus.layer === "rolling") return ring
      return (
        <LayerMark g={g} layer={focus.layer} petals={petals} spokes={spokes} limits={limits} />
      )
    default:
      return null
  }
}
