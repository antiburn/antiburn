import type { ComponentPropsWithoutRef, Ref } from "react"

import type { BurnCheckPresentation } from "../../lib/presentation/burnChecks"
import { BurnCheckIndicator } from "./BurnCheckIndicator"

type BurnCheckStatusProps = {
  presentation: BurnCheckPresentation
  ref?: Ref<HTMLSpanElement>
} & Omit<ComponentPropsWithoutRef<"span">, "children" | "aria-label">

export function BurnCheckStatus({ presentation, ref, ...triggerProps }: BurnCheckStatusProps) {
  return (
    <span
      {...triggerProps}
      ref={ref}
      className="flex min-w-0 items-center gap-x-1.5 type-footnote text-label-secondary"
      aria-label={presentation.accessibleDescription}
    >
      <BurnCheckIndicator presentation={presentation} size={16} />
      <span className="min-w-0 truncate">
        {presentation.compactPhrases.map((phrase, index) => (
          <span key={`${phrase.outcome}-${phrase.text}`}>
            {index > 0 && <span aria-hidden="true"> · </span>}
            <span
              className={
                phrase.outcome === "failed"
                  ? "font-medium! text-burn-check-failure-text"
                  : undefined
              }
            >
              {phrase.text}
            </span>
          </span>
        ))}
      </span>
    </span>
  )
}
