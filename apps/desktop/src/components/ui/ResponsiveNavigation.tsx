import { Menu, X } from "lucide-react"
import { useId, useState, type ReactNode } from "react"

/** The window owner supplies the breakpoint and navigation content. */
export function ResponsiveNavigation({
  compact,
  label,
  children,
}: {
  compact: boolean
  label: string
  children: (close: () => void) => ReactNode
}) {
  return compact ? (
    <NavigationDrawer label={label}>{children}</NavigationDrawer>
  ) : (
    children(() => undefined)
  )
}

function NavigationDrawer({
  label,
  children,
}: {
  label: string
  children: (close: () => void) => ReactNode
}) {
  const id = useId()
  const [open, setOpen] = useState(false)
  const close = () => {
    const dialog = document.getElementById(id)
    if (dialog instanceof HTMLDialogElement) dialog.close()
    setOpen(false)
    document.getElementById(`${id}-trigger`)?.focus()
  }

  return (
    <>
      <div className="responsive-navigation-bar">
        <button
          id={`${id}-trigger`}
          type="button"
          className="ui-push-button gap-2"
          aria-label={`Open ${label}`}
          aria-expanded={open}
          aria-controls={id}
          onClick={() => setOpen(true)}
        >
          <Menu size={14} aria-hidden="true" />
          Navigation
        </button>
      </div>
      {open && (
        <dialog
          id={id}
          aria-label={label}
          className="responsive-navigation-dialog"
          ref={(node) => {
            if (node && !node.open) node.showModal()
          }}
          onCancel={(event) => {
            event.preventDefault()
            close()
          }}
          onClick={(event) => {
            if (event.target === event.currentTarget) close()
          }}
        >
          <div className="responsive-navigation-content">
            <div className="flex shrink-0 justify-end p-2">
              <button type="button" className="ui-push-button gap-1" onClick={close}>
                <X size={14} aria-hidden="true" />
                Close navigation
              </button>
            </div>
            {children(close)}
          </div>
        </dialog>
      )}
    </>
  )
}
