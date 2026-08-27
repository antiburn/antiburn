import { cn } from "../../../lib/cn"
import {
  formatCompact,
  skillMcpOriginLabel,
  skillMcpStatusLabel,
  skillMcpUsage,
  type SkillMcpRow,
} from "../../../lib/presentation/sessionAnalysis"
import type { InitialContextBreakdown } from "../../../lib/types/session"
import { useSkillsMcpExpanded } from "./useSkillsMcpExpanded"

export interface SkillsMcpChartProps {
  breakdown: InitialContextBreakdown
}

export const SKILLS_MCP_COLLAPSED_ROWS = 8

/** Kind label shown before a row's name. */
function skillMcpKindLabel(kind: SkillMcpRow["kind"]): string {
  if (kind === "skill") return "Skill"
  if (kind === "mcp") return "MCP"
  return "Tool"
}

function SkillMcpRowLine({ row }: { row: SkillMcpRow }) {
  const used = row.useCount > 0
  return (
    <div className="col-span-full grid grid-cols-subgrid rounded-control -mx-1 px-1 type-caption transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover">
      <span className="flex min-w-0 items-center gap-1 truncate">
        <span className="shrink-0 text-label-tertiary">{skillMcpKindLabel(row.kind)}</span>
        <span className="truncate text-label">{row.name}</span>
      </span>
      <span className="text-center text-label-tertiary">
        {skillMcpOriginLabel(row.origin) ?? "—"}
      </span>
      <span
        className={cn("text-center text-label-tertiary tabular-nums", !used && "font-bold")}
      >
        {formatCompact(row.tokenCount)}
      </span>
      <span
        className={cn(
          "text-center",
          used ? "text-label-secondary" : "text-label-tertiary font-bold",
        )}
      >
        {skillMcpStatusLabel(row)}
      </span>
    </div>
  )
}

export function SkillsMcpChart({ breakdown }: SkillsMcpChartProps) {
  const usage = skillMcpUsage(breakdown)
  const [expanded, setExpanded] = useSkillsMcpExpanded()

  if (usage.totalTokens === 0) {
    return <p className="type-footnote text-label-tertiary">No skills, MCPs or tools loaded.</p>
  }

  const hiddenCount = usage.rows.length - SKILLS_MCP_COLLAPSED_ROWS
  const visibleRows =
    expanded || hiddenCount <= 0 ? usage.rows : usage.rows.slice(0, SKILLS_MCP_COLLAPSED_ROWS)

  return (
    <div className="grid grid-cols-[1fr_max-content_max-content_max-content] gap-x-3 gap-y-1 border-t border-separator pt-2">
      <div className="col-span-full grid grid-cols-subgrid type-caption">
        <span>Name</span>
        <span className="text-center">Source</span>
        <span className="text-center">Tokens</span>
        <span className="text-center">Check</span>
      </div>

      {visibleRows.map((row) => (
        <SkillMcpRowLine key={row.key} row={row} />
      ))}
      {hiddenCount > 0 && (
        <button
          type="button"
          onClick={() => setExpanded(!expanded)}
          className="col-span-full type-caption py-1 font-medium text-accent text-right hover:bg-surface-hover"
        >
          {expanded ? "Show less" : `Show ${hiddenCount} more`}
        </button>
      )}
    </div>
  )
}
