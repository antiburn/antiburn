// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

import { render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { SessionAnalyticsPayload } from '../../lib/ipc';
import { SessionPane, type SessionSubject } from './SessionPane';

const getSessionAnalytics = vi.hoisted(() => vi.fn());
const getSubagentAnalytics = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/plugin-dialog', () => ({
  confirm: vi.fn(),
  save: vi.fn(),
}));

vi.mock('../../components/session/SessionAnalyticsPresentation', () => ({
  SessionAnalyticsPresentation: ({
    loading,
    session,
  }: {
    loading?: boolean;
    session?: { title?: string; wslDistro?: string | null };
  }) => (
    <div>
      <span data-testid="analytics-state">{loading ? 'loading' : session?.title}</span>
      <span data-testid="analytics-environment">{session?.wslDistro ?? 'native'}</span>
    </div>
  ),
}));

vi.mock('../../lib/ipc', () => ({
  deleteSessionData: vi.fn(),
  exportSession: vi.fn(),
  getSessionAnalytics,
  getSubagentAnalytics,
  revealSource: vi.fn(),
  withPopoverHold: vi.fn((callback: () => unknown) => callback()),
}));

function analytics(title: string, wslDistro: string | null): SessionAnalyticsPayload {
  return {
    summary: null,
    supportsAnalytics: true,
    title,
    wslDistro,
    isActive: false,
    cost: null,
    models: [],
    skills: [],
    orchestration: null,
    relations: null,
    sourcePath: null,
  };
}

function subject(overrides: Partial<SessionSubject> = {}): SessionSubject {
  return {
    agent: 'claude-code',
    sessionId: 'same-session-id',
    title: 'fallback title',
    wslDistro: null,
    ...overrides,
  };
}

const noop = () => undefined;

function renderPane(current: SessionSubject) {
  return render(
    <SessionPane subject={current} onBack={noop} onOpenSession={noop} onDeleted={noop} />,
  );
}

describe('SessionPane analytics identity', () => {
  beforeEach(() => {
    getSessionAnalytics.mockReset();
    getSubagentAnalytics.mockReset();
  });

  it('refetches instead of reusing analytics when the same id moves between native and WSL', async () => {
    getSessionAnalytics.mockImplementation(
      async (_agent: string, _sessionId: string, wslDistro: string | null) =>
        analytics(wslDistro ?? 'native analytics', wslDistro),
    );

    const view = renderPane(subject());
    await waitFor(() =>
      expect(screen.getByTestId('analytics-state')).toHaveTextContent('native analytics'),
    );
    expect(getSessionAnalytics).toHaveBeenCalledTimes(1);
    expect(getSessionAnalytics).toHaveBeenLastCalledWith(
      'claude-code',
      'same-session-id',
      null,
    );

    view.rerender(
      <SessionPane
        subject={subject({ wslDistro: 'Ubuntu' })}
        onBack={noop}
        onOpenSession={noop}
        onDeleted={noop}
      />,
    );

    await waitFor(() =>
      expect(screen.getByTestId('analytics-state')).toHaveTextContent('Ubuntu'),
    );
    expect(getSessionAnalytics).toHaveBeenCalledTimes(2);
    expect(getSessionAnalytics).toHaveBeenLastCalledWith(
      'claude-code',
      'same-session-id',
      'Ubuntu',
    );
  });

  it('refetches sub-agent analytics when the launching session changes', async () => {
    getSubagentAnalytics.mockImplementation(
      async (
        _agent: string,
        parentSessionId: string,
        _subagentId: string,
        wslDistro: string | null,
      ) => analytics(parentSessionId, wslDistro),
    );

    const initial = subject({
      subagent: { parentSessionId: 'parent-one', subagentId: 'same-subagent-id' },
      sessionId: 'same-subagent-id',
    });
    const view = renderPane(initial);
    await waitFor(() =>
      expect(screen.getByTestId('analytics-state')).toHaveTextContent('parent-one'),
    );
    expect(getSubagentAnalytics).toHaveBeenCalledTimes(1);

    view.rerender(
      <SessionPane
        subject={subject({
          subagent: { parentSessionId: 'parent-two', subagentId: 'same-subagent-id' },
          sessionId: 'same-subagent-id',
        })}
        onBack={noop}
        onOpenSession={noop}
        onDeleted={noop}
      />,
    );

    await waitFor(() =>
      expect(screen.getByTestId('analytics-state')).toHaveTextContent('parent-two'),
    );
    expect(getSubagentAnalytics).toHaveBeenCalledTimes(2);
    expect(getSubagentAnalytics).toHaveBeenLastCalledWith(
      'claude-code',
      'parent-two',
      'same-subagent-id',
      null,
    );
  });
});
