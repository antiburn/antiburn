// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';

import {
  inclusiveCostSubject,
  subagentsCostSubject,
  topLevelCostSubject,
  type LocalSessionCost,
} from '../../../lib/presentation/sessionCosts';
import { CostBreakdown } from './CostBreakdown';

afterEach(cleanup);

function result(totalCostUsd = 2.4, over: Partial<LocalSessionCost> = {}): LocalSessionCost {
  return {
    subject: inclusiveCostSubject('claude-code', 'parent'),
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
    ...over,
  };
}

describe('CostBreakdown', () => {
  it('renders the total and its four billable component rows', () => {
    render(<CostBreakdown cost={result()} />);
    expect(screen.getByText('Input')).toBeTruthy();
    expect(screen.getByText('Output')).toBeTruthy();
    expect(screen.getByText('Cache read')).toBeTruthy();
    expect(screen.getByText('Cache write')).toBeTruthy();
    expect(screen.getByText('$0.30')).toBeTruthy();
    expect(screen.getByText('$1.10')).toBeTruthy();
    expect(screen.getByText('$2.40')).toBeTruthy();
  });

  it('labels a live subject as projected and a settled one as estimated', () => {
    const { unmount } = render(<CostBreakdown cost={result(2.4, { isActive: true })} />);
    expect(screen.getByText('Projected cost')).toBeTruthy();
    unmount();

    render(<CostBreakdown cost={result(2.4)} />);
    expect(screen.getByText('Estimated cost')).toBeTruthy();
  });

  it('shows the inclusive parent/sub-agent split above the components', () => {
    render(
      <CostBreakdown
        cost={result(41.45)}
        split={{
          parent: result(32.95, { subject: topLevelCostSubject('claude-code', 'parent') }),
          subagents: result(8.5, { subject: subagentsCostSubject('claude-code', 'parent') }),
          subagentCount: 3,
        }}
      />,
    );
    expect(screen.getByText('Parent agent')).toBeTruthy();
    expect(screen.getByText('3 sub-agents')).toBeTruthy();
    expect(screen.getByText('$32.95')).toBeTruthy();
    expect(screen.getByText('$8.50')).toBeTruthy();
    expect(screen.getByText('$41.45')).toBeTruthy();
  });

  it('says "sub-agent" in the singular for a fan-out of one', () => {
    render(
      <CostBreakdown
        cost={result(41.45)}
        split={{ parent: result(32.95), subagents: result(8.5), subagentCount: 1 }}
      />,
    );
    expect(screen.getByText('1 sub-agent')).toBeTruthy();
  });

  it('omits the split entirely for a non-orchestration subject', () => {
    render(<CostBreakdown cost={result()} />);
    expect(screen.queryByText('Parent agent')).toBeNull();
  });
});
