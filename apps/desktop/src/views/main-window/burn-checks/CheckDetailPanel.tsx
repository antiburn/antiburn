import { CircleAlert, Info } from "lucide-react"
import type { ReactNode } from "react"

import { BURN_CHECK_MARKS } from "../../../components/burn-checks/burnCheckMarks"

const variants = {
  success: BURN_CHECK_MARKS.clean,
  error: { Icon: CircleAlert, strokeWidth: 2, iconClass: "text-system-red" },
  info: { Icon: Info, strokeWidth: 2, iconClass: "text-label-tertiary" },
}

export function CheckDetailPanel({
  variant,
  children,
}: {
  variant: keyof typeof variants
  children: ReactNode
}) {
  const { Icon, strokeWidth, iconClass } = variants[variant]
  return (
    <div className="flex items-start gap-3 rounded-control bg-surface-card/75 p-4">
      <Icon
        size={14}
        strokeWidth={strokeWidth}
        className={`mt-0.5 shrink-0 ${iconClass}`}
        aria-hidden="true"
      />
      <div className="min-w-0 flex-1 type-callout text-label-secondary">{children}</div>
    </div>
  )
}
