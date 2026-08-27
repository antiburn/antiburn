import { useCallback, useRef, useSyncExternalStore } from "react"

/** A callback ref that may return a cleanup, as React 19 allows. */
export type ViewportRef = (node: HTMLDivElement | null) => void | (() => void)

export interface ActivityGroupPinning {
  /** Callback ref for the scrolling viewport the groups live in. */
  assignViewportRef: ViewportRef
  /** Callback-ref factory for each in-flow group heading, keyed by its label. */
  registerHeading: (label: string) => (node: HTMLHeadingElement | null) => void
  /** The day label to show in the sticky header, or null when there are none. */
  pinnedLabel: string | null
}

function getServerSnapshot(): string | null {
  return null
}

/** The unsubscribe for a subscription that attached nothing. */
function noopUnsubscribe(): void {
  return undefined
}

/**
 * Track which day heading a scrolling activity list is currently under, so the
 * list can paint one sticky label above the viewport.
 *
 * The scroll machinery belongs to the list, not to any one row kind: it needs
 * only the ordered day labels and the headings rendered for them.
 */
export function useActivityGroupPinning(
  groupLabels: string[],
  externalViewportRef?: ViewportRef,
): ActivityGroupPinning {
  const viewportRef = useRef<HTMLDivElement | null>(null)
  const headingRefs = useRef(new Map<string, HTMLHeadingElement>())
  // Caches the last computed label so `getSnapshot` can return the same
  // reference when it hasn't changed — `useSyncExternalStore` treats a fresh
  // value every call as a store change on every render otherwise.
  const lastLabel = useRef<string | null>(null)

  // NUL-joined because day labels contain spaces, so the separator has to be a
  // character a label can never hold.
  const groupLabelsSignature = groupLabels.join("\0")

  // Stable, so React attaches it once per viewport rather than on every
  // render. When the host's ref returns a cleanup, React calls that on detach
  // in place of a null call, so the cleanup must also clear the local ref.
  const assignViewportRef = useCallback(
    (node: HTMLDivElement | null) => {
      viewportRef.current = node
      const cleanup = externalViewportRef?.(node)
      if (!cleanup) return
      return () => {
        viewportRef.current = null
        cleanup()
      }
    },
    [externalViewportRef],
  )

  const registerHeading = (label: string) => (node: HTMLHeadingElement | null) => {
    if (node) headingRefs.current.set(label, node)
    else headingRefs.current.delete(label)
  }

  // Values only ever drawn from the current `labels` array, so the result can
  // never point at a label the list has since dropped — no stale-value
  // fallback needed the way a piece of remembered state would need one.
  const getSnapshot = useCallback((): string | null => {
    const labels = groupLabelsSignature ? groupLabelsSignature.split("\0") : []
    const viewport = viewportRef.current
    const first = labels[0] ?? null

    // At (or above) the top, the first group is pinned by definition — no
    // need to walk headings whose rects may not even be settled yet.
    let next = first
    if (viewport && labels.length > 0 && viewport.scrollTop > 0) {
      const viewportTop = viewport.getBoundingClientRect().top + 0.5
      for (const label of labels.slice(1)) {
        const heading = headingRefs.current.get(label)
        if (!heading || heading.getBoundingClientRect().top > viewportTop) break
        next = label
      }
    }

    if (lastLabel.current === next) return lastLabel.current
    lastLabel.current = next
    return next
  }, [groupLabelsSignature])

  // Coalesces scroll events onto one frame: a trackpad fling fires far more
  // often than the display refreshes, and the answer only changes per frame.
  // Re-established whenever the label set changes, which — per
  // `useSyncExternalStore`'s guaranteed post-subscribe resync (see
  // `useElementWidth`'s doc comment for the same mechanism) — also covers the
  // initial catch-up an already-scrolled viewport needs on mount, with no
  // manual kick required.
  const subscribe = useCallback(
    (onChange: () => void) => {
      const viewport = viewportRef.current
      // No viewport yet means no listener, so there is nothing to remove.
      if (!viewport) return noopUnsubscribe
      let animationFrame: number | null = null
      const scheduleUpdate = () => {
        if (animationFrame !== null) return
        animationFrame = requestAnimationFrame(() => {
          animationFrame = null
          onChange()
        })
      }
      viewport.addEventListener("scroll", scheduleUpdate, { passive: true })
      return () => {
        viewport.removeEventListener("scroll", scheduleUpdate)
        if (animationFrame !== null) cancelAnimationFrame(animationFrame)
      }
      // Keyed on the label set (not read inside, hence the lint-visible "unused
      // dependency"): resubscribing per label-set change is what makes the
      // guaranteed post-subscribe resync cover a remount into an already-
      // scrolled viewport, matching the effect this replaced.
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [groupLabelsSignature],
  )

  const pinnedLabel = useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)

  return {
    assignViewportRef,
    registerHeading,
    pinnedLabel,
  }
}
