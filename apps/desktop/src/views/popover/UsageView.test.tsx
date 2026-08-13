// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type {
  ProviderUsagePayload,
  ProviderUsageSummaryPayload,
  ProviderUsageWindowPayload,
} from '../../lib/ipc';
import { UsageView } from './UsageView';

function usageWindow(
  overrides: Partial<ProviderUsageWindowPayload> = {},
): ProviderUsageWindowPayload {
  return {
    tokensIn: 0,
    tokensOut: 0,
    cacheRead: 0,
    estimatedUsd: null,
    sessionCount: 0,
    ...overrides,
  };
}

/** Anthropic, busy today; OpenAI, busy only earlier in the week. */
const ANTHROPIC: ProviderUsagePayload = {
  provider: 'anthropic',
  displayName: 'Anthropic',
  state: 'estimated',
  staleness: 'fresh',
  windows: {
    today: usageWindow({ estimatedUsd: 2.5, tokensIn: 1_000, sessionCount: 1 }),
    week: usageWindow({ estimatedUsd: 8, tokensIn: 4_000, sessionCount: 4 }),
    month: usageWindow({ estimatedUsd: 8, tokensIn: 4_000, sessionCount: 4 }),
  },
  lastActivityAt: new Date().toISOString(),
};

const OPENAI: ProviderUsagePayload = {
  provider: 'openai',
  displayName: 'OpenAI',
  state: 'observed',
  staleness: 'stale',
  windows: {
    today: usageWindow(),
    week: usageWindow({ tokensIn: 20_000, sessionCount: 2 }),
    month: usageWindow({ tokensIn: 20_000, sessionCount: 2 }),
  },
  lastActivityAt: new Date(Date.now() - 3 * 86_400_000).toISOString(),
};

function summary(
  overrides: Partial<ProviderUsageSummaryPayload> = {},
): ProviderUsageSummaryPayload {
  return {
    providers: [ANTHROPIC, OPENAI],
    generatedAt: '2027-01-15T08:00:00Z',
    retentionDays: 14,
    coverageSince: '2027-01-01T08:00:00Z',
    ...overrides,
  };
}

describe('UsageView', () => {
  it('sections current work first: used-today providers under Recently used', () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />);

    expect(screen.getByRole('heading', { name: 'Usage' })).toBeInTheDocument();

    const recent = within(screen.getByRole('region', { name: 'Recently used' }));
    expect(recent.getByText('Anthropic')).toBeInTheDocument();
    expect(recent.getByText('Used today')).toBeInTheDocument();

    const rest = within(screen.getByRole('region', { name: 'All detected' }));
    expect(rest.getByText('OpenAI')).toBeInTheDocument();
    expect(rest.queryByText('Used today')).not.toBeInTheDocument();
  });

  it('shows every window on one card, with sessions beside each figure', () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />);

    const card = screen.getByText('Anthropic').closest('li');
    expect(card).not.toBeNull();
    expect(within(card!).getByText('Today')).toBeInTheDocument();
    expect(within(card!).getByText('Last 7 days')).toBeInTheDocument();
    expect(within(card!).getByText('This month')).toBeInTheDocument();
    // "$2.50" appears twice by design: the Today's-spend metric row and the
    // Today window row describe the same day.
    expect(within(card!).getAllByText('$2.50')).toHaveLength(2);
    expect(within(card!).getAllByText('$8.00')).toHaveLength(2);
    expect(within(card!).getByText('1 session')).toBeInTheDocument();
  });

  it('derives the metric block from the reader’s own windows', () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />);

    const card = screen.getByText('Anthropic').closest('li');
    expect(within(card!).getByText("Today's spend")).toBeInTheDocument();
    expect(within(card!).getByText("Today's tokens")).toBeInTheDocument();
    // 1,000 today vs (4,000 − 1,000)/6 = 500 per day → 2.0× and rising.
    expect(within(card!).getByText(/Picking up · 2\.0×/)).toBeInTheDocument();
  });

  it('marks an unpriced provider observed and shows its tokens instead of a dollar zero', () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />);

    const card = screen.getByText('OpenAI').closest('li');
    expect(card).not.toBeNull();
    expect(within(card!).getByText('Observed')).toBeInTheDocument();
    expect(within(card!).getAllByText('20.0k').length).toBeGreaterThan(0);
    expect(within(card!).getByText(/Last used 3d ago/)).toBeInTheDocument();
    // Nothing today against a real weekly baseline reads as easing off.
    expect(within(card!).getByText(/Easing · 0\.0×/)).toBeInTheDocument();
  });

  it('says how far back the windows can see, because retention is shorter than a month', () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />);

    expect(screen.getByText(/keeps 14 days of session history/)).toBeInTheDocument();
    expect(screen.getByText(/window of its most recent activity/)).toBeInTheDocument();
    expect(screen.getByText(/Not a bill/)).toBeInTheDocument();
  });

  it('is honest when there is nothing to show', () => {
    render(<UsageView summary={summary({ providers: [] })} onBack={vi.fn()} />);

    expect(screen.getByText('No local evidence yet')).toBeInTheDocument();
  });

  it('goes back to the activity list', () => {
    const onBack = vi.fn();
    render(<UsageView summary={summary()} onBack={onBack} />);

    fireEvent.click(screen.getByRole('button', { name: 'Back to activity' }));
    expect(onBack).toHaveBeenCalledTimes(1);
  });
});

describe('UsageWindowRows shares', () => {
  it('fills each bar with the window’s share of the provider’s own month', () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />);

    const card = screen.getByText('Anthropic').closest('li')!;
    // Fixture: today $2.50 of this month's $8.00 → 31% (rounded).
    expect(within(card).getByTestId('usage-share-today')).toHaveStyle({ width: '31%' });
    expect(within(card).getByTestId('usage-share-month')).toHaveStyle({ width: '100%' });
  });
});
