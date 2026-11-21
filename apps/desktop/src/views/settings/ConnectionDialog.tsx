import { useCallback, useId, useRef, useState, type ReactNode } from "react"
import { TooltipPortalContext } from "../../components/presentation/Tooltip"

const fieldSelector =
  "input:not(:disabled):not([type=hidden]), select:not(:disabled), button:not(:disabled)"

export function isConnectionFocusTarget(element: Element | null): element is HTMLElement {
  return (
    element instanceof HTMLElement &&
    element.isConnected &&
    (element.tabIndex >= 0 || element.hasAttribute("tabindex")) &&
    !element.matches(":disabled, [aria-disabled=true]") &&
    !element.closest("[inert], [hidden], [aria-hidden=true], dialog:not([open])")
  )
}

export function ConnectionDialog({
  title,
  onCancel,
  getFallbackFocus,
  children,
  headerAction,
  dismissOnBackdrop = false,
  initialFocus = "control",
  scrim = "subtle",
}: {
  initialFocus?: "control" | "dialog"
  scrim?: "subtle" | "modal"
  dismissOnBackdrop?: boolean
  title: string
  onCancel: () => void
  getFallbackFocus?: () => HTMLElement | null
  headerAction?: ReactNode
  children: ReactNode
}) {
  const backdropPress = useRef(false)
  const backdropRelease = useRef(false)
  const outside = (event: {
    currentTarget: HTMLDialogElement
    target: EventTarget
    clientX: number
    clientY: number
  }) => {
    const bounds = event.currentTarget.getBoundingClientRect()
    return (
      event.target === event.currentTarget &&
      (event.clientX < bounds.left ||
        event.clientX > bounds.right ||
        event.clientY < bounds.top ||
        event.clientY > bounds.bottom)
    )
  }
  const titleId = useId()
  const [tooltipContainer, setTooltipContainer] = useState<HTMLDialogElement | null>(null)
  const attach = useCallback(
    (node: HTMLDialogElement | null) => {
      if (!node) return
      setTooltipContainer(node)
      const opener = document.activeElement
      node.showModal()
      if (initialFocus === "dialog") node.focus()
      else node.querySelector<HTMLElement>(fieldSelector)?.focus()
      return () => {
        node.close()
        if (isConnectionFocusTarget(opener)) opener.focus()
        else
          requestAnimationFrame(() => {
            if (isConnectionFocusTarget(document.activeElement)) return
            const target = isConnectionFocusTarget(opener)
              ? opener
              : (getFallbackFocus?.() ?? null)
            if (isConnectionFocusTarget(target)) target.focus()
          })
      }
    },
    [getFallbackFocus, initialFocus],
  )

  return (
    <dialog
      ref={attach}
      autoFocus={initialFocus === "dialog"}
      aria-labelledby={titleId}
      aria-modal="true"
      tabIndex={-1}
      onKeyDownCapture={(event) => {
        if (event.key !== "Escape" || event.nativeEvent.isComposing) return
        event.preventDefault()
        event.stopPropagation()
        onCancel()
      }}
      onPointerDown={(event) => {
        backdropPress.current = dismissOnBackdrop && event.button === 0 && outside(event)
        backdropRelease.current = false
      }}
      onPointerUp={(event) => {
        backdropRelease.current = backdropPress.current && outside(event)
      }}
      onPointerCancel={() => {
        backdropPress.current = false
        backdropRelease.current = false
      }}
      onClick={(event) => {
        const dismiss = dismissOnBackdrop && backdropRelease.current && outside(event)
        backdropPress.current = false
        backdropRelease.current = false
        if (!dismiss) return
        event.preventDefault()
        event.stopPropagation()
        onCancel()
      }}
      onKeyDown={(event) => {
        if (event.key !== "Tab" || event.altKey || event.ctrlKey || event.metaKey) return
        const fields = [
          ...event.currentTarget.querySelectorAll<HTMLElement>(fieldSelector),
        ].filter((field) => field.tabIndex >= 0 && !field.hidden)
        const first = fields[0]
        const last = fields.at(-1)
        const atEdge = document.activeElement === (event.shiftKey ? first : last)
        if (atEdge || !fields.some((field) => field === document.activeElement)) {
          event.preventDefault()
          ;(event.shiftKey ? last : first)?.focus()
          if (!first) event.currentTarget.focus()
        }
      }}
      onCancel={(event) => {
        event.preventDefault()
        onCancel()
      }}
      className={`m-auto max-h-[calc(100%-2rem)] w-[calc(100%-2rem)] max-w-xl overflow-y-auto rounded-popover border border-separator bg-surface-overlay p-0 text-label shadow-popover ${scrim === "modal" ? "backdrop:bg-modal-scrim" : "backdrop:bg-command-palette-scrim"}`}
    >
      <TooltipPortalContext value={tooltipContainer}>
        <div className="flex items-center justify-between gap-2 px-4 pt-4 pb-2">
          <h2 id={titleId} className="wrap-anywhere type-title-3">
            {title}
          </h2>
          {headerAction}
        </div>
        {children}
      </TooltipPortalContext>
    </dialog>
  )
}
