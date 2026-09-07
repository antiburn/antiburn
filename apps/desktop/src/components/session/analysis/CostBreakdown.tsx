import { ChevronDown, ChevronRight } from "lucide-react"

import {
  costBreakdownRows,
  costFigureLabel,
  formatCost,
  formatTime,
  formatTokensShort,
} from "../../../lib/presentation/sessionAnalysis"
import {
  resultComponentCost,
  type LocalSessionCost,
} from "../../../lib/presentation/sessionCosts"
import { modelRunShortNames } from "../../../lib/presentation/models"
import type { SubagentMember } from "../../../lib/types/session"
import { TruncatedText } from "../../presentation/TruncatedText"
import { RowInfo } from "./RowInfo"
import { toggleSubagentsExpanded, useSubagentsExpanded } from "./subagentsExpandedStore"

interface CostBreakdownSplit {
  /** The orchestrator's own transcript. */
  parent: LocalSessionCost
  /** Every sub-agent it launched, together. */
  subagents: LocalSessionCost
  subagentCount: number
  /** One entry per sub-agent, for the expandable detail rows below the total. */
  members: SubagentMember[]
  /** Unix seconds of the session's own first transcript event, or null when
   * unknown. Each detail row shows its sub-agent's start relative to this. */
  sessionStartedAtEpoch: number | null
}

export interface CostBreakdownProps {
  /**
   * The selected result. Its headline and every component row describe one
   * subject, so the rows always sum to the total shown.
   */
  cost: LocalSessionCost
  /**
   * Parent/sub-agents split, for an inclusive orchestration result. Omit it
   * for any other subject, where there is nothing to break apart.
   */
  split?: CostBreakdownSplit | null
  /** Open one sub-agent's own analysis, from an expanded detail row. */
  onOpenSubagent?: (subagentId: string, label: string) => void
}

/* One sentence per billable component, for that row's info button. */
const COST_ROW_HELP: Record<string, string> = {
  Input: "Fresh tokens sent to the model, billed at the full input price.",
  Output: "Tokens the model wrote back. The highest price per token.",
  "Cache read":
    "Context replayed from the provider's cache, at about 10% of the fresh-input price.",
  "Cache write":
    "Tokens stored into the provider's cache, at about 25% over the fresh-input price.",
  "Parent agent": "What the orchestrating session itself spent.",
}

/**
 * Percent of `totalUsd` that `usd` accounts for, as a whole percent. A
 * positive share under half a percent reads `"<1%"` rather than rounding away
 * to `"0%"`. `"—"` stands in when the total itself is zero, where a percent
 * is undefined.
 */
function formatSharePct(usd: number, totalUsd: number): string {
  if (!(totalUsd > 0)) return "—"
  const pct = (usd / totalUsd) * 100
  if (pct <= 0) return "0%"
  if (pct < 0.5) return "<1%"
  return `${Math.round(pct)}%`
}

/** One row: a label, its token count, its USD cost, and its share of the total. */
function CostRowLine({
  label,
  usd,
  tokens,
  totalUsd,
}: {
  label: string
  usd: number
  tokens: number
  totalUsd: number
}) {
  const help = COST_ROW_HELP[label]
  return (
    <div className="group col-span-full grid grid-cols-subgrid rounded-control -mx-1 px-1 py-0.5 type-body transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover">
      <span className="flex min-w-0 items-center gap-1 text-label-tertiary">
        <span className="truncate">{label}</span>
        {help && <RowInfo label={label} body={help} />}
      </span>
      <span className="text-right text-label-tertiary tabular-nums">
        {formatTokensShort(tokens)}
      </span>
      <span className="pr-1.5 text-right text-label tabular-nums">{formatCost(usd)}</span>
      <span className="text-right text-label-tertiary tabular-nums">
        {formatSharePct(usd, totalUsd)}
      </span>
    </div>
  )
}

function memberTokenTotal(member: SubagentMember): number | null {
  if (!member.tokens) return null
  const { inputTokens, outputTokens, cacheReadTokens, cacheCreationTokens } = member.tokens
  return inputTokens + outputTokens + cacheReadTokens + cacheCreationTokens
}

function formatMemberStart(
  member: SubagentMember,
  sessionStartedAtEpoch: number | null,
): string {
  if (sessionStartedAtEpoch == null || member.startedAtEpoch == null) return "—"
  const elapsedSecs = Math.max(0, member.startedAtEpoch - sessionStartedAtEpoch)
  return formatTime(elapsedSecs)
}

/**
 * One sub-agent, indented under the "N sub-agents" row it expands from. The
 * whole row is a button: a click opens that sub-agent's own analysis. A
 * chevron marks that, because nothing else said the row went anywhere. A
 * sub-agent that has no priced cost yet shows a dash in its cost and percent
 * columns rather than a false zero.
 */
