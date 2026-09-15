import { Flame } from "lucide-react"
import { useId } from "react"

import { formatTokenBurnPercent } from "../../lib/presentation/checks"
import { Tooltip } from "../presentation/Tooltip"

const FLAMES = [0, 1, 2, 3] as const

export function BurnCheckFlames({ basisPoints }: { basisPoints: number }) {
  const id = useId()
  const value = Math.max(0, Math.min(10_000, basisPoints))
  const label = `${formatTokenBurnPercent(value)} token burn`
  const lowBurn = value < 500

  return (
    <Tooltip
      label={
        <span className="block space-y-1">
          <span
            className={`block font-mono type-callout font-semibold! tabular-nums ${value === 0 ? "text-burn-check-pass-fill" : "text-burn-check-failure-text"}`}
          >
            {label}
          </span>
          <span className="block type-callout text-label-secondary">
            Estimated share of tokens spent on avoidable work.
          </span>
          <span className="block type-footnote text-label-tertiary">
            Each full flame represents 25% token burn.
          </span>
        </span>
      }
    >
      <span
        tabIndex={0}
        role="meter"
        aria-label="Estimated token burn"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={value / 100}
        aria-valuetext={label}
        className="inline-flex min-h-10 items-center rounded-control"
      >
        <svg
          aria-hidden="true"
          width={78}
          height={18}
          viewBox="0 0 104 24"
          className="block shrink-0"
        >
          <defs>
            <linearGradient id={`${id}-gradient`} x1="0" y1="1" x2="0" y2="0">
              <stop
                offset="0%"
                stopColor={
                  lowBurn
                    ? "var(--color-system-yellow-tint-val)"
                    : "var(--color-brand-tint-val)"
                }
              />
              <stop
                offset="100%"
                stopColor={
                  lowBurn ? "var(--color-brand-tint-val)" : "var(--color-system-red-tint-val)"
                }
              />
            </linearGradient>
            <mask
              id={`${id}-mask`}
              maskUnits="userSpaceOnUse"
              x={0}
              y={0}
              width={104}
              height={24}
            >
              {FLAMES.map((index) => (
                <Flame key={index} x={index * 27} size={23} fill="white" stroke="white" />
              ))}
            </mask>
          </defs>
          <g mask={`url(#${id}-mask)`}>
            <rect
              width={104}
              height={24}
              fill="var(--color-burn-check-neutral-val)"
              opacity={0.3}
            />
            {FLAMES.map((index) => {
              const fraction = Math.max(0, Math.min(1, value / 2_500 - index))
              return (
                <rect
                  key={index}
                  x={index * 27}
                  y={23 * (1 - fraction)}
                  width={23}
                  height={23 * fraction}
                  fill={`url(#${id}-gradient)`}
                />
              )
            })}
          </g>
        </svg>
      </span>
    </Tooltip>
  )
}
