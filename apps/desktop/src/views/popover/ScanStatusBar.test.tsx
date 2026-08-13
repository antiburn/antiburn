// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { ScanStatus } from '../../lib/ipc';
import { ScanStatusBar, scanStatusLabel } from './ScanStatusBar';

/**
 * The line that answers "how old is what I am reading?".
 *
 * Every branch below is a state a reader can actually be in, and each one has
 * to be distinguishable from the others — "not scanned yet", "paused", and
 * "stopped" all look like an idle app if they share a sentence.
 */

function status(overrides: Partial<ScanStatus> = {}): ScanStatus {
  return {
    running: false,
    completedAgents: 0,
    totalAgents: 0,
    sessions: 0,
    finishedAt: null,
    cancelled: false,
    error: null,
    agents: [],
    ...overrides,
  };
}

function controls() {
  return {
    onRescan: vi.fn(),
    onCancel: vi.fn(),
    onTogglePaused: vi.fn(),
  };
}

describe('scanStatusLabel', () => {
  it('says nothing has happened yet before the first pass', () => {
    expect(scanStatusLabel(null, false)).toBe('Not scanned yet');
    expect(scanStatusLabel(status(), false)).toBe('Not scanned yet');
  });

  it('counts agents once there is a ratio worth printing', () => {
    expect(scanStatusLabel(status({ running: true, totalAgents: 11 }), false)).toBe('Scanning…');
    expect(
      scanStatusLabel(status({ running: true, completedAgents: 4, totalAgents: 11 }), false),
    ).toBe('Scanning… 4 of 11 agents');
  });

  it('reports the age of the index once a pass has finished', () => {
    const finishedAt = new Date(Date.now() - 3 * 60_000).toISOString();
    expect(scanStatusLabel(status({ finishedAt }), false)).toBe('Scanned 3m ago');
  });

  it('keeps a failure, a cancellation, and a pause distinguishable', () => {
    const finishedAt = new Date().toISOString();
    expect(scanStatusLabel(status({ finishedAt, error: 'disk went away' }), false)).toBe(
      'Last scan did not finish',
    );
    expect(scanStatusLabel(status({ finishedAt, cancelled: true }), false)).toBe(
      'Last scan was stopped',
    );
    expect(scanStatusLabel(status({ finishedAt }), true)).toBe('Scanning paused');
  });

  it('reports a running pass even while paused, because it is running', () => {
    // Pausing stops the *scheduler*. A pass a reader asked for still runs, and
    // saying "paused" over a spinning scan would be the wrong sentence.
    expect(scanStatusLabel(status({ running: true }), true)).toBe('Scanning…');
  });
});

describe('ScanStatusBar', () => {
  it('offers a rescan while idle and a stop while running', () => {
    const handlers = controls();
    const { rerender } = render(
      <ScanStatusBar status={status()} paused={false} {...handlers} />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Scan now' }));
    expect(handlers.onRescan).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('button', { name: 'Stop' })).not.toBeInTheDocument();

    rerender(<ScanStatusBar status={status({ running: true })} paused={false} {...handlers} />);
    fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
    expect(handlers.onCancel).toHaveBeenCalledTimes(1);
  });

  it('names the pause control for the state pressing it produces', () => {
    const handlers = controls();
    const { rerender } = render(
      <ScanStatusBar status={status()} paused={false} {...handlers} />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Pause background scanning' }));
    expect(handlers.onTogglePaused).toHaveBeenCalledWith(true);

    rerender(<ScanStatusBar status={status()} paused {...handlers} />);
    fireEvent.click(screen.getByRole('button', { name: 'Resume background scanning' }));
    expect(handlers.onTogglePaused).toHaveBeenCalledWith(false);
  });

  it('announces changes politely rather than interrupting', () => {
    render(<ScanStatusBar status={status()} paused={false} {...controls()} />);
    expect(screen.getByTestId('scan-status')).toHaveAttribute('aria-live', 'polite');
  });
});
