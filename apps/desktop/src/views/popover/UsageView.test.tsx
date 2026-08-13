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

function summary(overrides: Partial<ProviderUsageSummaryPayload> = {}): ProviderUsageSummaryPayload {
  return {
    providers: [ANTHROPIC, OPENAI],
    generatedAt: '2027-01-15T08:00:00Z',
    retentionDays: 14,
    coverageSince: '2027-01-01T08:00:00Z',
    ...overrides,
  };
}

/** The headline figure, which is deliberately separate from the rows below it. */
function total() {
  return within(screen.getByRole('region', { name: 'Usage total' }));
}

describe('UsageView', () => {
  it('opens on today and lists only the providers used then', () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />);

    expect(screen.getByRole('heading', { name: 'Provider usage' })).toBeInTheDocument();
    expect(screen.getByText('Anthropic')).toBeInTheDocument();
    expect(screen.queryByText('OpenAI')).not.toBeInTheDocument();
    expect(total().getByText('Today')).toBeInTheDocument();
    expect(total().getByText('$2.50')).toBeInTheDocument();
  });

  it('switches windows and re-totals from the same snapshot', () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />);

    fireEvent.click(screen.getByRole('radio', { name: 'Week' }));

    expect(total().getByText('Last 7 days')).toBeInTheDocument();
    expect(screen.getByText('OpenAI')).toBeInTheDocument();
    // The headline is every provider's week added up; OpenAI contributes
    // tokens but no price, so the total stays the priced part.
    expect(total().getByText('$8.00')).toBeInTheDocument();
    expect(total().getByText(/6 sessions/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('radio', { name: 'Month' }));
    expect(total().getByText('This month')).toBeInTheDocument();
  });

  it('keeps the arrow-key contract of the window selector', () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />);
    const group = screen.getByRole('radiogroup', { name: 'Usage window' });

    fireEvent.keyDown(within(group).getByRole('radio', { name: 'Today' }), {
      key: 'ArrowRight',
    });
    expect(within(group).getByRole('radio', { name: 'Week' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
  });

  it('marks an unpriced provider observed and shows its tokens instead of a dollar zero', () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />);
    fireEvent.click(screen.getByRole('radio', { name: 'Week' }));

    const row = screen.getByText('OpenAI').closest('li');
    expect(row).not.toBeNull();
    expect(within(row!).getByText('Observed')).toBeInTheDocument();
    expect(within(row!).getByText('20.0k')).toBeInTheDocument();
    expect(within(row!).getByText(/Last used 3d ago/)).toBeInTheDocument();
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
    // The headline is a dash, never a $0.00 that would claim the work was free.
    expect(total().getByText('—')).toBeInTheDocument();
  });

  it('goes back to the activity list', () => {
    const onBack = vi.fn();
    render(<UsageView summary={summary()} onBack={onBack} />);

    fireEvent.click(screen.getByRole('button', { name: 'Back to activity' }));
    expect(onBack).toHaveBeenCalledTimes(1);
  });
});
