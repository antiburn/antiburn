import type { ReactNode } from "react"

import type { BurnCheckPresentation } from "../../lib/presentation/burnChecks"
import { BurnCheckIndicator } from "./BurnCheckIndicator"

export function BurnCheckSummary({
  presentation,
  trailing,
}: {
  presentation: BurnCheckPresentation
  trailing?: ReactNode
}) {
  const secondary = [
    ...presentation.breakdownPhrases.map((phrase) => ({
      key: `${phrase.outcome}-${phrase.text}`,
      text: phrase.text,
      failure: phrase.outcome === "failed",
    })),
    ...presentation.contextPhrases.map((text) => ({
      key: `context-${text}`,
      text,
      failure: false,
    })),
  ]

  return (
    <span
      className="grid min-h-11 min-w-0 flex-1 grid-cols-[24px_minmax(0,1fr)_max-content] items-center gap-x-2 px-2 py-1.5 text-left"
      aria-label={presentation.accessibleDescription}
    >
      <BurnCheckIndicator presentation={presentation} size={24} />
      <span className="min-w-0">
        <span
          className={`block truncate type-headline ${presentation.headlineTone === "failure" ? "text-burn-check-failure-text" : "text-label"}`}
        >
          {presentation.headline}
        </span>
        {secondary.length > 0 && (
          <span className="block truncate type-footnote text-label-secondary">
            {secondary.map((phrase, index) => (
              <span key={phrase.key}>
                {index > 0 && <span aria-hidden="true"> · </span>}
                <span
                  className={
                    phrase.failure ? "font-medium! text-burn-check-failure-text" : undefined
                  }
                >
                  {phrase.text}
                </span>
              </span>
            ))}
          </span>
        )}
      </span>
      {trailing && <span className="shrink-0">{trailing}</span>}
    </span>
  )
}
