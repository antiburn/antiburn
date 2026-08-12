// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  inclusiveCostSubject,
  type LocalSessionCost,
} from '../../lib/presentation/sessionCosts';
import type {
  ActiveSessionsSummary,
  PhaseSegment,
  SessionBucket,
  SessionMetrics,
} from '../../lib/types/session';
import {
  SessionAnalyticsPresentation,
  type SessionAnalyticsPresentationProps,
} from './SessionAnalyticsPresentation';

afterEach(cleanup);

const EVEN_MIX = {
  implementing: 0.5,
  testing: 0.1,
  exploring: 0.2,
  thinking: 0.15,
  disruption: 0.05,
};

function bucket(over: Partial<SessionBucket> = {}): SessionBucket {
  return {
    dominantPhase: 'implementing',
    distribution: EVEN_MIX,
    tokensIn: 1000,
    tokensOut: 200,
    contextTokens: 40_000,
    isCompactionBoundary: false,
    ...over,
  };
}

function segment(over: Partial<PhaseSegment> = {}): PhaseSegment {
  return {
    phase: 'implementing',
    activeMs: 60_000,
    tokensIn: 1000,
    tokensOut: 200,
    contextTokens: 40_000,
    ...over,
  };
}

function metrics(over: Partial<SessionMetrics> = {}): SessionMetrics {
  return {
    agent: 'claude-code',
    sessionId: 'session-1',
    durationSecs: 3600,
    activeSecs: 1800,
    eventCount: 42,
    tokensIn: 120_000,
    tokensOut: 8_000,
    peakContextTokens: 90_000,
    contextFraction: 0.45,
    contextAvailable: true,
    contextWindow: 200_000,
    toolMix: { edit: 10, read: 8, search: 3, test: 2, bash: 5, other: 1 },
    grepCount: 3,
    disruptionCount: 1,
    phaseDistribution: EVEN_MIX,
    patternScore: 82,
    signals: [],
    buckets: [bucket(), bucket()],
    segments: [segment()],
    ...over,
  };
}

function summary(over: Partial<ActiveSessionsSummary> = {}): ActiveSessionsSummary {
  return {
    sessionCount: 1,
    avgDurationSecs: 3600,
    avgActiveSecs: 1800,
    avgPatternScore: 82,
    phaseDistribution: EVEN_MIX,
    toolMix: { edit: 10, read: 8, search: 3, test: 2, bash: 5, other: 1 },
    grepTotal: 3,
    tokensInTotal: 120_000,
    tokensOutTotal: 8_000,
    peakContextTokens: 90_000,
    contextAvailable: true,
    contextWindow: 200_000,
    buckets: [bucket(), bucket()],
    signals: ['Repeated failed edits'],
    sessions: [metrics()],
    ...over,
  };
}

function cost(totalCostUsd = 2.4): LocalSessionCost {
  return {
    subject: inclusiveCostSubject('claude-code', 'session-1'),
    inputTokens: 1,
    outputTokens: 2,
    cacheReadTokens: 3,
    cacheCreationTokens: 4,
    totalTokens: 10,
    inputCostUsd: 0.3,
    outputCostUsd: 0.8,
    cacheReadCostUsd: 1.1,
    cacheWriteCostUsd: 0.2,
    totalCostUsd,
    isActive: false,
  };
}

function view(over: Partial<SessionAnalyticsPresentationProps> = {}) {
  const props: SessionAnalyticsPresentationProps = {
    summary: summary(),
    onBack: () => {},
    session: { agent: 'claude-code', sessionId: 'session-1', title: 'Fix the flaky test' },
    ...over,
  };
  return render(<SessionAnalyticsPresentation {...props} />);
}

describe('SessionAnalyticsPresentation — chrome', () => {
  it('renders the whole card hierarchy for a settled session', () => {
    view();
    expect(screen.getByText('Fix the flaky test')).toBeTruthy();
    expect(screen.getByText('Session rhythm')).toBeTruthy();
    expect(screen.getByText('Modes')).toBeTruthy();
    expect(screen.getByText('Pattern health')).toBeTruthy();
    expect(screen.getByText('Tokens')).toBeTruthy();
    expect(screen.getByText('Context')).toBeTruthy();
    expect(screen.getByText('Tools')).toBeTruthy();
  });

  it('navigates back through the callback', () => {
    const onBack = vi.fn();
    view({ onBack });
    fireEvent.click(screen.getByText('Session Health'));
    expect(onBack).toHaveBeenCalledOnce();
  });

  it('disables traversal that has no adjacent session, and wires the arrow keys', () => {
    const onNext = vi.fn();
    view({ onNext });
    expect(screen.getByLabelText('Newer session').hasAttribute('disabled')).toBe(true);
    expect(screen.getByLabelText('Older session').hasAttribute('disabled')).toBe(false);

    fireEvent.keyDown(document, { key: 'ArrowRight' });
    expect(onNext).toHaveBeenCalledOnce();
    fireEvent.keyDown(document, { key: 'ArrowLeft' });
    expect(onNext).toHaveBeenCalledOnce();
  });

  it('leaves the arrow keys alone while typing', () => {
    const onNext = vi.fn();
    view({ onNext });
    const input = document.createElement('input');
    document.body.appendChild(input);
    input.focus();
    fireEvent.keyDown(input, { key: 'ArrowRight' });
    expect(onNext).not.toHaveBeenCalled();
    input.remove();
  });

  it('summarizes the live aggregate instead of one session', () => {
    view({ session: undefined, summary: summary({ sessionCount: 3 }) });
    expect(screen.getByText('3 live sessions')).toBeTruthy();
    expect(screen.getByText(/Averaged/)).toBeTruthy();
    expect(screen.queryByLabelText('Newer session')).toBeNull();
  });
});

