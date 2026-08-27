import type { LucideIcon } from "lucide-react"
import type { ReactNode } from "react"

import { cn } from "../../lib/cn"

/** `primary` is the plain label color, `secondary` the quieter one used for a
 *  trailing status word next to a row label. */
export type StatusTextTone = "primary" | "secondary"

/** One line of status: an optional 12px glyph and a footnote-sized message.
 *
 *  The single shape for "here is what is going on right now" — a check next to
 *  an installed status, a spinner while a check runs, a warning when one fails —
 *  so the same state reads the same way in every pane. The glyph is decorative;
 *  the message carries the meaning. */
export function StatusText({
  icon: Icon,
  iconClassName = "",
  iconStrokeWidth = 2,
  tone = "primary",
  className = "",
  children,
}: {
  icon?: LucideIcon
  iconClassName?: string
  /** Bumped to 2.5–3 for tiny marks such as a check, per the icon guidance. */
  iconStrokeWidth?: number
  tone?: StatusTextTone
  className?: string
  children: ReactNode
}) {
  return (
    <span
      className={cn(
        "type-footnote inline-flex items-center gap-1.5",
        tone === "secondary" ? "text-label-secondary" : "text-label",
        className,
      )}
    >
      {Icon && (
        <Icon
          size={12}
          strokeWidth={iconStrokeWidth}
          aria-hidden="true"
          className={cn("shrink-0", iconClassName)}
        />
      )}
      {children}
    </span>
  )
}
