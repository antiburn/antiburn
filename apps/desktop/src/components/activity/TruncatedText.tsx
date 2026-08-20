// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import {
  useCallback,
  useRef,
  useSyncExternalStore,
  type CSSProperties,
  type RefObject,
} from "react"

import { cn } from "../../lib/cn"

/**
 * Track whether `ref`'s element is overflowing its own box, re-rendering as
 * that changes. Modeled on `useElementWidth`: `subscribe` attaches a
 * `ResizeObserver` at subscribe time, which is enough to cover the initial
 * measurement too — `useSyncExternalStore` re-reads `getSnapshot` right after
 * `subscribe` runs on mount, after refs have attached, correcting the first
 * render's stale value before it is ever seen (this holds even in jsdom,
 * which has no `ResizeObserver` and so a `subscribe` that no-ops).
 *
 * `subscribe` is also keyed on `text` and `lines`. The element's box can stay
 * the same size while its content size changes. A `ResizeObserver` only reports
 * box changes. Resubscribing after these values change checks the new content.
 */
function useTruncated(
  ref: RefObject<HTMLElement | null>,
  text: string,
  lines: number,
): boolean {
  const subscribe = useCallback(
    (onChange: () => void) => {
      const element = ref.current
      if (!element || typeof ResizeObserver === "undefined") return () => {}
      const observer = new ResizeObserver(onChange)
      observer.observe(element)
      return () => observer.disconnect()
    },
    // `text` and `lines` force a resubscribe after content or layout changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [ref, text, lines],
  )

  const getSnapshot = useCallback(
    () =>
      ref.current
        ? ref.current.scrollWidth > ref.current.clientWidth ||
          (lines > 1 && ref.current.scrollHeight > ref.current.clientHeight)
        : false,
    [lines, ref],
  )

  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
}

function getServerSnapshot(): boolean {
  return false
}

export interface TruncatedTextProps {
  className?: string
  text: string
  /** Maximum lines to show before the component cuts off the text. */
  lines?: number
  /**
   * Sweep a highlight across the text, marking its subject as still running.
   * The text is duplicated into `data-text` for the CSS overlay to paint.
   */
  shimmer?: boolean
}

/**
 * Text that reveals its full value in a native `title` tooltip when it is cut
 * off. The optional line limit shows more text before it cuts the value.
 */
export function TruncatedText({
  className,
  text,
  lines = 1,
  shimmer = false,
}: TruncatedTextProps) {
  const lineLimit = Number.isFinite(lines) ? Math.max(1, Math.floor(lines)) : 1
  const ref = useRef<HTMLDivElement | null>(null)
  const truncated = useTruncated(ref, text, lineLimit)
  const lineStyle =
    lineLimit > 1 ? ({ "--truncated-text-lines": lineLimit } as CSSProperties) : undefined

  return (
    <div
      ref={ref}
      className={cn(
        className,
        lineLimit === 1 ? "truncate" : "truncated-text-lines",
        shimmer && "activity-row-title-shimmer",
      )}
      style={lineStyle}
      title={truncated ? text : undefined}
      data-text={shimmer ? text : undefined}
      aria-label={shimmer ? text : undefined}
    >
      {text}
    </div>
  )
}
