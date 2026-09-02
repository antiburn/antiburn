import { cn } from "../../../lib/cn"
import {
  formatCompact,
  skillMcpOriginLabel,
  skillMcpStatusLabel,
  skillMcpUsage,
  type SkillMcpRow,
} from "../../../lib/presentation/sessionAnalysis"
import type { InitialContextBreakdown } from "../../../lib/types/session"

export interface SkillsMcpChartProps {
  breakdown: InitialContextBreakdown
}

/** Kind label shown on a row's detail line. */
function skillMcpKindLabel(kind: SkillMcpRow["kind"]): string {
  if (kind === "skill") return "Skill"
  if (kind === "mcp") return "MCP"
  return "Tool"
}

/* Token levels where an unused source's status steps up in heat. */
const UNUSED_WARM_TOKENS = 10_000
const UNUSED_CRITICAL_TOKENS = 40_000

/* The use count where a "Used ×N" status turns red: heavy repeat use is
   where a session's tool spend concentrates. */
const USED_HOT_COUNT = 20

/**
 * Ink for a source's status word. A heavily used source reads red, because
 * its repeat calls carry a large token bill. An unused source steps up in
 * heat with the tokens it burned without a use. A deferred tool stays
 * neutral: it cost close to nothing.
 */
function statusClass(row: SkillMcpRow): string | null {
  if (row.useCount >= USED_HOT_COUNT) return "text-system-red-text"
  if (row.useCount > 0 || row.deferred) return null
  if (row.tokenCount >= UNUSED_CRITICAL_TOKENS) return "text-system-red-text"
  if (row.tokenCount >= UNUSED_WARM_TOKENS) return "text-system-orange"
  return "text-context-warning"
}

/**
 * One source as a two-line cell: the name with its token count, then a
 * muted detail line. The cell shape matches the session list, at half the
 * card wash. Not a button and no hover wash — the row does nothing.
 */
function SkillMcpRowLine({ row }: { row: SkillMcpRow }) {
  const statusInk = statusClass(row)
  const origin = skillMcpOriginLabel(row.origin)
  return (
    <div className="flex flex-col gap-y-0.5 rounded-[var(--radius-popover)] bg-surface-card/50 px-3 py-2 type-body">
      <span className="flex items-baseline justify-between gap-x-3">
        <span className="min-w-0 truncate font-semibold text-label">{row.name}</span>
        <span className="shrink-0 text-label-tertiary tabular-nums">
          {formatCompact(row.tokenCount)}
        </span>
      </span>
      <span className="flex min-w-0 items-baseline gap-x-1 text-label-tertiary">
        <span>{skillMcpKindLabel(row.kind)}</span>
        {origin && (
          <>
            <span aria-hidden="true">·</span>
            <span>{origin}</span>
          </>
        )}
        <span aria-hidden="true">·</span>
        <span className={cn(statusInk)}>{skillMcpStatusLabel(row)}</span>
      </span>
    </div>
  )
}

/**
 * The full skills, MCPs and tools list. Every source renders as a two-line
 * cell: the list is the tab's whole content, so it hides nothing behind a
 * disclosure and needs no column headers.
 */
export function SkillsMcpChart({ breakdown }: SkillsMcpChartProps) {
  const usage = skillMcpUsage(breakdown)

  if (usage.totalTokens === 0) {
    return <p className="type-footnote text-label-tertiary">No skills, MCPs or tools loaded.</p>
  }

  return (
    <div className="flex flex-col gap-y-1.5">
      {usage.rows.map((row) => (
        <SkillMcpRowLine key={row.key} row={row} />
      ))}
    </div>
  )
}
