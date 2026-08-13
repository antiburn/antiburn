// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { ProviderUsagePayload, ProviderUsageWindowPayload } from '../../lib/ipc';
import { ProviderUsageCluster } from './ProviderUsageCluster';

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

function provider(overrides: Partial<ProviderUsagePayload> = {}): ProviderUsagePayload {
  const today = usageWindow({ estimatedUsd: 1.25, tokensIn: 1_000, sessionCount: 1 });
  return {
    provider: 'anthropic',
    displayName: 'Anthropic',
    state: 'estimated',
    staleness: 'fresh',
    windows: { today, week: today, month: today },
    lastActivityAt: new Date().toISOString(),
    ...overrides,
  };
}

/** Providers ranked below the first, so the overflow affordance appears. */
function ranked(count: number): ProviderUsagePayload[] {
  return Array.from({ length: count }, (_, index) => {
    const window = usageWindow({ estimatedUsd: count - index, sessionCount: 1 });
    return provider({
      provider: `p${index}`,
      displayName: `Provider ${index}`,
      windows: { today: window, week: window, month: window },
    });
  });
}

describe('ProviderUsageCluster', () => {
  it('shows a chip per provider used today, with the figure in its name', () => {
    render(
      <ProviderUsageCluster
        onOpenSettings={vi.fn()}
        providers={[provider()]}
        onViewAll={vi.fn()}
      />,
    );

    // The chip renders a glyph and a number; everything a reader needs to know
    // about what that number *is* has to be in the accessible name.
    expect(
      screen.getByRole('button', { name: 'Anthropic, $1.25 today, estimated' }),
    ).toBeInTheDocument();
  });

  it('shows a token count when the provider could not be priced', () => {
    const window = usageWindow({ tokensIn: 12_000, sessionCount: 2 });
    render(
      <ProviderUsageCluster
        providers={[
          provider({
            state: 'observed',
            windows: { today: window, week: window, month: window },
          }),
        ]}
        onViewAll={vi.fn()}
        onOpenSettings={vi.fn()}
      />,
    );

    expect(
      screen.getByRole('button', { name: 'Anthropic, 12.0k today, observed' }),
    ).toBeInTheDocument();
  });

  it('carries staleness into the chip name rather than only into a color', () => {
    render(
      <ProviderUsageCluster
        providers={[
          provider({
            staleness: 'stale',
            lastActivityAt: new Date(Date.now() - 2 * 86_400_000).toISOString(),
          }),
        ]}
        onViewAll={vi.fn()}
        onOpenSettings={vi.fn()}
      />,
    );

    expect(
      screen.getByRole('button', {
        name: /anthropic, \$1\.25 today, estimated, last used 2d ago/i,
      }),
    ).toBeInTheDocument();
  });

  it('says so honestly when nothing was used today', () => {
    const idle = provider({
      windows: {
        today: usageWindow(),
        week: usageWindow({ tokensIn: 5 }),
        month: usageWindow(),
      },
    });
    render(
      <ProviderUsageCluster onOpenSettings={vi.fn()} providers={[idle]} onViewAll={vi.fn()} />,
    );

    expect(screen.getByText('No provider usage today')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /anthropic/i })).not.toBeInTheDocument();
  });

  it('collapses everything past the chip budget into one overflow affordance', () => {
    const onViewAll = vi.fn();
    render(
      <ProviderUsageCluster
        onOpenSettings={vi.fn()}
        providers={ranked(5)}
        onViewAll={onViewAll}
        maxVisible={3}
      />,
    );

    expect(screen.getAllByRole('button', { name: /^Provider \d/ })).toHaveLength(3);
    const overflow = screen.getByRole('button', { name: 'Show 2 more providers' });
    fireEvent.click(overflow);
    expect(onViewAll).toHaveBeenCalledTimes(1);
  });

  it('opens a provider panel on click and closes it on a second click', () => {
    render(
      <ProviderUsageCluster
        onOpenSettings={vi.fn()}
        providers={[provider()]}
        onViewAll={vi.fn()}
      />,
    );
    const chip = screen.getByRole('button', { name: /anthropic/i });

    expect(chip).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(chip);

    const panel = screen.getByRole('dialog', { name: 'Anthropic' });
    expect(panel).toBeInTheDocument();
    expect(chip).toHaveAttribute('aria-expanded', 'true');
    expect(chip).toHaveAttribute('aria-controls', panel.id);
    // The panel states what the figures are, not how much of an allowance is left.
    expect(screen.getByText(/priced on this device/i)).toBeInTheDocument();

    fireEvent.click(chip);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('closes the panel on Escape and on a press outside it', () => {
    render(
      <ProviderUsageCluster
        onOpenSettings={vi.fn()}
        providers={[provider()]}
        onViewAll={vi.fn()}
      />,
    );
    const chip = screen.getByRole('button', { name: /anthropic/i });

    fireEvent.click(chip);
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();

    fireEvent.click(chip);
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('leads to the full view from the panel, and to settings from the gear', () => {
    const onViewAll = vi.fn();
    const onOpenSettings = vi.fn();
    render(
      <ProviderUsageCluster
        onOpenSettings={onOpenSettings}
        providers={[provider()]}
        onViewAll={onViewAll}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /anthropic/i }));
    fireEvent.click(screen.getByRole('button', { name: 'All provider usage' }));
    expect(onViewAll).toHaveBeenCalledTimes(1);
    // Navigating away also dismisses the panel, so returning shows the list.
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Open settings' }));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
    expect(onViewAll).toHaveBeenCalledTimes(1);
  });

  it('moves focus into the panel it opens', () => {
    render(
      <ProviderUsageCluster
        onOpenSettings={vi.fn()}
        providers={[provider()]}
        onViewAll={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /anthropic/i }));

    // `role="dialog"` promises containment. A reader who is not in the dialog
    // has been told about a surface they cannot reach.
    const panel = screen.getByRole('dialog');
    expect(panel.contains(document.activeElement)).toBe(true);
    expect(screen.getByRole('button', { name: 'All provider usage' })).toHaveFocus();
  });

  it('holds Tab inside the panel while it is open', () => {
    render(
      <ProviderUsageCluster
        onOpenSettings={vi.fn()}
        providers={[provider()]}
        onViewAll={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /anthropic/i }));
    const panel = screen.getByRole('dialog');
    const viewAll = screen.getByRole('button', { name: 'All provider usage' });

    // Forwards from the last control wraps to the first rather than escaping
    // into the footer behind the dialog.
    fireEvent.keyDown(document, { key: 'Tab' });
    expect(panel.contains(document.activeElement)).toBe(true);
    expect(viewAll).toHaveFocus();

    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true });
    expect(panel.contains(document.activeElement)).toBe(true);
  });

  it('returns focus to the chip that opened the panel', () => {
    render(
      <ProviderUsageCluster
        onOpenSettings={vi.fn()}
        providers={[provider()]}
        onViewAll={vi.fn()}
      />,
    );
    const chip = screen.getByRole('button', { name: /anthropic/i });

    fireEvent.click(chip);
    fireEvent.keyDown(document, { key: 'Escape' });

    // Not <body>: dismissing a dialog must put the reader back where they were.
    expect(chip).toHaveFocus();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('claims Escape so a host does not also act on it', () => {
    render(
      <ProviderUsageCluster
        onOpenSettings={vi.fn()}
        providers={[provider()]}
        onViewAll={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /anthropic/i }));
    // `fireEvent` returns false when a handler called `preventDefault`, which
    // is the signal the popover uses to leave one Escape closing one thing.
    const notCancelled = fireEvent.keyDown(document, { key: 'Escape', cancelable: true });
    expect(notCancelled).toBe(false);
  });

  it('renders a reserved state correctly if one ever arrives', () => {
    // v1 never emits `live`, but the contract says a view must not fall through
    // to an unknown branch the day a reviewed passive source does.
    render(
      <ProviderUsageCluster
        onOpenSettings={vi.fn()}
        providers={[provider({ state: 'live' })]}
        onViewAll={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /anthropic, \$1\.25 today, live/i }));
    expect(screen.getByText('Live')).toBeInTheDocument();
    expect(screen.getByText(/reported this usage directly/i)).toBeInTheDocument();
  });
});