function SubagentMemberRow({
  member,
  totalUsd,
  sessionStartedAtEpoch,
  onOpenSubagent,
}: {
  member: SubagentMember
  totalUsd: number
  sessionStartedAtEpoch: number | null
  onOpenSubagent?: ((subagentId: string, label: string) => void) | undefined
}) {
  const modelLabel = modelRunShortNames(member.modelRuns).join(" · ")
  const tokens = memberTokenTotal(member)
  const usd = member.cost?.totalUsd ?? null

  return (
    <button
      type="button"
      aria-label={`Open the analysis for ${member.label}`}
      onClick={() => onOpenSubagent?.(member.subagentId, member.label)}
      className="group col-span-full grid grid-cols-subgrid gap-y-0.5 text-label-tertiary type-body py-2 border-t border-separator transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover"
    >
      <span className="col-span-full flex min-w-0 items-center gap-x-1 text-left">
        <span className="tabular-nums">
          [{formatMemberStart(member, sessionStartedAtEpoch)}]
        </span>
        <TruncatedText className="min-w-0 flex-1" text={member.label} />
        <ChevronRight
          size={12}
          aria-hidden="true"
          className="shrink-0 transition-transform duration-[var(--duration-fast)] ease-out group-hover:translate-x-0.5"
        />
      </span>

      <span className="truncate text-left">{modelLabel}</span>
      <span className="text-right tabular-nums">
        {tokens != null ? formatTokensShort(tokens) : "—"}
      </span>
      <span className="pr-1.5 text-right text-label tabular-nums">
        {usd != null ? formatCost(usd) : "—"}
      </span>
      <span className="text-right tabular-nums">
        {usd != null ? formatSharePct(usd, totalUsd) : "—"}
      </span>
    </button>
  )
}

/**
 * The "N sub-agents" split row. It expands, when it has a roster to show, into
 * one {@link SubagentMemberRow} per sub-agent. A roster-less split (an older
 * result the engine has not repriced yet) falls back to the plain summary row,
 * since there is nothing underneath it to disclose.
 *
 * `members` keeps the engine's own order: the earliest-started sub-agent
 * first. This view does not re-sort it.
 */
function SubagentsSplitRow({
  label,
  usd,
  tokens,
  totalUsd,
  members,
  sessionStartedAtEpoch,
  onOpenSubagent,
}: {
  label: string
  usd: number
  tokens: number
  totalUsd: number
  members: SubagentMember[]
  sessionStartedAtEpoch: number | null
  onOpenSubagent?: ((subagentId: string, label: string) => void) | undefined
}) {
  const expanded = useSubagentsExpanded()

  if (members.length === 0) {
    return <CostRowLine label={label} usd={usd} tokens={tokens} totalUsd={totalUsd} />
  }

  const Chevron = expanded ? ChevronDown : ChevronRight

  return (
    <>
      <button
        type="button"
        onClick={toggleSubagentsExpanded}
        aria-expanded={expanded}
        className="col-span-full grid grid-cols-subgrid rounded-control -mx-1 my-1 px-1 type-body transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover"
      >
        <span className="min-w-0 flex items-center gap-x-1 text-label-tertiary">
          <Chevron size={12} aria-hidden="true" className="shrink-0" />
          {label}
        </span>
        <span className="text-right text-label-tertiary tabular-nums">
          {formatTokensShort(tokens)}
        </span>
        <span className="pr-1.5 text-right text-label tabular-nums">{formatCost(usd)}</span>
        <span className="text-right text-label-tertiary tabular-nums">
          {formatSharePct(usd, totalUsd)}
        </span>
      </button>

      {expanded &&
        members.map((member) => (
          <SubagentMemberRow
            key={member.subagentId}
            member={member}
            totalUsd={totalUsd}
            sessionStartedAtEpoch={sessionStartedAtEpoch}
            onOpenSubagent={onOpenSubagent}
          />
        ))}
    </>
  )
}

/**
 * The billable-component breakdown beneath the tokens chart.
 *
 * Every figure here is the on-device estimate for one exact subject: a split
 * only appears when the caller has parent and sub-agent results drawn from the
 * same computation as the headline, because mixing sources would produce rows
 * that do not add up. The token and percent columns share that same subject,
 * so every row's percent reads as a share of the one total shown in the footer.
 */
export function CostBreakdown({ cost, split, onOpenSubagent }: CostBreakdownProps) {
  const rows = costBreakdownRows(resultComponentCost(cost))
  const totalUsd = cost.totalCostUsd

  return (
    <div className="grid grid-cols-[1fr_max-content_max-content_max-content] gap-x-3 gap-y-1">
      {split && (
        <div className="col-span-full grid grid-cols-subgrid mb-1 border-b border-separator pb-1">
          <CostRowLine
            label="Parent agent"
            usd={split.parent.totalCostUsd}
            tokens={split.parent.totalTokens}
            totalUsd={totalUsd}
          />
          <SubagentsSplitRow
            label={`${split.subagentCount} sub-agent${split.subagentCount === 1 ? "" : "s"}`}
            usd={split.subagents.totalCostUsd}
            tokens={split.subagents.totalTokens}
            totalUsd={totalUsd}
            members={split.members}
            sessionStartedAtEpoch={split.sessionStartedAtEpoch}
            onOpenSubagent={onOpenSubagent}
          />
        </div>
      )}

      {rows.map((row) => (
        <CostRowLine
          key={row.label}
          label={row.label}
          usd={row.usd}
          tokens={row.tokens ?? 0}
          totalUsd={totalUsd}
        />
      ))}

      <div className="col-span-full grid grid-cols-subgrid mt-1 border-t border-separator pt-1 type-body">
        <span className="text-label-tertiary">{costFigureLabel(cost.isActive)}</span>
        <span className="text-right text-label-tertiary tabular-nums">
          {formatTokensShort(cost.totalTokens)}
        </span>
        <span className="flex justify-end">
          <span className="flex shrink-0 items-center rounded-full bg-surface-secondary px-1.5 py-px type-body font-medium text-label tabular-nums">
            {formatCost(cost.totalCostUsd)}
          </span>
        </span>
        <span className="text-right text-label-tertiary tabular-nums">
          {formatSharePct(cost.totalCostUsd, totalUsd)}
        </span>
      </div>
    </div>
  )
}
