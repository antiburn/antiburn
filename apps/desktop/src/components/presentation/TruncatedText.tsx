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
function useOverflow(ref: RefObject<HTMLElement | null>, text: string, lines: number): number {
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

  const getSnapshot = useCallback(() => {
    if (!ref.current) return 0
    const horizontal = Math.max(0, ref.current.scrollWidth - ref.current.clientWidth)
    const vertical =
      lines > 1 ? Math.max(0, ref.current.scrollHeight - ref.current.clientHeight) : 0
    return Math.max(horizontal, vertical)
  }, [lines, ref])

  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
}

function getServerSnapshot(): number {
  return 0
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
  /** Reveal a truncated single line by moving it horizontally on parent hover. */
  scrollOnHover?: boolean
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
  scrollOnHover = false,
}: TruncatedTextProps) {
  const lineLimit = Number.isFinite(lines) ? Math.max(1, Math.floor(lines)) : 1
  const ref = useRef<HTMLDivElement | null>(null)
  const overflow = useOverflow(ref, text, lineLimit)
  const truncated = overflow > 0
  const lineStyle =
    lineLimit > 1 ? ({ "--truncated-text-lines": lineLimit } as CSSProperties) : undefined

  if (scrollOnHover && lineLimit === 1) {
    const scrollStyle = {
      "--truncated-text-offset": `${-overflow}px`,
      "--truncated-text-duration": `${Math.max(900, overflow * 22)}ms`,
    } as CSSProperties

    return (
      <div
        className={cn(className, "session-title-scroll")}
        style={scrollStyle}
        title={truncated ? text : undefined}
        data-scroll-on-hover=""
        data-truncated={truncated ? "true" : undefined}
        aria-label={shimmer ? text : undefined}
      >
        <div
          ref={ref}
          className={cn(
            "session-title-scroll-rest truncate",
            shimmer && "activity-row-title-shimmer",
          )}
          data-text={shimmer ? text : undefined}
        >
          {text}
        </div>
        <span className="session-title-scroll-copy" data-text={text} aria-hidden="true" />
      </div>
    )
  }

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
