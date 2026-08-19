// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useRef } from "react"

import { cn } from "../../lib/cn"

/** A horizontal value slider.
 *
 *  A native `input[type=range]`, so the platform supplies the drag, the
 *  keyboard behavior, and the accessible slider role; the design foundation
 *  only restyles the track and the accent.
 *
 *  `onChange` fires continuously so a live preview can follow the thumb;
 *  `onCommit` fires once when the gesture settles, which is where a caller
 *  should persist. */
export function RangeSlider({
  value,
  min,
  max,
  step = 1,
  onChange,
  onCommit,
  ariaLabel,
  ariaValueText,
  disabled = false,
  className = "",
}: {
  value: number
  min: number
  max: number
  step?: number
  onChange: (next: number) => void
  /** Fires once when a drag or key gesture settles and changed the value. */
  onCommit?: (next: number) => void
  ariaLabel: string
  ariaValueText?: string
  disabled?: boolean
  className?: string
}) {
  const gestureStart = useRef<number | null>(null)

  function beginGesture() {
    if (gestureStart.current === null) gestureStart.current = value
  }

  function endGesture(next: number) {
    const started = gestureStart.current
    gestureStart.current = null
    if (started === null || started === next) return
    onCommit?.(next)
  }

  return (
    <input
      type="range"
      min={min}
      max={max}
      step={step}
      value={value}
      disabled={disabled}
      onChange={(event) => onChange(Number(event.currentTarget.value))}
      onPointerDown={beginGesture}
      onKeyDown={beginGesture}
      onPointerUp={(event) => endGesture(Number(event.currentTarget.value))}
      onPointerCancel={(event) => endGesture(Number(event.currentTarget.value))}
      onKeyUp={(event) => endGesture(Number(event.currentTarget.value))}
      onBlur={(event) => endGesture(Number(event.currentTarget.value))}
      aria-label={ariaLabel}
      aria-valuetext={ariaValueText}
      className={cn(
        "h-1 w-full cursor-default appearance-none rounded-full bg-surface-secondary accent-[var(--color-accent-fill)] disabled:cursor-not-allowed",
        className,
      )}
    />
  )
}
