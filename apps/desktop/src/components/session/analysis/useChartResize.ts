import { useRef, useState } from "react"

import { prefersReducedMotion } from "../../../lib/popoverHeight"

export interface ChartResizeState {
  /** Pass to `ResponsiveContainer`'s `onResize`. */
  onResize: (width: number, height: number) => void
  /** True while the current data set is redrawing from a geometry change, not new data. */
  resizing: boolean
  /** True when the chart should play its entrance animation. */
  animate: boolean
  /**
   * True while `data` is still the value the chart first mounted with. A
   * chart that stages its entrance (each layer starting a little after the
   * one before it) only does that for this first data set: a later data set
   * arrives from a live poll, where a staggered replay would read as the
   * panel redrawing itself.
   */
  initial: boolean
}

/**
 * Tracks whether a chart's data changed because the panel resized, so a
 * geometry change never replays the entrance animation. `data` is the chart's
 * own data set (by reference): the first render remembers it, and a later
 * `onResize` call that finds a real size change marks that same reference as
 * "resized", which suppresses the animation until a new data set arrives.
 *
 * No effect: the resize itself is reported through recharts' own `onResize`
 * callback, and the state update happens there, in the event that caused it.
 */
export function useChartResize<T>(data: T): ChartResizeState {
  const [initialData] = useState(() => data)
  const measuredSize = useRef<{ width: number; height: number } | null>(null)
  const [resizedData, setResizedData] = useState<T | null>(null)
  const resizing = resizedData === data
  const animate = !resizing && !prefersReducedMotion()
  const onResize = (width: number, height: number) => {
    if (width <= 0 || height <= 0) return
    const next = { width: Math.round(width), height: Math.round(height) }
    const previous = measuredSize.current
    measuredSize.current = next
    if (previous && (previous.width !== next.width || previous.height !== next.height)) {
      // Geometry changes must not replay the data's entrance animation.
      setResizedData(data)
    }
  }
  return { onResize, resizing, animate, initial: data === initialData }
}
