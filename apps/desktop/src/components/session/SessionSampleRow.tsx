import { ArrowRight, ArrowUpRight, ChevronRight } from "lucide-react"

import { cn } from "../../lib/cn"
import { Tooltip } from "../presentation/Tooltip"
import { renderAgentIcon } from "../../lib/agentIcon"
import { agentDisplayName, type AgentSurface } from "../../lib/presentation/agents"

export interface SessionSampleRowProps {
  title: string
  agent: string
  surface: AgentSurface
  observedAtMs: number
  appearance?: "card" | "inset"
  busy?: boolean
  trailing?: "arrow" | "chevron" | "up-right"
  onOpen: () => void
}

/** A compact session card for views that can expose only safe session metadata. */
export function SessionSampleRow({
  title,
  agent,
  surface,
  observedAtMs,
  busy = false,
  appearance = "card",
  trailing = "arrow",
  onOpen,
}: SessionSampleRowProps) {
  const TrailingIcon =
    trailing === "chevron" ? ChevronRight : trailing === "up-right" ? ArrowUpRight : ArrowRight
  return (
    <Tooltip label="Open session">
      <button
        type="button"
        disabled={busy}
        aria-busy={busy || undefined}
        aria-label={`Open sample session ${title}`}
        onClick={onOpen}
        className={cn(
          "group grid w-full grid-cols-[14px_minmax(0,1fr)_max-content] items-center gap-x-2 px-3 py-3 text-left transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-secondary/50 active:transform-none active:opacity-100 disabled:cursor-wait disabled:opacity-60",
          appearance === "inset"
            ? "rounded-control bg-surface-card"
            : "rounded-[var(--radius-popover)] bg-surface-card/50",
        )}
      >
        <span className="pt-[3px]">{renderAgentIcon(agent, 14, surface)}</span>
        <span className="min-w-0">
          <span className="block truncate type-body-large text-label">{title}</span>
          <span className="block truncate type-callout text-label-tertiary">
            {agentDisplayName(agent)} ·{" "}
            <span className="tabular-nums">{new Date(observedAtMs).toLocaleDateString()}</span>
          </span>
        </span>
        <TrailingIcon size={14} className="text-label-tertiary" aria-hidden="true" />
      </button>
    </Tooltip>
  )
}
