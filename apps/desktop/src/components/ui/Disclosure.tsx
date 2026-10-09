import { ChevronDown } from "lucide-react"
import { useId, useState, type ReactNode } from "react"

import { cn } from "../../lib/cn"

/**
 * A single collapsible disclosure: a full-width header button that reveals its
 * body. Hairline-separated and chrome-free by design — it sits directly on the
 * surface rather than inside a `Card`, for explanatory prose that would read as
 * an over-built list if every paragraph were given a container.
 *
 * Pass `defaultOpen` for an initially expanded disclosure. Use `open` and
 * `onOpenChange` when navigation controls the expanded state.
 *
 * Hand-rolled rather than pulled from a library: the whole contract here is a
 * button, `aria-expanded`, and `aria-controls`, which is not worth a
 * dependency.
 */
export function Disclosure({
  label,
  children,
  defaultOpen = false,
  open: controlledOpen,
  onOpenChange,
  className = "",
}: {
  label: string
  children: ReactNode
  defaultOpen?: boolean
  open?: boolean
  onOpenChange?: (open: boolean) => void
  className?: string
}) {
  const [localOpen, setOpen] = useState(defaultOpen)
  const open = controlledOpen ?? localOpen
  const bodyId = useId()

  return (
    <div className={cn("border-b border-separator last:border-b-0", className)}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => {
          setOpen(!open)
          onOpenChange?.(!open)
        }}
        // No hover fill and no press feedback: these sit on the bare window
        // surface as prose, not as list rows, and the global button:active
        // opacity rule (styles/controls.css) made a paragraph heading
        // twitch. The chevron rotation is the affordance.
        className="flex w-full items-center gap-3 rounded-control px-1 py-3 text-left active:opacity-100 hover:opacity-70"
      >
        <ChevronDown
          size={14}
          strokeWidth={2}
          aria-hidden="true"
          className={cn(
            "shrink-0 text-label-secondary transition-transform duration-[var(--duration-fast)] ease-out",
            open && "rotate-180",
          )}
        />
        <span className="type-body text-label">{label}</span>
      </button>
      {/* Unmounted rather than hidden when collapsed, so collapsed prose stays
          out of the accessibility tree and out of find-in-page. */}
      {open && (
        <div id={bodyId} className="type-body px-1 pb-3 text-pretty text-label-secondary">
          {children}
        </div>
      )}
    </div>
  )
}

/** A hairline-separated stack of `Disclosure`s. */
export function DisclosureGroup({
  children,
  className = "",
}: {
  children: ReactNode
  className?: string
}) {
  return <div className={cn("border-t border-separator", className)}>{children}</div>
}
