import type { ChecksCategoryPayload } from "../../../lib/insightsIpc"
import { cn } from "../../../lib/cn"
import { checkRowPresentation } from "../../checks/checkUi"

/** The config checks on the Overview. Failing checks come first, then
 *  passing checks. Each row shows the check and its status. */
export function OverviewConfigChecks({
  failures,
  passing,
}: {
  failures: readonly ChecksCategoryPayload[]
  passing: readonly ChecksCategoryPayload[]
}) {
  const rows = [...failures, ...passing]
  return (
    <section
      aria-label="Config checks"
      className="flex flex-col gap-(--space-sm) rounded-(--radius-popover) bg-surface-sidebar p-(--space-xl)"
    >
      <h2 className="type-footnote font-semibold text-label-secondary">Config checks</h2>
      <ul className="flex flex-col gap-(--space-xs)">
        {rows.map((check) => {
          const row = checkRowPresentation(check)
          const failed = check.lifecycle === "failing"
          return (
            <li
              key={check.id}
              className={cn(
                "flex items-center gap-(--space-md) rounded-control px-(--space-md) py-(--space-sm)",
                failed && "bg-brand-tint/10",
              )}
            >
              <row.Icon
                aria-hidden="true"
                size={15}
                className={cn(
                  "shrink-0",
                  row.iconTone
                    .split(" ")
                    .filter((token) => token.startsWith("text-"))
                    .join(" "),
                )}
              />
              <span className="type-body font-semibold text-label">{row.label}</span>
              <span className="min-w-0 flex-1 truncate type-body text-label-secondary">
                {row.summary}
              </span>
              <span
                className={cn(
                  "shrink-0 type-footnote",
                  failed ? "text-brand-tint" : "text-label-secondary",
                )}
              >
                {failed ? "needs fix" : "passing"}
              </span>
            </li>
          )
        })}
      </ul>
    </section>
  )
}
