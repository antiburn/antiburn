import type { ReactNode } from "react"

import type { BurnCheckPresentation } from "../../lib/presentation/burnChecks"

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
      passed: phrase.outcome === "passed",
    })),
    ...presentation.contextPhrases
      .filter((text) => text !== "Evidence incomplete")
      .map((text) => ({
        key: `context-${text}`,
        text,
        failure: false,
        passed: false,
      })),
  ]
  return (
    <span
      className="grid min-h-10 min-w-0 flex-1 grid-cols-[minmax(0,1fr)_max-content] items-center gap-x-3 px-[var(--space-md)] py-[var(--space-sm)] text-left"
      aria-label={presentation.accessibleDescription}
    >
      <span className="min-w-0">
        <span className="flex min-w-0 items-baseline gap-2">
          <span
            data-testid="burn-check-headline"
            className="truncate type-headline font-semibold! text-label"
          >
            All burn checks
          </span>
          <span className="shrink-0 type-footnote font-normal! text-label-tertiary">
            30 days
          </span>
        </span>
        {secondary.length > 0 ? (
          <span className="block truncate type-footnote tabular-nums text-label-secondary">
            {secondary.map((phrase, index) => (
              <span key={phrase.key}>
                {index > 0 && <span aria-hidden="true"> · </span>}
                <span
                  className={
                    phrase.failure
                      ? "font-mono font-semibold! text-burn-check-failure-text"
                      : phrase.passed
                        ? "font-mono text-burn-check-pass-fill"
                        : undefined
                  }
                >
                  {phrase.text}
                </span>
              </span>
            ))}
          </span>
        ) : (
          <span className="block truncate type-footnote text-label-secondary">
            {presentation.headline}
          </span>
        )}
      </span>
      {trailing && <span className="shrink-0">{trailing}</span>}
    </span>
  )
}
