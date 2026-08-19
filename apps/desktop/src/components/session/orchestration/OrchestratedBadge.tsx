// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Bot } from "lucide-react"

import { scoreColor } from "../../../lib/presentation/sessionAnalytics"
import type { LocalOrchestrationStatus } from "../../../lib/types/session"
import { CollapsibleOrchestrationCard } from "./CollapsibleOrchestrationCard"
import { SubagentRosterRow, type AgentIconRenderer } from "./SubagentRosterRow"

export interface OrchestratedBadgeProps {
  /** The sub-agents this session launched, as discovered on disk. */
  status: LocalOrchestrationStatus
  /** Open one sub-agent's own analytics. */
  onOpenSubagent: (
    parentAgent: string,
    parentSessionId: string,
    subagentId: string,
    label: string,
  ) => void
  renderAgentIcon?: AgentIconRenderer | undefined
}

/**
 * The card that appears under a session header when that session orchestrated
 * sub-agents. Collapsed it states the fan-out; expanded it lists every
 * sub-agent with its health score, each row opening that sub-agent's analytics.
 *
 * The count comes from the caller, which has already applied the fan-out gate —
 * so this component renders whatever roster it is handed and does not
 * re-derive a threshold of its own.
 */
export function OrchestratedBadge({
  status,
  onOpenSubagent,
  renderAgentIcon,
}: OrchestratedBadgeProps) {
  return (
    <CollapsibleOrchestrationCard
      icon={<Bot size={13} aria-hidden="true" className="shrink-0 text-system-indigo-text" />}
      title={`Orchestrated ${status.subagentCount} agents`}
      bodyClassName="space-y-0.5"
    >
      {status.members.map((member) => (
        <SubagentRosterRow
          key={member.subagentId}
          agent={member.agent}
          label={member.label}
          renderAgentIcon={renderAgentIcon}
          trailing={
            <span
              className="w-9 shrink-0 text-right type-caption font-medium tabular-nums"
              style={{ color: scoreColor(member.patternScore) }}
            >
              {member.patternScore}%
            </span>
          }
          onClick={() =>
            onOpenSubagent(
              status.orchestratorAgent,
              status.orchestratorSessionId,
              member.subagentId,
              member.label,
            )
          }
        />
      ))}
    </CollapsibleOrchestrationCard>
  )
}
