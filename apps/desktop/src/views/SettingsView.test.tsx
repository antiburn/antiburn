// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingsView } from './SettingsView';

/**
 * The settings window's persistence, through the mocked command layer.
 *
 * The window has no Save button, so "did it persist" is not something a reader
 * can check — these tests are what checks it.
 */

const invoke = vi.hoisted(() => vi.fn());
const openDialog = vi.hoisted(() => vi.fn());
const confirmDialog = vi.hoisted(() => vi.fn());
const checkForUpdate = vi.hoisted(() => vi.fn());
/** Shell event handlers the view subscribed to, by event name. */
const listeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());

vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri: () => true }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return () => listeners.delete(name);
  }),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: openDialog,
  confirm: confirmDialog,
  save: vi.fn(),
}));
vi.mock('@tauri-apps/plugin-updater', () => ({ check: checkForUpdate }));

/** Push a shell event at whatever subscribed to it. */
function emit(name: string, payload: unknown) {
  act(() => listeners.get(name)?.({ payload }));
}

const SETTINGS = {
  theme: 'system' as const,
  activityWindowDays: 7,
  onboardingCompleted: true,
  launchAtLogin: false,
  autoUpdate: true,
  discoveryPaused: false,
};

const INFO = {
  appVersion: '0.1.0',
  pricingCatalogVersion: '2026-08-12',
  schemaVersion: 1,
  dataDir: '/home/avery/Library/Application Support/ai.antiburn.desktop',
  indexedSessions: 42,
  databaseBytes: 3_670_016,
  updatesSupported: false,
};

const SCAN_STATUS = {
  running: false,
  completedAgents: 11,
  totalAgents: 11,
  sessions: 42,
  finishedAt: new Date(Date.now() - 5 * 60_000).toISOString(),
  cancelled: false,
  error: null,
  agents: [],
};

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
    if (command in overrides) return Promise.resolve(overrides[command]);
    switch (command) {
      case 'get_settings':
        return Promise.resolve(SETTINGS);
      case 'set_settings':
        // The store answers with what it actually stored, and that is what the
        // panes must then render.
        return Promise.resolve(args?.['settings']);
      case 'app_info':
        return Promise.resolve(INFO);
      case 'get_scan_status':
      case 'scan_now':
      case 'cancel_scan':
        return Promise.resolve(SCAN_STATUS);
      case 'list_repositories':
      case 'list_scan_roots':
      case 'refresh_repositories':
        return Promise.resolve([]);
      default:
        return Promise.resolve(null);
    }
  });
}

