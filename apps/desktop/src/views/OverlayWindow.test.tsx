// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type * as Ipc from '../lib/ipc';
import type { LiveUsageSummaryPayload } from '../lib/ipc';
import { OverlayWindow } from './OverlayWindow';

/* The overlay talks to the shell three ways — ipc wrappers, a direct invoke
 * for the hover region, and the window/monitor handles for positioning — and
 * jsdom has none of them, so all three are stubbed at the module boundary. */

const getLiveUsage = vi.hoisted(() => vi.fn());
const getLatestSessionActivity = vi.hoisted(() => vi.fn());
const openSettingsWindow = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('../lib/ipc', async () => {
  const actual = await vi.importActual<typeof Ipc>('../lib/ipc');
  return { ...actual, getLiveUsage, getLatestSessionActivity, openSettingsWindow };
});

const invoke = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri: () => true }));

/** The Rust cursor watcher's hover event, captured so a test can play the
 * backgrounded-app path where no DOM mouse event ever fires. */
const hover = vi.hoisted(() => ({ emit: null as ((next: boolean) => void) | null }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_event: string, handler: (e: { payload: boolean }) => void) => {
    hover.emit = (next: boolean) => handler({ payload: next });
    return () => {};
  }),
}));

const hide = vi.hoisted(() => vi.fn(async () => {}));
const setPosition = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    outerPosition: async () => ({ x: 600, y: 40 }),
    setPosition,
    hide,
  }),
  currentMonitor: async () => ({
    scaleFactor: 1,
    position: { x: 0, y: 0 },
    size: { width: 1512, height: 982 },
  }),
}));
vi.mock('@tauri-apps/api/dpi', () => ({
  LogicalPosition: class {
    x: number;
    y: number;
    constructor(x: number, y: number) {
      this.x = x;
      this.y = y;
    }
  },
}));

/** One synthetic provider with one limit window at 81%. */
function summary(): LiveUsageSummaryPayload {
  return {
    providers: [
      {
        provider: 'anthropic',
        displayName: 'Anthropic',
        support: 'live',
        freshness: 'fresh',
        sourceLabel: 'cached usage',
        observedAt: new Date().toISOString(),
        windows: [
          {
            id: 'five-hour',
            role: 'primaryShort',
            kind: 'rolling',
            scopeModel: null,
            usedPercent: 81,
            startsAt: null,
            resetsAt: new Date(Date.now() + 2 * 3_600_000).toISOString(),
            hasNonzeroUsageInCurrentPeriod: true,
            forecast: {
              unavailableReason: 'sparseHistory',
              confidence: null,
              consumptionRate: null,
              paceRatio: null,
              paceTrend: null,
              runwayAt: null,
              usedToday: null,
            },
          },
        ],
        extraUsage: null,
      },
    ],
    errors: [],
    generatedAt: new Date().toISOString(),
  };
}

/** The draggable panel root: the first element inside the full-window frame. */
function panel(container: HTMLElement): HTMLElement {
  return container.firstElementChild!.firstElementChild as HTMLElement;
}

async function expand(container: HTMLElement) {
  fireEvent.mouseEnter(panel(container));
  await waitFor(() => expect(screen.getByText('5-hour limit')).toBeInTheDocument());
}

