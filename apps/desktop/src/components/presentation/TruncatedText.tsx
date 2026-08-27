import {
  useCallback,
  useRef,
  useSyncExternalStore,
  type CSSProperties,
  type RefObject,
} from "react"

import { cn } from "../../lib/cn"

/**
 * Track whether the element overflows its box.
 * The hook reads the value again after `subscribe` attaches the ref.
 * `ResizeObserver` reports later box size changes.
 * A text or line change starts a new subscription and checks the content again.
 */
function useTruncated(
  ref: RefObject<HTMLElement | null>,
  text: string,
  lines: number,
): boolean {
  const subscribe = useCallback(
    (onChange: () => void) => {
      const element = ref.current
      if (!element || typeof ResizeObserver === "undefined") return () => undefined
      const observer = new ResizeObserver(onChange)
      observer.observe(element)
      return () => observer.disconnect()
    },
    // `text` and `lines` start a new subscription after content changes.
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
  /** The maximum number of lines to show before the component cuts off the text. */
  lines?: number
  /**
   * Sweep a highlight across the text to mark its subject as running.
   * The text is duplicated into `data-text` for the CSS overlay to paint.
   */
  shimmer?: boolean
}

/**
 * Show the full text in a native `title` tooltip when the visible text is cut off.
 * The optional line limit shows more text before the component cuts it off.
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