describe('SessionAnalyticsPresentation — states', () => {
  it('holds the skeleton back on a fast load and shows it on a slow one', () => {
    vi.useFakeTimers();
    try {
      const { rerender } = render(
        <SessionAnalyticsPresentation summary={null} loading onBack={() => {}} />,
      );
      expect(screen.queryByTestId('session-analytics-skeleton')).toBeNull();

      act(() => {
        vi.advanceTimersByTime(250);
      });
      expect(screen.getByTestId('session-analytics-skeleton')).toBeTruthy();

      // Once shown it holds for its minimum-visible window even after the
      // load finishes, so it cannot flicker.
      rerender(
        <SessionAnalyticsPresentation summary={summary()} loading={false} onBack={() => {}} />,
      );
      expect(screen.getByTestId('session-analytics-skeleton')).toBeTruthy();

      act(() => {
        vi.advanceTimersByTime(500);
      });
      expect(screen.queryByTestId('session-analytics-skeleton')).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('reports a failure without pretending the session was empty', () => {
    view({ summary: null, error: true });
    expect(screen.getByText("Couldn't read this session.")).toBeTruthy();
    expect(screen.queryByText('No session health available')).toBeNull();
  });

  it('explains an empty session, and an unsupported agent differently', () => {
    const { unmount } = view({ summary: summary({ sessionCount: 0 }) });
    expect(screen.getByText('No session health available')).toBeTruthy();
    unmount();

    view({
      summary: summary({ sessionCount: 0 }),
      supportsAnalytics: false,
      session: { agent: 'kiro', sessionId: 's1' },
    });
    expect(screen.getByText(/Session health for Kiro sessions/)).toBeTruthy();
  });

  it('blames the fork parent when a fork has no activity of its own', () => {
    view({
      summary: summary({ sessionCount: 0 }),
      relations: {
        parent: { identity: { agent: 'claude-code', sessionId: 'p1' }, available: true },
        children: [],
      },
    });
    expect(screen.getByText(/This fork has no analyzable child activity yet/)).toBeTruthy();
  });

  it('still shows the price of a session it could not analyze', () => {
    view({
      summary: summary({ sessionCount: 0 }),
      costBadge: { totalUsd: 2.4, figureLabel: 'Estimated cost' },
    });
    expect(screen.getByLabelText('Estimated cost $2.40')).toBeTruthy();
  });
});

describe('SessionAnalyticsPresentation — session facts', () => {
  it('shows the local cost badge and its breakdown', () => {
    view({
      cost: cost(),
      costBadge: { totalUsd: 2.4, figureLabel: 'Estimated cost' },
    });
    expect(screen.getByLabelText('Estimated cost $2.40')).toBeTruthy();
    expect(screen.getByText('Input')).toBeTruthy();
    // The pill and the breakdown headline are the same figure, by design.
    expect(screen.getAllByText('$2.40').length).toBeGreaterThan(1);
  });

  it('marks a WSL session origin in the header', () => {
    view({
      session: { agent: 'claude-code', sessionId: 's1', title: 'T', wslDistro: 'Ubuntu-24.04' },
    });
    expect(
      screen.getByLabelText('Found in Ubuntu-24.04 on Windows Subsystem for Linux'),
    ).toBeTruthy();
  });

  it('offers the orchestrator roster and opens a sub-agent from it', () => {
    const onOpenSubagent = vi.fn();
    view({
      onOpenSubagent,
      orchestration: {
        orchestrating: true,
        orchestratorAgent: 'claude-code',
        orchestratorSessionId: 'session-1',
        subagentCount: 2,
        members: [
          { agent: 'claude-code', subagentId: 'a', label: 'Investigate', patternScore: 70 },
          { agent: 'claude-code', subagentId: 'b', label: 'Write tests', patternScore: 90 },
        ],
      },
    });
    fireEvent.click(screen.getByText('Orchestrated 2 agents'));
    fireEvent.click(screen.getByText('Write tests'));
    expect(onOpenSubagent).toHaveBeenCalledWith('claude-code', 'session-1', 'b', 'Write tests');
  });

  it('marks a sub-agent view and links up to its orchestrator', () => {
    const onOpenOrchestrator = vi.fn();
    view({
      onOpenOrchestrator,
      session: {
        agent: 'claude-code',
        sessionId: 'child-1',
        subagent: {
          parentSessionId: 'parent-1',
          subagentId: 'child-1',
          parentTitle: 'Ship the release',
        },
      },
    });
    expect(screen.getByText('Sub-agent')).toBeTruthy();
    fireEvent.click(screen.getByText('Autonomous sub-agent'));
    fireEvent.click(screen.getByText('Ship the release'));
    expect(onOpenOrchestrator).toHaveBeenCalledOnce();
  });

  it('opens a fork parent through the callback', () => {
    const onOpenRelatedSession = vi.fn();
    const parent = {
      identity: { agent: 'claude-code', sessionId: 'p1' },
      title: 'Original run',
      available: true,
    };
    view({ relations: { parent, children: [] }, onOpenRelatedSession });
    fireEvent.click(screen.getByLabelText('Open fork parent'));
    expect(onOpenRelatedSession).toHaveBeenCalledWith(parent, 'Original run');
  });

  it('marks a fork parent whose transcript is gone as unavailable', () => {
    view({
      relations: {
        parent: { identity: { agent: 'claude-code', sessionId: 'p1' }, available: false },
        children: [],
      },
    });
    expect(screen.getByLabelText('Fork parent is unavailable locally')).toBeTruthy();
    expect(screen.queryByLabelText('Open fork parent')).toBeNull();
  });

  it('collects several forks behind one control', () => {
    view({
      relations: {
        parent: null,
        children: [
          { identity: { agent: 'claude-code', sessionId: 'c1' }, title: 'A', available: true },
          { identity: { agent: 'claude-code', sessionId: 'c2' }, title: 'B', available: true },
        ],
      },
    });
    expect(screen.getByLabelText('Show 2 direct forks')).toBeTruthy();
  });

  it('falls back to a short session id when a relation has no title', () => {
    const onOpenRelatedSession = vi.fn();
    view({
      relations: {
        parent: {
          identity: { agent: 'claude-code', sessionId: 'abcdef1234567' },
          available: true,
        },
        children: [],
      },
      onOpenRelatedSession,
    });
    fireEvent.click(screen.getByLabelText('Open fork parent'));
    expect(onOpenRelatedSession).toHaveBeenCalledWith(expect.anything(), 'Session abcdef1');
  });

  it('states that context occupancy is unavailable rather than charting zero', () => {
    view({ summary: summary({ contextAvailable: false }) });
    expect(screen.getByText('Context occupancy is unavailable for this model.')).toBeTruthy();
  });

  it('adds the initial-context card only for a single session that has one', () => {
    const withContext = summary({
      sessions: [
        metrics({
          initialContext: {
            trackingStatus: 'tracked',
            totalTokens: 12_000,
            sources: [
              { source: 'skill_instructions', sourceName: 'research', tokenCount: 12_000 },
            ],
          },
        }),
      ],
    });
    const { unmount } = view({ summary: withContext });
    expect(screen.getByText('Initial context')).toBeTruthy();
    unmount();

    view({ summary: summary() });
    expect(screen.queryByText('Initial context')).toBeNull();
  });
});

describe('SessionAnalyticsPresentation — host actions', () => {
  it('shows no export, delete, or reveal control until the host supplies one', () => {
    view();
    expect(screen.queryByLabelText('Export this session')).toBeNull();
    expect(screen.queryByLabelText('Delete this session')).toBeNull();
    expect(screen.queryByLabelText('Reveal in file manager')).toBeNull();
  });

  it('wires export, delete, and reveal to their callbacks', () => {
    const onExportSession = vi.fn();
    const onDeleteSession = vi.fn();
    const onRevealSource = vi.fn();
    view({ onExportSession, onDeleteSession, onRevealSource, revealLabel: 'Reveal in Finder' });

    fireEvent.click(screen.getByLabelText('Export this session'));
    fireEvent.click(screen.getByLabelText('Delete this session'));
    fireEvent.click(screen.getByLabelText('Reveal in Finder'));
    expect(onExportSession).toHaveBeenCalledOnce();
    expect(onDeleteSession).toHaveBeenCalledOnce();
    expect(onRevealSource).toHaveBeenCalledOnce();
  });

  it('renders an agent icon only from the injected renderer', () => {
    const { unmount } = view();
    expect(screen.queryByTestId('agent-icon')).toBeNull();
    unmount();

    view({ renderAgentIcon: () => <span data-testid="agent-icon" /> });
    expect(screen.getAllByTestId('agent-icon').length).toBeGreaterThan(0);
  });
});
