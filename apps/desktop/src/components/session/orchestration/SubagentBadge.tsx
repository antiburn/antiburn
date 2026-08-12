// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Bot } from 'lucide-react';

import { CollapsibleOrchestrationCard } from './CollapsibleOrchestrationCard';
import { SubagentRosterRow, type AgentIconRenderer } from './SubagentRosterRow';

export interface SubagentBadgeProps {
  /** Orchestrator (parent) agent slug — the icon for its session row. */
  parentAgent: string;
  /** Title of the orchestrator session that launched this sub-agent, if known. */
  parentTitle?: string;
  /**
   * Open the launching orchestrator's own analytics, where its roster lives.
   * Omitted leaves the badge informational.
   */
  onOpenOrchestrator?: () => void;
  renderAgentIcon?: AgentIconRenderer | undefined;
}

/**
 * The banner at the top of a sub-agent's analytics, mirroring the orchestrator
 * card. Collapsed it marks the view as a worker that something else launched,
 * not a session the user drove; expanding it reveals the launching orchestrator
 * as a row that links up to its analytics.
 */
export function SubagentBadge({
  parentAgent,
  parentTitle,
  onOpenOrchestrator,
  renderAgentIcon,
}: SubagentBadgeProps) {
  return (
    <CollapsibleOrchestrationCard
      icon={<Bot size={13} aria-hidden="true" className="shrink-0 text-system-indigo-text" />}
      title="Autonomous sub-agent"
    >
      {onOpenOrchestrator ? (
        <>
          <div className="px-2 pb-0.5 type-caption text-label-tertiary">Launched by</div>
          <SubagentRosterRow
            agent={parentAgent}
            label={parentTitle?.trim() || 'Orchestrator session'}
            onClick={onOpenOrchestrator}
            renderAgentIcon={renderAgentIcon}
          />
        </>
      ) : (
        <div className="px-2 py-1.5 type-caption text-label-secondary">
          Ran unsupervised — not a session you drove
        </div>
      )}
    </CollapsibleOrchestrationCard>
  );
}
