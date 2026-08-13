// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SourcesPane } from './SourcesPane';

/**
 * The Sources pane owns scanning now that the popover carries no status line:
 * the status sentence, the on-demand rescan, and the paused wording all live
 * here, so this file keeps the coverage those popover tests used to hold.
 */

const invoke = vi.hoisted(() => vi.fn());
const openDialog = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri: () => true }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: openDialog }));

const SETTINGS = {
  theme: 'system' as const,
  activityWindowDays: 7,
  onboardingCompleted: true,
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
  invoke.mockImplementation((command: string) => {
    if (command in overrides) return Promise.resolve(overrides[command]);
    switch (command) {
      case 'get_settings':
        return Promise.resolve(SETTINGS);
      case 'get_scan_status':
      case 'scan_now':
        return Promise.resolve(SCAN_STATUS);
      case 'list_scan_roots':
      case 'list_repositories':
        return Promise.resolve([]);
      default:
        return Promise.resolve(null);
    }
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  mockCommands();
});

describe('SourcesPane scanning', () => {
  it('shows when the index was last refreshed and can rescan on demand', async () => {
    render(<SourcesPane />);

    expect(await screen.findByText(/scanned 2m ago/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Rescan' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('scan_now'));
  });

  it('says so plainly while discovery is paused', async () => {
    mockCommands({ get_settings: { ...SETTINGS, discoveryPaused: true } });
    render(<SourcesPane />);

    expect(await screen.findByText('Scanning paused')).toBeInTheDocument();
    // Pausing background work never removes the way to ask for a pass.
    expect(screen.getByRole('button', { name: 'Rescan' })).toBeInTheDocument();
  });

  it('disables the rescan affordance while a scan is running', async () => {
    mockCommands({
      get_scan_status: { ...SCAN_STATUS, running: true, finishedAt: null },
    });
    render(<SourcesPane />);

    const button = await screen.findByRole('button', { name: /scanning/i });
    expect(button).toBeDisabled();
  });
});
