// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronRight } from "lucide-react"
import type { ReactNode } from "react"

import { cn } from "../../../lib/cn"

/**
 * Renders the icon for an agent slug. Components take this as a slot rather
 * than reaching for artwork themselves, so the presentation layer ships no
 * icon set of its own and an app can supply whatever it has (or nothing).
 */
export type AgentIconRenderer = (slug: string, size: number) => ReactNode

export interface SubagentRosterRowProps {
  /** App slug, handed to {@link renderAgentIcon}. */
  agent: string
  /** Row title — a sub-agent's task label, or an orchestrator's session title. */
  label: string
  onClick?: () => void
  renderAgentIcon?: AgentIconRenderer | undefined
}

/**
 * One agent row: leading icon, truncated label, and a chevron that appears on
 * hover to signal the row opens that agent's analysis.
 *
 * Shared by the orchestrator roster and the "launched by" row on a sub-agent
 * view.
 */
export function SubagentRosterRow({
  agent,
  label,
  onClick,
  renderAgentIcon,
}: SubagentRosterRowProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "group/srow flex w-full items-center gap-2 rounded-md text-left transition-colors duration-[var(--duration-fast)] ease-out hover:bg-system-indigo/10",
        "px-2 py-1.5",
      )}
    >
      {renderAgentIcon?.(agent, 14)}
      <span className="min-w-0 flex-1 truncate type-callout text-label">{label}</span>
      <ChevronRight
        size={13}
        aria-hidden="true"
        className="shrink-0 text-label-tertiary opacity-0 transition-opacity group-hover/srow:opacity-100"
      />
    </button>
  )
}
