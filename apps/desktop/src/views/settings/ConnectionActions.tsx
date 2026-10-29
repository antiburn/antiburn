import * as DropdownMenu from "@radix-ui/react-dropdown-menu"
import { Ellipsis } from "lucide-react"
import { useRef } from "react"

export function ConnectionActions({
  id,
  label,
  disabled,
  pending,
  onEdit,
  onRemove,
}: {
  id: string
  label: string
  disabled: boolean
  pending: boolean
  onEdit: () => void
  onRemove: (opener: HTMLButtonElement) => void
}) {
  const trigger = useRef<HTMLButtonElement>(null)
  const selectedAction = useRef<((opener: HTMLButtonElement) => void) | null>(null)

  function select(action: (opener: HTMLButtonElement) => void) {
    selectedAction.current = action
  }

  return (
    <DropdownMenu.Root modal={false}>
      <DropdownMenu.Trigger asChild>
        <button
          id={id}
          ref={trigger}
          type="button"
          aria-label={`More actions for ${label}`}
          disabled={disabled}
          className="ui-push-button px-2 disabled:opacity-50"
        >
          <Ellipsis size={14} aria-hidden="true" />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          className="ui-menu min-w-32"
          align="end"
          sideOffset={4}
          onCloseAutoFocus={(event) => {
            const action = selectedAction.current
            if (!action) return
            selectedAction.current = null
            event.preventDefault()
            const opener = trigger.current
            if (!opener) return
            // The menu must unmount before the native dialog handles Escape.
            opener.focus()
            action(opener)
          }}
        >
          <DropdownMenu.Item
            className="ui-menu-item"
            disabled={disabled}
            onSelect={() => select(onEdit)}
          >
            Edit
          </DropdownMenu.Item>
          <DropdownMenu.Item
            className="ui-menu-item text-system-red-text"
            disabled={disabled || pending}
            onSelect={() => select(onRemove)}
          >
            Remove
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  )
}
