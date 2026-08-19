// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { App } from './App';

// The shell is not attached under jsdom, so the IPC surface is stubbed at the
// module boundary: `isTauri()` reports presence and `invoke` answers commands.
const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri: () => true }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

const settings = {
  theme: 'system',
  activityWindowDays: 7,
  onboardingCompleted: false,
  launchAtLogin: false,
  autoUpdate: true,
};

describe('App', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockImplementation((command: string) => {
      switch (command) {
        case 'get_settings':
          return Promise.resolve(settings);
        case 'app_info':
          return Promise.resolve({
            appVersion: '0.1.0',
            pricingCatalogVersion: '2026-01-01',
            schemaVersion: 1,
            dataDir: '/home/avery/.antiburn',
            updatesSupported: false,
          });
        case 'default_scan_roots':
          return Promise.resolve(['/home/avery/code']);
        case 'list_scan_roots':
        case 'list_recent_sessions':
        case 'list_repositories':
          return Promise.resolve([]);
        default:
          return Promise.resolve(null);
      }
    });
    window.location.hash = '';
    delete document.documentElement.dataset['theme'];
  });

  it('renders the first-run window for the onboarding fragment', async () => {
    window.location.hash = '#/onboarding';

    render(<App />);

    expect(
      await screen.findByRole('heading', { name: 'Your sessions never leave this machine' }),
    ).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('get_settings');
  });

  it('renders the popover for an unknown fragment, not the first-run flow', async () => {
    // The default route is the popover on purpose, so a fragment nothing
    // recognizes lands somewhere real. Since onboarding moved to its own
    // window (D-25) that fallback must not draw the flow, whatever the
    // onboarding flag says.
    window.location.hash = '#/not-a-window';

    render(<App />);

    await screen.findByRole('heading', { name: 'antiburn' });
    expect(
      screen.queryByRole('heading', { name: 'Your sessions never leave this machine' }),
    ).not.toBeInTheDocument();
  });

  it('renders the settings window for the settings fragment', async () => {
    window.location.hash = '#/settings';

    render(<App />);

    expect(screen.getByRole('tablist', { name: 'Settings sections' })).toBeInTheDocument();
    expect(await screen.findByRole('heading', { name: 'General' })).toBeInTheDocument();
  });
});
