import * as TooltipPrimitive from "@radix-ui/react-tooltip"
import { useState, type ReactNode } from "react"

export interface TooltipProps {
  /** Tooltip body. Rich content is allowed — the surface sizes to it. */
  label: ReactNode
  /** The trigger. Must accept a ref and spread props (Radix uses `asChild`). */
  children: ReactNode
  side?: "top" | "right" | "bottom" | "left"
  /** Where the surface sits along the trigger edge. */
  align?: "start" | "center" | "end"
  /** Shift along the trigger edge, in pixels. Ignored when align is center. */
  alignOffset?: number
  /** Hover delay before opening, in milliseconds. */
  delayMs?: number
  /**
   * Also toggle on click, not just hover and focus. For info affordances on
   * small or touch targets, where hover alone is undiscoverable.
   */
  interactive?: boolean
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
  align = "center",
  alignOffset = 0,
  delayMs = 600,
  interactive = false,
}: TooltipProps) {
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
            align={align}
            alignOffset={alignOffset}
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
