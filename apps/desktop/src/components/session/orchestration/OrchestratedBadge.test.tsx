// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { LocalOrchestrationStatus, SubagentMember } from '../../../lib/types/session';
import { OrchestratedBadge } from './OrchestratedBadge';

afterEach(cleanup);

function member(id: string, label: string, agent = 'claude-code'): SubagentMember {
  return { agent, subagentId: id, label, patternScore: 80 };
}

function status(members: SubagentMember[], agent = 'claude-code'): LocalOrchestrationStatus {
  return {
    orchestrating: true,
    orchestratorAgent: agent,
    orchestratorSessionId: 'parent-1',
    subagentCount: members.length,
    members,
  };
}

describe('OrchestratedBadge', () => {
  it('labels the session as having orchestrated sub-agents', () => {
    render(
      <OrchestratedBadge
        status={status([member('a', 'Investigate'), member('b', 'Write tests')])}
        onOpenSubagent={() => {}}
      />,
    );
    expect(screen.getByText('Orchestrated 2 agents')).toBeTruthy();
  });

  it('hides the roster until expanded, then reveals it', () => {
    render(
      <OrchestratedBadge
        status={status([member('a', 'Investigate the build')])}
        onOpenSubagent={() => {}}
      />,
    );
    expect(screen.queryByText('Investigate the build')).toBeNull();
    expect(screen.getByRole('button', { expanded: false })).toBeTruthy();

    fireEvent.click(screen.getByText('Orchestrated 1 agents'));
    expect(screen.getByText('Investigate the build')).toBeTruthy();
  });

  it("opens a sub-agent's analytics from a roster row", () => {
    const onOpenSubagent = vi.fn();
    render(
      <OrchestratedBadge
        status={status([member('agent-xyz', 'Refactor auth')])}
        onOpenSubagent={onOpenSubagent}
      />,
    );
    fireEvent.click(screen.getByText('Orchestrated 1 agents'));
    fireEvent.click(screen.getByText('Refactor auth'));
    expect(onOpenSubagent).toHaveBeenCalledWith(
      'claude-code',
      'parent-1',
      'agent-xyz',
      'Refactor auth',
    );
  });

  it.each(['claude-code', 'codex', 'antigravity'])(
    'preserves exact %s orchestration navigation identity',
    (agent) => {
      const onOpenSubagent = vi.fn();
      render(
        <OrchestratedBadge
          status={status([member('worker-1', 'Investigate', agent)], agent)}
          onOpenSubagent={onOpenSubagent}
        />,
      );
      fireEvent.click(screen.getByText('Orchestrated 1 agents'));
      fireEvent.click(screen.getByText('Investigate'));
      expect(onOpenSubagent).toHaveBeenCalledWith(agent, 'parent-1', 'worker-1', 'Investigate');
    },
  );

  it('colors each roster row by its health score', () => {
    render(
      <OrchestratedBadge
        status={status([
          { ...member('a', 'Healthy'), patternScore: 90 },
          { ...member('b', 'Thrashing'), patternScore: 20 },
        ])}
        onOpenSubagent={() => {}}
      />,
    );
    fireEvent.click(screen.getByText('Orchestrated 2 agents'));
    expect(screen.getByText('90%').getAttribute('style')).toContain('--color-system-green');
    expect(screen.getByText('20%').getAttribute('style')).toContain('--color-mode-disruption');
  });

  it('fills the leading icon slot from the injected renderer only', () => {
    render(
      <OrchestratedBadge
        status={status([member('a', 'Investigate')])}
        onOpenSubagent={() => {}}
        renderAgentIcon={(slug) => <span data-testid={`icon-${slug}`} />}
      />,
    );
    fireEvent.click(screen.getByText('Orchestrated 1 agents'));
    expect(screen.getByTestId('icon-claude-code')).toBeTruthy();
  });
});
