import { useCallback, useId, type ReactNode } from "react"

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
}: {
  title: string
  onCancel: () => void
  getFallbackFocus?: () => HTMLElement | null
  children: ReactNode
}) {
  const titleId = useId()
  const attach = useCallback(
    (node: HTMLDialogElement | null) => {
      if (!node) return
      const opener = document.activeElement
      node.showModal()
      node.querySelector<HTMLElement>(fieldSelector)?.focus()
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
    [getFallbackFocus],
  )

  return (
    <dialog
      ref={attach}
      aria-labelledby={titleId}
      aria-modal="true"
      tabIndex={-1}
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
      className="m-auto max-h-[calc(100%-2rem)] w-[calc(100%-2rem)] max-w-xl overflow-y-auto rounded-popover border border-separator bg-surface-overlay p-0 text-label shadow-popover backdrop:bg-command-palette-scrim"
    >
      <h2 id={titleId} className="wrap-anywhere px-4 pt-4 pb-2 type-title-3">
        {title}
      </h2>
      {children}
    </dialog>
  )
}