describe('OverlayWindow', () => {
  beforeEach(() => {
    getLiveUsage.mockReset();
    getLiveUsage.mockResolvedValue(summary());
    getLatestSessionActivity.mockReset();
    getLatestSessionActivity.mockResolvedValue(null);
    openSettingsWindow.mockClear();
    hide.mockClear();
    invoke.mockClear();
    localStorage.clear();
  });

  it('marks its own body transparent, since the shared stylesheet paints it opaque', () => {
    const { unmount } = render(<OverlayWindow />);
    expect(document.body.dataset['transparentWindow']).toBe('true');
    unmount();
    expect(document.body.dataset['transparentWindow']).toBeUndefined();
  });

  it('rests collapsed: bars only, no labels, chrome faded out of reach', async () => {
    render(<OverlayWindow />);
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled());
    expect(screen.queryByText('5-hour limit')).not.toBeInTheDocument();
    // The header stays mounted so expansion is pure CSS, but collapsed it is
    // invisible and takes no clicks.
    const close = screen.getByRole('button', { name: 'Close overlay' });
    expect(close.closest('div')).toHaveClass('opacity-0', 'pointer-events-none');
  });

  it('waits out the hover-intent dwell before expanding, and collapses at once on leave', async () => {
    const { container } = render(<OverlayWindow />);
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled());

    fireEvent.mouseEnter(panel(container));
    // Passing the cursor over the HUD must not open it: nothing appears
    // synchronously, only after the 250ms dwell.
    expect(screen.queryByText('5-hour limit')).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByText('5-hour limit')).toBeInTheDocument());
    expect(screen.getByText('81%')).toBeInTheDocument();
    expect(screen.getByText(/^resets in /)).toBeInTheDocument();

    fireEvent.mouseLeave(panel(container));
    await waitFor(() => expect(screen.queryByText('5-hour limit')).not.toBeInTheDocument());
  });

  it('expands from the Rust cursor watcher too, for when the app is not focused', async () => {
    render(<OverlayWindow />);
    await waitFor(() => expect(hover.emit).not.toBeNull());
    hover.emit!(true);
    await waitFor(() => expect(screen.getByText('5-hour limit')).toBeInTheDocument());
    hover.emit!(false);
    await waitFor(() => expect(screen.queryByText('5-hour limit')).not.toBeInTheDocument());
  });

  it('draws collapsed for the length of a drag, expanded again on release', async () => {
    const { container } = render(<OverlayWindow />);
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled());
    await expand(container);

    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 });
    // An expanded panel covers whatever the HUD is being lined up against.
    await waitFor(() => expect(screen.queryByText('5-hour limit')).not.toBeInTheDocument());

    // Release is the only thing that ends the drag; the pointer is still on
    // the panel, so it re-expands.
    fireEvent.mouseUp(window);
    await waitFor(() => expect(screen.getByText('5-hour limit')).toBeInTheDocument());
  });

  it('clears the stored preference when closed from its own ✕', async () => {
    const { container } = render(<OverlayWindow />);
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled());
    localStorage.setItem('antiburn.showFloatingHud', '1');
    await expand(container);

    fireEvent.click(screen.getByRole('button', { name: 'Close overlay' }));
    // The Settings toggle reads this key, and a HUD closed here must not
    // come back on next launch claiming the toggle said so.
    expect(localStorage.getItem('antiburn.showFloatingHud')).toBe('0');
    await waitFor(() => expect(hide).toHaveBeenCalled());
  });

  it('opens settings from the wordmark instead of dragging', async () => {
    const { container } = render(<OverlayWindow />);
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled());
    await expand(container);
    fireEvent.click(screen.getByRole('button', { name: 'Open antiburn settings' }));
    expect(openSettingsWindow).toHaveBeenCalledWith('general');
  });

  it('says so when no source reports a limit, rather than drawing empty meters', async () => {
    getLiveUsage.mockResolvedValue({ providers: [], errors: [], generatedAt: '' });
    const { container } = render(<OverlayWindow />);
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled());
    expect(screen.queryByText(/no usage limits/i)).not.toBeInTheDocument();
    fireEvent.mouseEnter(panel(container));
    await waitFor(() =>
      expect(screen.getByText('No usage limits detected yet.')).toBeInTheDocument(),
    );
  });

  it('reports its drawn panel height to the Rust hover watcher', async () => {
    render(<OverlayWindow />);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        'set_overlay_hover_region',
        expect.objectContaining({ top: expect.any(Number), bottom: expect.any(Number) }),
      ),
    );
  });
});
