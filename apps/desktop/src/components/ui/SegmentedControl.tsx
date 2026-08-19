// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useRef } from "react"

export type SegmentedOption<T extends string> = {
  value: T
  label: string
}

export type SegmentedControlVariant = "segmented" | "text-tabs"

/** A single-select pill group exposed as a radiogroup (or a tablist). Works
 *  for any number of options.
 *
 *  `variant="text-tabs"` drops the pill chrome for a run of plain labels with
 *  an underline on the selected one — the same control and the same keyboard
 *  contract, for places where a filled segment would be too loud. */
export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  ariaLabel,
  className = "",
  equalWidth = false,
  animatedIndicator = false,
  semantics = "radio",
  idPrefix,
  variant = "segmented",
}: {
  options: ReadonlyArray<SegmentedOption<T>>
  value: T
  onChange: (next: T) => void
  ariaLabel: string
  className?: string
  equalWidth?: boolean
  animatedIndicator?: boolean
  semantics?: "radio" | "tabs"
  idPrefix?: string
  variant?: SegmentedControlVariant
}) {
  const textTabs = variant === "text-tabs"
  const showAnimatedIndicator = animatedIndicator && !textTabs
  const selectedIndex = Math.max(
    0,
    options.findIndex((option) => option.value === value),
  )
  const buttonRefs = useRef<Array<HTMLButtonElement | null>>([])

  function selectIndex(index: number) {
    if (options.length === 0) return
    const normalized = (index + options.length) % options.length
    const option = options[normalized]
    if (!option) return
    onChange(option.value)
    buttonRefs.current[normalized]?.focus()
  }

  return (
    <div
      role={semantics === "tabs" ? "tablist" : "radiogroup"}
      aria-label={ariaLabel}
      data-variant={variant}
      className={
        textTabs
          ? `inline-flex h-6 items-center gap-3 whitespace-nowrap ${className}`.trimEnd()
          : `${showAnimatedIndicator ? "relative " : ""}${equalWidth ? "grid " : "inline-flex "}h-6 overflow-hidden rounded-control border border-separator bg-surface-secondary ${className}`.trimEnd()
      }
      style={
        equalWidth && !textTabs
          ? {
              gridTemplateColumns: `repeat(${options.length}, minmax(0, 1fr))`,
            }
          : undefined
      }
    >
      {showAnimatedIndicator ? (
        <span
          aria-hidden="true"
          className="pointer-events-none absolute inset-y-0 left-0 rounded-[4px] bg-accent-fill transition-transform duration-[120ms] ease-out motion-reduce:hidden"
          style={{
            width: `${100 / options.length}%`,
            transform: `translateX(${selectedIndex * 100}%)`,
          }}
        />
      ) : null}
      {options.map((option, index) => {
        const selected = value === option.value
        const optionId = idPrefix ? `${idPrefix}-${option.value}` : undefined
        return (
          <button
            ref={(node) => {
              buttonRefs.current[index] = node
            }}
            key={option.value}
            id={optionId}
            type="button"
            role={semantics === "tabs" ? "tab" : "radio"}
            aria-checked={semantics === "radio" ? selected : undefined}
            aria-selected={semantics === "tabs" ? selected : undefined}
            aria-controls={semantics === "tabs" && idPrefix ? `${idPrefix}-panel` : undefined}
            tabIndex={selected ? 0 : -1}
            onClick={() => onChange(option.value)}
            onKeyDown={(event) => {
              if (event.key === "ArrowRight" || event.key === "ArrowDown") {
                event.preventDefault()
                selectIndex(index + 1)
              } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
                event.preventDefault()
                selectIndex(index - 1)
              } else if (event.key === "Home") {
                event.preventDefault()
                selectIndex(0)
              } else if (event.key === "End") {
                event.preventDefault()
                selectIndex(options.length - 1)
              }
            }}
            className={
              textTabs
                ? `type-footnote relative flex h-full items-center whitespace-nowrap px-0 transition-colors duration-100 ease-[cubic-bezier(0.23,1,0.32,1)] ${selected ? "font-medium text-label" : "text-label-tertiary hover:text-label-secondary"}`
                : `${showAnimatedIndicator ? "relative " : ""}${equalWidth ? "min-w-0 " : ""}type-footnote px-2 ${selected ? `${showAnimatedIndicator ? "" : "bg-accent-fill "}text-white` : "text-label-secondary hover:bg-surface-hover"}`
            }
          >
            {textTabs ? (
              // `segmented-control-text-indicator` carries no rule in the
              // design foundation on purpose: everything about this underline
              // is expressed in the utilities on the same element. The class is
              // a stable hook for tests and for a consumer that needs to find
              // the indicator, not a styling API.
              <span
                aria-hidden="true"
                className={`segmented-control-text-indicator pointer-events-none absolute inset-x-0 bottom-0 h-px rounded-full bg-label-secondary transition-opacity duration-100 ease-[cubic-bezier(0.23,1,0.32,1)] ${selected ? "opacity-100" : "opacity-0"}`}
              />
            ) : null}
            {showAnimatedIndicator ? (
              // Reduced-motion fallback for the sliding indicator above: the
              // fill moves by appearing on the selected segment instead of
              // travelling. `.ui-segmented-reduced-fill` is what styles/motion.css
              // grants a short crossfade to, so the swap is not a hard flicker.
              <span
                aria-hidden="true"
                className={`ui-segmented-reduced-fill pointer-events-none absolute inset-0 hidden rounded-[4px] bg-accent-fill transition-opacity ease-out motion-reduce:block ${selected ? "opacity-100" : "opacity-0"}`}
              />
            ) : null}
            <span className={showAnimatedIndicator ? "relative z-10" : undefined}>
              {option.label}
            </span>
          </button>
        )
      })}
    </div>
  )
}
