import { ArrowRight } from "lucide-react"

import { renderAgentIcon } from "../../lib/agentIcon"
import { agentDisplayName, type AgentSurface } from "../../lib/presentation/agents"

export interface SessionSampleRowProps {
  title: string
  agent: string
  surface: AgentSurface
  observedAtMs: number
  busy?: boolean
  onOpen: () => void
}

/** A compact session card for views that can expose only safe session metadata. */
export function SessionSampleRow({
  title,
  agent,
  surface,
  observedAtMs,
  busy = false,
  onOpen,
}: SessionSampleRowProps) {
  return (
    <button
      type="button"
      disabled={busy}
      aria-busy={busy || undefined}
      aria-label={`Open sample session ${title}`}
      onClick={onOpen}
      className="group grid w-full grid-cols-[14px_minmax(0,1fr)_max-content] items-center gap-x-2 rounded-[var(--radius-popover)] bg-surface-card/50 px-3 py-3 text-left transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-secondary/50 active:transform-none active:opacity-100 disabled:cursor-wait disabled:opacity-60"
    >
      <span className="pt-[3px]">{renderAgentIcon(agent, 14, surface)}</span>
      <span className="min-w-0">
        <span className="block truncate type-body-large text-label">{title}</span>
        <span className="block truncate type-callout text-label-tertiary">
          {agentDisplayName(agent)} · {new Date(observedAtMs).toLocaleDateString()}
        </span>
      </span>
      <ArrowRight size={14} className="text-label-tertiary" aria-hidden="true" />
    </button>
  )
}
