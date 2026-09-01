import * as TooltipPrimitive from "@radix-ui/react-tooltip"
import {
  cloneElement,
  createContext,
  isValidElement,
  useCallback,
  useContext,
  useState,
  type ReactElement,
  type ReactNode,
  type Ref,
  type RefCallback,
} from "react"

export interface TooltipProps {
  /** Tooltip body. Rich content is allowed — the surface sizes to it. */
  label: ReactNode
  /** The trigger. Must accept a ref and spread props (Radix uses `asChild`). */
  children: ReactNode
  side?: "top" | "right" | "bottom" | "left"
  /** Hover delay before opening, in milliseconds. */
  delayMs?: number
  /**
   * Also toggle on click, not just hover and focus. For info affordances on
   * small or touch targets, where hover alone is undiscoverable.
   */
  interactive?: boolean
}

export interface SharedTooltipRegistration {
  label: ReactNode
  side: NonNullable<TooltipProps["side"]>
  delayMs: number
}

export interface SharedTooltipOwner {
  register: (node: HTMLElement, registration: SharedTooltipRegistration) => () => void
}

export const SharedTooltipOwnerContext = createContext<SharedTooltipOwner | null>(null)

function setRef<T>(ref: Ref<T> | undefined, value: T | null): void | (() => void) {
  if (typeof ref === "function") return ref(value)
  if (ref) ref.current = value
}

function SharedTooltip({
  owner,
  label,
  children,
  side,
  delayMs,
}: Required<Pick<TooltipProps, "label" | "children" | "side" | "delayMs">> & {
  owner: SharedTooltipOwner
}) {
  const child = isValidElement(children)
    ? (children as ReactElement<{
        ref?: Ref<HTMLElement> | undefined
        "data-shared-tooltip-trigger"?: string | undefined
      }>)
    : null
  const childRef = child?.props.ref
  const assignTriggerRef = useCallback<RefCallback<HTMLElement>>(
    (node) => {
      const childCleanup = setRef(childRef, node)
      if (!node) return childCleanup
      const unregister = owner.register(node, { label, side, delayMs })
      return () => {
        unregister()
        if (childCleanup) childCleanup()
        else setRef(childRef, null)
      }
    },
    [childRef, delayMs, label, owner, side],
  )

  if (!child) throw new Error("Tooltip requires one element child")
  return cloneElement(child, { ref: assignTriggerRef, "data-shared-tooltip-trigger": "" })
}

function StandaloneTooltip({
  label,
  children,
  side,
  delayMs,
  interactive,
}: Required<TooltipProps>) {
  const [open, setOpen] = useState(false)

  const rootProps = interactive ? { open, onOpenChange: setOpen } : {}

  const triggerProps = interactive
    ? {
        onClick: () => setOpen((o) => !o),
        onPointerDown: (event: React.PointerEvent) => event.preventDefault(),
      }
    : {}

  return (
    <TooltipPrimitive.Provider delayDuration={delayMs}>
      <TooltipPrimitive.Root {...rootProps}>
        <TooltipPrimitive.Trigger asChild {...triggerProps}>
          {children}
        </TooltipPrimitive.Trigger>
        <TooltipPrimitive.Portal>
          <TooltipPrimitive.Content
            side={side}
            sideOffset={4}
            collisionPadding={8}
            className="ui-tooltip max-w-[220px] whitespace-normal"
          >
            {label}
          </TooltipPrimitive.Content>
        </TooltipPrimitive.Portal>
      </TooltipPrimitive.Root>
    </TooltipPrimitive.Provider>
  )
}

/**
 * A tooltip on the platform surface style (`.ui-tooltip`).
 *
 * Accessibility, dismissal, and collision handling come from Radix; the only
 * behavior added here is the optional click toggle, which has to suppress
 * Radix's pointer-down auto-dismiss or the click would open and immediately
 * close it.
 */
export function Tooltip({
  label,
  children,
  side = "top",
  delayMs = 600,
  interactive = false,
}: TooltipProps) {
  const owner = useContext(SharedTooltipOwnerContext)
  if (owner && !interactive) {
    return (
      <SharedTooltip owner={owner} label={label} side={side} delayMs={delayMs}>
        {children}
      </SharedTooltip>
    )
  }

  return (
    <StandaloneTooltip label={label} side={side} delayMs={delayMs} interactive={interactive}>
      {children}
    </StandaloneTooltip>
  )
}
