import { BookOpen, type LucideIcon } from "lucide-react"

import type { BurnCheckDetectorId } from "../../../lib/insightsIpc"
import { CHECK_UI } from "../../checks/checkPresentation"

interface CheckCategoryAppearance {
  Icon: LucideIcon
  className: string
}

const CHECK_CATEGORY_APPEARANCE: Record<BurnCheckDetectorId, CheckCategoryAppearance> = {
  unusedBuiltInTools: { Icon: CHECK_UI.unusedBuiltInTools.icon, className: "text-check-tools" },
  unusedMcpServers: { Icon: CHECK_UI.unusedMcpServers.icon, className: "text-check-mcp" },
  modelOverthinking: {
    Icon: CHECK_UI.modelOverthinking.icon,
    className: "text-check-overthinking",
  },
  unusedSkills: { Icon: BookOpen, className: "text-check-skills" },
  overpoweredSubagents: {
    Icon: CHECK_UI.overpoweredSubagents.icon,
    className: "text-check-subagents",
  },
  oldModelUsage: { Icon: CHECK_UI.oldModelUsage.icon, className: "text-check-old-model" },
  overuseOfFastMode: {
    Icon: CHECK_UI.overuseOfFastMode.icon,
    className: "text-check-fast-mode",
  },
  cacheChurn: { Icon: CHECK_UI.cacheChurn.icon, className: "text-check-cache" },
  sessionsOverDepth: { Icon: CHECK_UI.sessionsOverDepth.icon, className: "text-check-depth" },
  ignoredInstructions: {
    Icon: CHECK_UI.ignoredInstructions.icon,
    className: "text-check-instructions",
  },
}

/** The check's colour as a CSS value, for a tint that matches its icon. */
export function burnCheckCategoryColor(detector: BurnCheckDetectorId): string {
  return `var(--color-${CHECK_CATEGORY_APPEARANCE[detector].className.replace(/^text-/, "")})`
}

/** `bare` draws the glyph alone, with no tinted circle behind it. */
export function BurnCheckCategoryIcon({
  detector,
  bare = false,
}: {
  detector: BurnCheckDetectorId
  bare?: boolean
}) {
  const { Icon, className } = CHECK_CATEGORY_APPEARANCE[detector]

  return (
    <span
      className={
        bare
          ? `relative z-10 grid h-lh shrink-0 place-items-center type-body ${className}`
          : `burn-check-category-icon relative z-10 grid shrink-0 place-items-center justify-self-center rounded-full ${className}`
      }
      aria-hidden="true"
    >
      <Icon size={16} strokeWidth={1.9} />
    </span>
  )
}
