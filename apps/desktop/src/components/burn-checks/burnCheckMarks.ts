import { CircleAlert, CircleCheck, CircleMinus, type LucideIcon } from "lucide-react"

export interface BurnCheckMark {
  Icon: LucideIcon
  strokeWidth: number
  iconClass: string
}

export const BURN_CHECK_MARKS = {
  finding: {
    Icon: CircleAlert,
    strokeWidth: 2.5,
    iconClass: "text-burn-check-failure-fill",
  },
  clean: {
    Icon: CircleCheck,
    strokeWidth: 2,
    iconClass: "text-burn-check-pass-fill",
  },
  notAssessed: {
    Icon: CircleMinus,
    strokeWidth: 2,
    iconClass: "text-burn-check-neutral",
  },
} satisfies Record<"finding" | "clean" | "notAssessed", BurnCheckMark>
