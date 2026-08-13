// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen } from '@testing-library/react';
import { AlertTriangle } from 'lucide-react';
import { describe, expect, it, vi } from 'vitest';

import { Banner } from './Banner';

describe('Banner', () => {
  it('announces politely rather than interrupting', () => {
    render(<Banner message="Something needs a look." onDismiss={() => {}} />);

    const banner = screen.getByRole('status');
    expect(banner).toHaveAttribute('aria-live', 'polite');
    expect(screen.getByText('Something needs a look.')).toBeInTheDocument();
  });

  it('runs the action and does not dismiss on its own', () => {
    const onAction = vi.fn();
    const onDismiss = vi.fn();
    render(
      <Banner
        message="The system is blocking antiburn from reading widgets."
        actionLabel="Review"
        onAction={onAction}
        onDismiss={onDismiss}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Review' }));

    expect(onAction).toHaveBeenCalledTimes(1);
    // Acting on a banner is not the same as agreeing to stop being told: the
    // problem is still there until the signal that raised it goes away.
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it('is always dismissible, under a name that says what is being dismissed', () => {
    const onDismiss = vi.fn();
    render(
      <Banner
        message="Storage failed."
        onDismiss={onDismiss}
        dismissLabel="Dismiss: Storage failed."
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Dismiss: Storage failed.' }));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it('renders no action control when there is nowhere to go', () => {
    render(<Banner message="Nothing to do about it." onDismiss={() => {}} />);

    // One control only: the dismissal. A banner without an action must not
    // leave a dead button behind.
    expect(screen.getAllByRole('button')).toHaveLength(1);
    expect(screen.getByRole('button', { name: 'Dismiss' })).toBeInTheDocument();
  });

  it('treats the glyph as decoration', () => {
    render(<Banner icon={AlertTriangle} message="Blocked." onDismiss={() => {}} />);

    const glyph = screen.getByRole('status').querySelector('svg')!;
    expect(glyph.getAttribute('aria-hidden')).toBe('true');
  });

  it('carries the warning tint by default and a quiet one when asked', () => {
    const { rerender } = render(<Banner message="Blocked." onDismiss={() => {}} />);
    expect(screen.getByRole('status').className).toContain('bg-system-orange-tint');

    rerender(<Banner tone="info" message="Blocked." onDismiss={() => {}} />);
    expect(screen.getByRole('status').className).toContain('bg-surface-secondary');
  });
});
