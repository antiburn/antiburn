import type { checkResultStatus } from "../../lib/presentation/checkRowStatus"
import { cn } from "../../lib/cn"

export function CheckStatusText({ status }: { status: ReturnType<typeof checkResultStatus> }) {
  return (
    <span className="font-mono type-footnote tabular-nums">
      {status.parts.map((part, index) => (
        <span key={part.kind + index}>
          {index > 0 && <span className="text-label-tertiary"> · </span>}
          <span
            className={cn(
              part.kind === "failed"
                ? "font-semibold! text-burn-check-failure-text"
                : part.kind === "passed" && !status.parts.some((part) => part.kind === "failed")
                  ? "text-burn-check-pass-fill"
                  : "text-label-secondary",
              part.kind === "checking" && "activity-row-active inline-block",
            )}
          >
            {part.kind === "checking" ? (
              <span className="activity-row-title-shimmer inline-block" data-text={part.text}>
                {part.text}
              </span>
            ) : (
              part.text
            )}
          </span>
        </span>
      ))}
    </span>
  )
}
