import { useRef } from "react"

import { cn } from "../../lib/cn"
import { SegmentedMeter } from "../ui/SegmentedMeter"

/**
 * One cell of the meter nav. The cell names a tab, states that tab's headline
 * figure, and shows one health reading for the content behind it.
 */
export interface MeterTab<T extends string> {
  value: T
  label: string
  /** The tab's headline figure, formatted for display. */
  figure: string
  /**
   * What the figure is, when the cell name does not say it in full. The
   * pointer tooltip states it. Defaults to the cell name.
   */
  figureLabel?: string
  /** The health reading, 0 to 100, or null when no figure measures it. */
  percent: number | null
  /** What the meter measures. The cell states this to assistive technology. */
  meterLabel: string
}

/**
 * Segments across one cell's track. Three cells share the popover width, so a
 * cell track is about a third of a full-width meter.
 */
const CELL_SEGMENTS = 10

/**
 * The session-detail navigation: one cell per tab, each carrying its own
 * figure and meter.
 *
 * The popover's usage bar is the model. There, a control is also a readout:
 * you see the standing before you decide to open it. Plain text tabs told the
 * reader nothing until they clicked, and the figures they needed sat in a
 * separate strip above. This control is both, so the header holds one row of
 * figures instead of two.
 *
 * Every meter reads the same direction: a higher reading is worse. A cell with
 * nothing to measure passes a null percent, and the meter shows no reading
 * rather than a reading of zero.
 *
 * Selection is a raised neutral card, not a brand fill. The meters own the
 * brand color on this row, and a selected cell in the same orange competed
 * with the reading inside it.
 */
export function SessionMeterTabs<T extends string>({
  tabs,
  value,
  onChange,
  ariaLabel,
  idPrefix,
}: {
  tabs: ReadonlyArray<MeterTab<T>>
  value: T
  onChange: (next: T) => void
  ariaLabel: string
  /** Prefix for the tab and panel ids that tie this control to its panel. */
  idPrefix: string
}) {
  const buttonRefs = useRef<Array<HTMLButtonElement | null>>([])

  function selectIndex(index: number) {
    if (tabs.length === 0) return
    const normalized = (index + tabs.length) % tabs.length
    const tab = tabs[normalized]
    if (!tab) return
    onChange(tab.value)
    buttonRefs.current[normalized]?.focus()
  }

  return (
    <div
      role="tablist"
      aria-label={ariaLabel}
      className="grid gap-1"
      style={{ gridTemplateColumns: `repeat(${tabs.length}, minmax(0, 1fr))` }}
    >
      {tabs.map((tab, index) => {
        const selected = value === tab.value
        const meterReading =
          tab.percent == null
            ? `${tab.meterLabel}: no stated figure`
            : `${tab.meterLabel}: ${Math.round(tab.percent)} percent`
        return (
          <button
            ref={(node) => {
              buttonRefs.current[index] = node
            }}
            key={tab.value}
            id={`${idPrefix}-${tab.value}`}
            type="button"
            role="tab"
            aria-selected={selected}
            aria-controls={`${idPrefix}-panel`}
            tabIndex={selected ? 0 : -1}
            title={`${tab.figureLabel ?? tab.label} — ${tab.figure} · ${meterReading}`}
            onClick={() => onChange(tab.value)}
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
                selectIndex(tabs.length - 1)
              }
            }}
            className={cn(
              "flex min-w-0 flex-col gap-y-1 rounded-control px-2 py-1.5 text-left transition-colors duration-[var(--duration-quick)] ease-out-quart",
              selected
                ? "bg-surface-secondary shadow-raised"
                : "hover:bg-surface-hover active:bg-surface-hover",
            )}
          >
            <span
              className={cn(
                "truncate type-caption font-medium! uppercase",
                selected ? "text-label-secondary" : "text-label-tertiary",
              )}
            >
              {tab.label}
            </span>
            <span
              className={cn(
                "truncate type-headline tabular-nums",
                selected ? "text-label" : "text-label-secondary",
              )}
            >
              {tab.figure}
            </span>
            <SegmentedMeter percent={tab.percent} segments={CELL_SEGMENTS} />
            <span className="sr-only">{meterReading}</span>
          </button>
        )
      })}
    </div>
  )
}
