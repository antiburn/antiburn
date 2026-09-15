import { useCallback, useId, useRef, useState, type ReactNode } from "react"

export function InfoPopover({
  label,
  icon,
  children,
}: {
  label: string
  icon: ReactNode
  children: (close: () => void) => ReactNode
}) {
  const detailsId = useId()
  const [open, setOpen] = useState(false)
  const trigger = useRef<HTMLButtonElement>(null)
  const bindDismiss = useCallback((node: HTMLDivElement | null) => {
    if (!node) return
    const dismissOutside = (event: PointerEvent) => {
      if (event.target instanceof Node && !node.contains(event.target)) setOpen(false)
    }
    const dismissEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return
      event.preventDefault()
      setOpen(false)
      trigger.current?.focus()
    }
    document.addEventListener("pointerdown", dismissOutside)
    document.addEventListener("keydown", dismissEscape)
    return () => {
      document.removeEventListener("pointerdown", dismissOutside)
      document.removeEventListener("keydown", dismissEscape)
    }
  }, [])
  return (
    <div
      className="ui-popover-anchor"
      ref={open ? bindDismiss : undefined}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false)
      }}
    >
      <button
        ref={trigger}
        type="button"
        aria-label={label}
        aria-expanded={open}
        aria-controls={detailsId}
        onClick={() => setOpen((value) => !value)}
        className="ui-info-popover-trigger rounded-control text-label-tertiary hover:text-label"
      >
        {icon}
      </button>
      {open && (
        <section id={detailsId} aria-label={label} className="ui-menu ui-info-popover">
          {children(() => setOpen(false))}
        </section>
      )}
    </div>
  )
}