describe('SettingsView', () => {
  beforeEach(() => {
    invoke.mockReset();
    openDialog.mockReset();
    confirmDialog.mockReset();
    checkForUpdate.mockReset();
    listeners.clear();
    delete document.documentElement.dataset['theme'];
    mockCommands();
  });

  it('persists a theme choice and applies it to the document immediately', async () => {
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'Appearance' }));
    fireEvent.click(await screen.findByRole('radio', { name: 'Dark' }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_settings', {
        settings: { ...SETTINGS, theme: 'dark' },
      }),
    );
    // The token layer resolves the palette from this attribute, so writing it
    // *is* applying the theme.
    expect(document.documentElement.dataset['theme']).toBe('dark');
  });

  it('choosing "system" removes the override rather than writing a third value', async () => {
    mockCommands({ get_settings: { ...SETTINGS, theme: 'dark' } });
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'Appearance' }));
    await waitFor(() => expect(document.documentElement.dataset['theme']).toBe('dark'));

    fireEvent.click(await screen.findByRole('radio', { name: 'System' }));

    await waitFor(() => expect(document.documentElement.dataset['theme']).toBeUndefined());
  });

  it('persists the launch-at-login preference and says it is not enforced yet', async () => {
    render(<SettingsView />);

    const toggle = await screen.findByRole('switch', { name: 'Open antiburn at login' });
    expect(screen.getByText(/This build does not install one yet/i)).toBeInTheDocument();

    fireEvent.click(toggle);

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_settings', {
        settings: { ...SETTINGS, launchAtLogin: true },
      }),
    );
  });

  it('persists the monitoring switch as the same preference the popover pauses', async () => {
    render(<SettingsView />);

    const toggle = await screen.findByRole('switch', {
      name: 'Keep looking for new sessions',
    });
    // On by default: the stored preference is `discoveryPaused`, and this
    // control is its inverse, so a reader is never asked to reason about a
    // double negative.
    expect(toggle).toBeChecked();

    fireEvent.click(toggle);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_settings', {
        settings: { ...SETTINGS, discoveryPaused: true },
      }),
    );
  });

  it('reports what the index holds and when it last ran a historical scan', async () => {
    render(<SettingsView />);

    expect(await screen.findByText('42 · 3.5 MB')).toBeInTheDocument();
    expect(screen.getByText(/Last scanned 5m ago/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Scan now' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('scan_now'));
  });

  it('persists the activity window', async () => {
    render(<SettingsView />);

    const slider = await screen.findByRole('slider', { name: 'Days of activity to show' });
    fireEvent.change(slider, { target: { value: '14' } });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_settings', {
        settings: { ...SETTINGS, activityWindowDays: 14 },
      }),
    );
  });

  it('is honest that updates are unavailable in a build without the updater', async () => {
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'Updates' }));

    const button = await screen.findByRole('button', { name: 'Check for updates' });
    // The button exists as soon as the pane renders, but stays disabled until
    // `app_info` resolves; wait for the settled state rather than racing it.
    await waitFor(() => expect(button).toBeDisabled());
    expect(
      screen.getByText(/updater is installed in packaged releases only/i),
    ).toBeInTheDocument();

    fireEvent.click(button);
    expect(checkForUpdate).not.toHaveBeenCalled();
  });

  it('renders no automatic-update control at all in a build that cannot update', async () => {
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'Updates' }));
    await screen.findByText(/updater is installed in packaged releases only/i);

    // Not a disabled switch over a preference nothing reads — no switch.
    expect(
      screen.queryByRole('switch', { name: 'Check for updates automatically' }),
    ).not.toBeInTheDocument();
    expect(screen.getByText(/never contacts the release feed/i)).toBeInTheDocument();
  });

  it('runs a real check when the build carries the updater', async () => {
    mockCommands({ app_info: { ...INFO, updatesSupported: true } });
    checkForUpdate.mockResolvedValue(null);
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'Updates' }));
    const button = await screen.findByRole('button', { name: 'Check for updates' });
    // The button renders disabled and is enabled only once `app_info` resolves.
    // Clicking before that is swallowed, which is what used to make this test
    // fail on a cold run.
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);

    await waitFor(() => expect(checkForUpdate).toHaveBeenCalledTimes(1));
    expect(await screen.findByText('Up to date')).toBeInTheDocument();
  });

  it('offers the automatic-check preference, and persists it, when updates work', async () => {
    mockCommands({ app_info: { ...INFO, updatesSupported: true } });
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'Updates' }));
    const toggle = await screen.findByRole('switch', {
      name: 'Check for updates automatically',
    });
    // The copy describes the schedule the shell actually runs.
    expect(screen.getByText(/every six hours/i)).toBeInTheDocument();

    fireEvent.click(toggle);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_settings', {
        settings: { ...SETTINGS, autoUpdate: false },
      }),
    );
  });

  it('reflects what an automatic check found', async () => {
    mockCommands({ app_info: { ...INFO, updatesSupported: true } });
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'Updates' }));
    await screen.findByRole('switch', { name: 'Check for updates automatically' });

    emit('update:status', {
      kind: 'available',
      version: '0.2.0',
      message: null,
      checkedAt: new Date().toISOString(),
      automatic: true,
    });

    expect(await screen.findByText('Version 0.2.0 is available')).toBeInTheDocument();
    // The switch says when the schedule last ran, so "automatic" is an
    // observable fact rather than an assurance.
    expect(screen.getByText(/last checked/i)).toBeInTheDocument();
  });

  it('clears the derived index after confirming, and says what went', async () => {
    confirmDialog.mockResolvedValue(true);
    mockCommands({ clear_local_index: 12 });
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'Privacy' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Clear index…' }));

    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1));
    const [message] = confirmDialog.mock.calls[0] as [string];
    // The confirmation answers the two things a reader could reasonably fear.
    expect(message).toMatch(/transcript files are not touched/i);
    expect(message).toMatch(/rediscover/i);

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('clear_local_index'));
    expect(await screen.findByText(/Cleared 12 sessions/)).toBeInTheDocument();
  });

  it('declining the clear confirmation removes nothing', async () => {
    confirmDialog.mockResolvedValue(false);
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'Privacy' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Clear index…' }));

    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1));
    expect(invoke).not.toHaveBeenCalledWith('clear_local_index');
  });

  it('states the data-handling contract, including the two derived excerpts', async () => {
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'Privacy' }));

    expect(
      await screen.findByText(/no message text, no tool arguments, no file contents/i),
    ).toBeInTheDocument();
    expect(screen.getByText(/capped at 200 characters/i)).toBeInTheDocument();
    expect(screen.getByText(/capped at 300 characters/i)).toBeInTheDocument();
    // Deleting a provider's own files is named as a non-feature rather than
    // left as a silence a reader would have to test for.
    expect(screen.getByText(/antiburn cannot do this, by design/i)).toBeInTheDocument();
  });

  it('adds a scan folder through the directory picker', async () => {
    mockCommands({ add_scan_root: ['/home/avery/work'] });
    openDialog.mockResolvedValue('/home/avery/work');
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'Sources' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Add a folder…' }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('add_scan_root', { path: '/home/avery/work' }),
    );
    expect(await screen.findByText('/home/avery/work')).toBeInTheDocument();
  });

  it('shows where the local database lives', async () => {
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'About' }));

    expect(await screen.findByText(INFO.dataDir)).toBeInTheDocument();
    expect(screen.getByText(INFO.pricingCatalogVersion)).toBeInTheDocument();
  });
});
