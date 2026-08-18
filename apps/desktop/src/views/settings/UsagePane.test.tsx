// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type * as Ipc from '../../lib/ipc';
import type { AppSettings, LiveUsageSummaryPayload } from '../../lib/ipc';
import { UsagePane } from './UsagePane';

const getLiveUsage = vi.hoisted(() => vi.fn());

vi.mock('../../lib/ipc', async () => {
  const actual = await vi.importActual<typeof Ipc>('../../lib/ipc');
  return { ...actual, getLiveUsage };
});

/** `isMacOS` is mocked mutably because jsdom has no operating system to ask. */
const platform = vi.hoisted(() => ({ mac: false }));
vi.mock('../../lib/platform', async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>();
  return { ...actual, isMacOS: () => platform.mac };
});

/* The preference reads and writes go through the real localStorage-backed
 * helpers; only the window calls are stubbed, since jsdom has no windows. */
const openOverlayWindow = vi.hoisted(() => vi.fn(async () => {}));
const hideOverlayWindow = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('../../lib/overlayWindow', async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>();
  return { ...actual, openOverlayWindow, hideOverlayWindow };
});

const SETTINGS = { liveUsageEnabled: false } as unknown as AppSettings;

function summary(overrides: Partial<LiveUsageSummaryPayload> = {}): LiveUsageSummaryPayload {
  return { providers: [], errors: [], generatedAt: '', ...overrides };
}

function pane(settings: Partial<AppSettings> = {}, update = vi.fn()) {
  render(
    <UsagePane settings={{ ...SETTINGS, ...settings } as AppSettings} update={update} loaded />,
  );
  return update;
}

describe('UsagePane', () => {
  beforeEach(() => {
    getLiveUsage.mockReset();
    getLiveUsage.mockResolvedValue(summary());
    platform.mac = false;
  });

  it('names both consequences of the one switch', async () => {
    // A switch with two effects has to say both, or it is not consent: it
    // makes readings possible at all *and* it lets milestone notifications fire.
    pane();
    const row = screen.getByText('Keep my plan limits current').closest('div')!;
    expect(row).toHaveTextContent(/that.s your own connection, made as you/i);
    expect(row).toHaveTextContent(/no antiburn server is involved/i);
    expect(row).toHaveTextContent(/milestone notifications fire/i);
  });

  it('says what happens with the switch off, rather than leaving it implied', async () => {
    pane();
    expect(screen.getByText('Without this').closest('div')).toHaveTextContent(
      /makes no request for you until you turn this on/i,
    );
  });

  it('writes the preference through when the switch moves', async () => {
    const update = pane();
    fireEvent.click(screen.getByRole('switch', { name: /keep my plan limits current/i }));
    expect(update).toHaveBeenCalledWith({ liveUsageEnabled: true });
  });

  it('distinguishes nothing found from nothing working', async () => {
    getLiveUsage.mockResolvedValue(summary());
    pane();
    await waitFor(() => expect(screen.getByText('No plan limits found')).toBeInTheDocument());
    expect(screen.queryByText('Could not read usage')).not.toBeInTheDocument();
  });

  it('turns each failure into something a reader could act on', async () => {
    getLiveUsage.mockResolvedValue(
      summary({ errors: [{ source: 'claude-usage-fetch', category: 'authentication' }] }),
    );
    pane();
    await waitFor(() =>
      expect(screen.getByText(/sign in again with your coding tool/i)).toBeInTheDocument(),
    );
    // And it is not reported as "nothing found", which would send the reader
    // to use their coding tool when the problem is that they are signed out of it.
    expect(screen.queryByText('No plan limits found')).not.toBeInTheDocument();
  });

  it('lists what each source can currently prove', async () => {
    getLiveUsage.mockResolvedValue(
      summary({
        providers: [
          {
            provider: 'anthropic',
            displayName: 'Anthropic',
            support: 'live',
            freshness: 'fresh',
            sourceLabel: 'Asked Claude directly',
            observedAt: new Date(Date.now() - 5 * 60_000).toISOString(),
            windows: [],
            extraUsage: null,
          },
        ],
      }),
    );
    pane();
    await waitFor(() => expect(screen.getByText('Anthropic')).toBeInTheDocument());
    expect(screen.getByText(/Asked Claude directly/)).toBeInTheDocument();
    expect(screen.getByText('Live · stated 5m ago')).toBeInTheDocument();
  });

  it('degrades to an empty list rather than throwing when the shell answers with nothing', async () => {
    getLiveUsage.mockRejectedValue(new Error('no shell'));
    pane();
    await waitFor(() => expect(screen.getByText('No plan limits found')).toBeInTheDocument());
  });
});

describe('UsagePane — floating HUD toggle', () => {
  beforeEach(() => {
    getLiveUsage.mockReset();
    getLiveUsage.mockResolvedValue(summary());
    platform.mac = true;
    localStorage.clear();
    openOverlayWindow.mockClear();
    hideOverlayWindow.mockClear();
  });

  it('offers the HUD only where its window exists', () => {
    // macOS-only for v1 (docs/deviations.md D-28): elsewhere the toggle would
    // promise a window the shell never registers.
    platform.mac = false;
    pane();
    expect(screen.queryByText('Floating HUD')).not.toBeInTheDocument();
  });

  it('opens the HUD and stores the preference when switched on', () => {
    pane();
    fireEvent.click(screen.getByRole('switch', { name: /show floating usage hud/i }));
    expect(localStorage.getItem('antiburn.showFloatingHud')).toBe('1');
    expect(openOverlayWindow).toHaveBeenCalled();
  });

  it('hides the HUD and clears the preference when switched off', () => {
    localStorage.setItem('antiburn.showFloatingHud', '1');
    pane();
    const toggle = screen.getByRole('switch', { name: /show floating usage hud/i });
    expect(toggle).toBeChecked();
    fireEvent.click(toggle);
    expect(localStorage.getItem('antiburn.showFloatingHud')).toBe('0');
    expect(hideOverlayWindow).toHaveBeenCalled();
  });
});
