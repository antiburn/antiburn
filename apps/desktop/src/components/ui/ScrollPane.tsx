import * as ScrollArea from "@radix-ui/react-scroll-area"
import { useCallback, type ReactNode, type Ref } from "react"

import { cn } from "../../lib/cn"

function syncTopEdgeFade(viewport: HTMLDivElement) {
  const shouldFade = viewport.scrollTop > 1
  if (shouldFade === viewport.hasAttribute("data-scroll-edge-top")) return

  if (shouldFade) {
    viewport.setAttribute("data-scroll-edge-top", "active")
  } else {
    viewport.removeAttribute("data-scroll-edge-top")
  }
}

function syncBottomEdgeFade(viewport: HTMLDivElement) {
  const shouldFade = viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop > 1
  if (shouldFade) {
    viewport.setAttribute("data-scroll-edge-bottom", "active")
  } else {
    viewport.removeAttribute("data-scroll-edge-bottom")
  }
}

/** Assign a ref. A function ref may return a cleanup, which is passed on. */
function assignRef<T>(ref: Ref<T> | undefined, value: T | null): void | (() => void) {
  if (typeof ref === "function") return ref(value)
  if (ref) ref.current = value
}

/** A scroll container wired to the design foundation's scrollbar styling.
 *
 *  `ui-scroll-viewport` is load-bearing, not cosmetic: on macOS it forces
 *  `overflow-y: scroll` (see styles/platform-controls.css) so scrollability
 *  never depends on Radix's ResizeObserver measurement, which latches to "no
 *  overflow" across a window hide/show cycle. */
export function ScrollPane({
  children,
  className = "",
  viewportClassName = "",
  viewportRef,
  viewportTabIndex,
  viewportLabel,
  topEdgeFade = false,
  bottomEdgeFade = false,
}: {
  children: ReactNode
  className?: string
  viewportClassName?: string
  viewportRef?: Ref<HTMLDivElement>
  viewportTabIndex?: number
  viewportLabel?: string
  /** Fade scrolling content into the viewport's top edge after it leaves the
   *  initial position. The scrollbar remains outside the mask. */
  topEdgeFade?: boolean
  /** Fade the bottom edge while content remains below the viewport. */
  bottomEdgeFade?: boolean
}) {
  // Stable so React attaches it once per viewport. A caller's ref cleanup
  // must reach React, or the caller's listeners outlive the node.
  const assignViewportRef = useCallback(
    (node: HTMLDivElement | null) => {
      if (node && topEdgeFade) syncTopEdgeFade(node)
      const cleanup = assignRef(viewportRef, node)
      if (!node || !bottomEdgeFade) return cleanup

      const sync = () => syncBottomEdgeFade(node)
      sync()
      const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(sync)
      observer?.observe(node)
      if (node.firstElementChild) observer?.observe(node.firstElementChild)
      return () => {
        observer?.disconnect()
        node.removeAttribute("data-scroll-edge-bottom")
        if (cleanup) cleanup()
        else assignRef(viewportRef, null)
      }
    },
    [viewportRef, topEdgeFade, bottomEdgeFade],
  )

  return (
    <ScrollArea.Root className={cn("flex-1 overflow-hidden", className)}>
      <ScrollArea.Viewport
        ref={assignViewportRef}
        tabIndex={viewportTabIndex}
        role={viewportLabel ? "region" : undefined}
        aria-label={viewportLabel}
        onScroll={
          topEdgeFade || bottomEdgeFade
            ? (event) => {
                if (topEdgeFade) syncTopEdgeFade(event.currentTarget)
                if (bottomEdgeFade) syncBottomEdgeFade(event.currentTarget)
              }
            : undefined
        }
        className={cn(
          "ui-scroll-viewport h-full",
          topEdgeFade && "scroll-edge-fade-top",
          bottomEdgeFade && "scroll-edge-fade-bottom",
          viewportClassName,
        )}
      >
        {children}
      </ScrollArea.Viewport>
      <ScrollArea.Scrollbar className="ui-scrollbar" orientation="vertical">
        <ScrollArea.Thumb className="ui-scrollbar-thumb" />
      </ScrollArea.Scrollbar>
    </ScrollArea.Root>
  )
}
