// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { OnboardingView } from './OnboardingView';

/**
 * The first-run window, driven entirely through the mocked command layer.
 *
 * These moved here from `PopoverView.test.tsx` when the flow moved out of the
 * popover (D-25). They assert the same things they always did — every step
 * appears, each is announced and takes focus, and the two settings writes go
 * out with the arguments the shell expects — because none of that changed;
 * only which window it happens in.
 */

const invoke = vi.hoisted(() => vi.fn());
const openDialog = vi.hoisted(() => vi.fn());
const listeners = vi.hoisted(
  () => new Map<string, ((event: { payload: unknown }) => void)[]>(),
);

vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri: () => true }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, [...(listeners.get(name) ?? []), handler]);
    return () => {
      listeners.set(
        name,
        (listeners.get(name) ?? []).filter((each) => each !== handler),
      );
    };
  }),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: openDialog }));

const SETTINGS = {
  theme: 'system' as const,
  activityWindowDays: 7,
  onboardingCompleted: false,
  launchAtLogin: false,
  autoUpdate: true,
  discoveryPaused: false,
};

const SCAN_STATUS = {
  running: false,
  completedAgents: 11,
  totalAgents: 11,
  sessions: 4,
  finishedAt: new Date(Date.now() - 120_000).toISOString(),
  cancelled: false,
  error: null,
  agents: [],
};

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string, args?: unknown) => {
    if (command in overrides) return Promise.resolve(overrides[command]);
    switch (command) {
      case 'get_settings':
        return Promise.resolve(SETTINGS);
      case 'scan_now':
      case 'cancel_scan':
        return Promise.resolve(SCAN_STATUS);
      // A fresh install: nothing has been scanned, so the repository step has
      // to ask for a pass before it has anything to show.
      case 'get_scan_status':
        return Promise.resolve(null);
      case 'set_settings':
        return Promise.resolve((args as Record<string, unknown> | undefined)?.['settings']);
      case 'list_scan_roots':
      case 'default_scan_roots':
      case 'list_repositories':
      case 'set_repository_enabled':
        return Promise.resolve([]);
      // Both return the roots as they now stand, not an acknowledgement.
      case 'add_scan_root':
        return Promise.resolve(['/home/avery/work']);
      case 'remove_scan_root':
        return Promise.resolve([]);
      default:
        return Promise.resolve(null);
    }
  });
}

describe('OnboardingView', () => {
  beforeEach(() => {
    invoke.mockReset();
    openDialog.mockReset();
    listeners.clear();
    mockCommands();
  });

  it('runs the five-step first-run flow and records that it finished', async () => {
    mockCommands({ default_scan_roots: ['/home/avery/code'] });
    render(<OnboardingView />);

    // 1 — Welcome. No account, and no promise of analytics this build does not
    // ship.
    expect(
      await screen.findByRole('heading', { name: 'Everything stays on this machine' }),
    ).toBeInTheDocument();
    expect(screen.getByText(/no usage data collected/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

    // 2 — Sources. The engine's own default roots are listed, so the reader can
    // see the common cases are already covered.
    expect(await screen.findByRole('heading', { name: 'Where to look' })).toBeInTheDocument();
    expect(screen.getByText('/home/avery/code')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

    // 3 — Repositories, which needs a discovery pass to have something to show.
    expect(await screen.findByRole('heading', { name: 'What to include' })).toBeInTheDocument();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('scan_now'));
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

    // 4 — Historical scan: the window choice, and the pass with a way out.
    expect(await screen.findByRole('heading', { name: 'Historical scan' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('radio', { name: '14 days' }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_settings', {
        settings: { ...SETTINGS, onboardingCompleted: false, activityWindowDays: 14 },
      }),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

    // 5 — Ready, which says where the app is about to go. The notification that
    // repeats it is the shell's, keyed off the write below.
    expect(await screen.findByRole('heading', { name: 'Ready' })).toBeInTheDocument();
    expect(screen.getByText(/in the menu bar from here on/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Start using antiburn' }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_settings', {
        settings: { ...SETTINGS, activityWindowDays: 14, onboardingCompleted: true },
      }),
    );
  });

  it('announces each step and moves focus to its heading', async () => {
    render(<OnboardingView />);

    const welcome = await screen.findByRole('heading', {
      name: 'Everything stays on this machine',
    });
    await waitFor(() => expect(welcome).toHaveFocus());
    expect(screen.getByText('Step 1 of 5')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

    const sources = await screen.findByRole('heading', { name: 'Where to look' });
    await waitFor(() => expect(sources).toHaveFocus());
    expect(screen.getByText('Step 2 of 5')).toBeInTheDocument();
  });

  it('opens the folder picker without holding the popover open', async () => {
    // The hold exists because the popover hides when it loses focus. This is a
    // decorated window, so asking for it would be asking the shell to guard a
    // window that needs no guarding.
    openDialog.mockResolvedValue('/home/avery/work');
    render(<OnboardingView />);

    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    fireEvent.click(await screen.findByRole('button', { name: /Add a folder/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('add_scan_root', expect.anything()),
    );
    expect(invoke).not.toHaveBeenCalledWith('begin_popover_hold');
    expect(invoke).not.toHaveBeenCalledWith('end_popover_hold');
  });

  it('does not close the window on Escape', async () => {
    // Escape dismissed the popover, which was right for a transient tray
    // surface and wrong for a decorated window in the middle of a task.
    render(<OnboardingView />);
    await screen.findByRole('heading', { name: 'Everything stays on this machine' });

    fireEvent.keyDown(document, { key: 'Escape' });

    expect(invoke).not.toHaveBeenCalledWith('hide_popover');
    expect(
      screen.getByRole('heading', { name: 'Everything stays on this machine' }),
    ).toBeInTheDocument();
  });
});
