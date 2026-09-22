import type { ComponentPropsWithoutRef, Ref } from "react"

import { cn } from "../../lib/cn"
import type { BurnCheckPresentation } from "../../lib/presentation/burnChecks"
import { BurnCheckIndicator } from "./BurnCheckIndicator"

type BurnCheckStatusProps = {
  presentation: BurnCheckPresentation
  hideText?: boolean
  ref?: Ref<HTMLSpanElement>
} & Omit<ComponentPropsWithoutRef<"span">, "children" | "aria-label">

export function BurnCheckStatus({
  presentation,
  hideText = false,
  ref,
  className,
  ...triggerProps
}: BurnCheckStatusProps) {
  const phrase = presentation.compactPhrase

  return (
    <span
      {...triggerProps}
      ref={ref}
      className={cn(
        "flex min-w-0 items-center gap-x-2 font-mono type-footnote tabular-nums text-label-secondary",
        className,
      )}
      aria-label={presentation.accessibleDescription}
    >
      <span
        className={hideText ? "inline-flex shrink-0" : "inline-flex shrink-0 translate-y-px"}
        data-burn-check-indicator-wrap=""
      >
        <BurnCheckIndicator presentation={presentation} size={16} />
      </span>
      {!hideText && (
        <span className="min-w-0 truncate">
          <span
            className={
              phrase.outcome === "failed"
                ? "font-semibold! text-burn-check-failure-text"
                : phrase.outcome === "passed"
                  ? "font-semibold! text-burn-check-pass-fill"
                  : undefined
            }
          >
            {phrase.text}
          </span>
        </span>
      )}
    </span>
  )
}
