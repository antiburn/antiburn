import { Fragment, type ComponentPropsWithoutRef, type Ref } from "react"

import type { BurnCheckPresentation } from "../../lib/presentation/burnChecks"
import { BurnCheckIndicator } from "./BurnCheckIndicator"

type BurnCheckStatusProps = {
  presentation: BurnCheckPresentation
  hideText?: boolean
  omitUnassessed?: boolean
  ref?: Ref<HTMLSpanElement>
} & Omit<ComponentPropsWithoutRef<"span">, "children" | "aria-label">

export function BurnCheckStatus({
  presentation,
  hideText = false,
  omitUnassessed = false,
  ref,
  ...triggerProps
}: BurnCheckStatusProps) {
  const isPassingVerdict =
    presentation.state === "allPassed" || presentation.state === "assessedPassed"
  const compactPhrases = (
    omitUnassessed
      ? presentation.compactPhrases.filter((phrase) => phrase.outcome !== "unassessed")
      : presentation.compactPhrases
  ).map((phrase) =>
    omitUnassessed && isPassingVerdict && phrase.outcome === "passed"
      ? { ...phrase, text: `All ${presentation.counts.passed} passed` }
      : phrase,
  )

  return (
    <span
      {...triggerProps}
      ref={ref}
      className="flex min-w-0 items-center gap-x-2 font-mono type-footnote tabular-nums text-label-secondary"
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
          {compactPhrases.map((phrase, index) => (
            <Fragment key={`${phrase.outcome}-${phrase.text}`}>
              {index > 0 && (
                <span
                  className="mx-0.5 inline-block"
                  aria-hidden="true"
                  data-burn-check-separator=""
                >
                  ·
                </span>
              )}
              <span
                className={
                  phrase.outcome === "failed"
                    ? "font-semibold! text-burn-check-failure-text"
                    : phrase.outcome === "passed"
                      ? isPassingVerdict
                        ? "font-semibold! text-burn-check-pass-fill"
                        : "text-burn-check-pass-fill"
                      : undefined
                }
              >
                {phrase.text}
              </span>
            </Fragment>
          ))}
        </span>
      )}
    </span>
  )
}
