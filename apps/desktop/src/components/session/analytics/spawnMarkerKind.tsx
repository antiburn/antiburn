// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Bot } from 'lucide-react';

import { scoreColor } from '../../../lib/presentation/sessionAnalytics';
import { SubagentRosterRow, type AgentIconRenderer } from '../orchestration/SubagentRosterRow';
import type { MarkerKind } from './HypnogramMarkers';

/**
 * The payload a sub-agent spawn marker carries. `open` opens that sub-agent's
 * analytics; the marker's own `progress` is where it launched on the
 * orchestrator's active-time axis.
 */
export interface SpawnPayload {
  patternScore: number;
  agent: string;
  label: string;
  open: () => void;
}

export interface SpawnMarkerKindOptions {
  renderAgentIcon?: AgentIconRenderer;
}

/**
 * The sub-agent spawn marker species: indigo dots matching the orchestration
 * card — a much stronger tint in dark mode, so they keep their punch against
 * the dark surface — with a hover card listing each sub-agent and its health
 * score.
 *
 * Adding another species means writing another kind like this; the overlay and
 * the clustering stay untouched.
 */
export function createSpawnMarkerKind(
  options: SpawnMarkerKindOptions = {},
): MarkerKind<SpawnPayload> {
  return {
    maxWidth: 270,
    dotFill: (dark) =>
      dark
        ? 'color-mix(in srgb, var(--color-system-indigo) 55%, transparent)'
        : 'color-mix(in srgb, var(--color-system-indigo) 20%, transparent)',
    tetherColor: 'var(--color-system-indigo-text)',
    textColor: 'var(--color-system-indigo-text)',
    textStroke: (dark) => (dark ? '0.5px var(--color-system-indigo-text)' : undefined),
    // A merged cluster shows its count; a lone dot shows the glyph, matching
    // the card header, so a single dot still reads as "a sub-agent".
    dotContent: (members) =>
      members.length > 1 ? members.length : <Bot size={14} aria-hidden="true" />,
    ariaLabel: (members) =>
      members.length > 1
        ? `${members.length} sub-agents spawned`
        : `Sub-agent spawned: ${members[0]?.data.label ?? 'unknown'}`,
    onLoneClick: (member) => member.data.open(),
    renderTooltip: (members, close) => (
      <div className="flex flex-col">
        {members.length > 1 && (
          <div className="flex items-center gap-1.5 px-1 pb-1">
            <Bot size={13} aria-hidden="true" className="shrink-0 text-system-indigo-text" />
            <span className="type-callout text-system-indigo-text">
              {members.length} sub-agents
            </span>
          </div>
        )}
        {/* Cap and scroll a large roster; the header above stays pinned. The
            negative right margin pulls the scrollbar to the card edge, and the
            right padding keeps the rows clear of it. */}
        <div
          className="-mr-1.5 overflow-y-auto overscroll-contain pr-2.5"
          style={{ maxHeight: 'min(260px, 55vh)' }}
        >
          {members.map((member, i) => (
            <SubagentRosterRow
              key={`member-${i}`}
              dense
              agent={member.data.agent}
              label={member.data.label}
              {...(options.renderAgentIcon ? { renderAgentIcon: options.renderAgentIcon } : {})}
              trailing={
                <span
                  className="shrink-0 tabular-nums"
                  style={{ color: scoreColor(member.data.patternScore) }}
                >
                  {member.data.patternScore}%
                </span>
              }
              onClick={() => {
                close();
                member.data.open();
              }}
            />
          ))}
        </div>
      </div>
    ),
  };
}

/** The spawn marker species with no icon renderer wired in. */
export const spawnMarkerKind: MarkerKind<SpawnPayload> = createSpawnMarkerKind();
