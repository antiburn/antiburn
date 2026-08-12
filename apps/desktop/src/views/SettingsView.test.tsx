// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
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
const checkForUpdate = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri: () => true }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: openDialog,
  confirm: vi.fn(),
  save: vi.fn(),
}));
vi.mock('@tauri-apps/plugin-updater', () => ({ check: checkForUpdate }));

const SETTINGS = {
  theme: 'system' as const,
  activityWindowDays: 7,
  onboardingCompleted: true,
  launchAtLogin: false,
  autoUpdate: true,
};

const INFO = {
  appVersion: '0.1.0',
  pricingCatalogVersion: '2026-08-12',
  schemaVersion: 1,
  dataDir: '/home/avery/Library/Application Support/ai.antiburn.desktop',
  updatesSupported: false,
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
    checkForUpdate.mockReset();
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
    expect(button).toBeDisabled();
    expect(
      screen.getByText(/updater is installed in packaged releases only/i),
    ).toBeInTheDocument();

    fireEvent.click(button);
    expect(checkForUpdate).not.toHaveBeenCalled();
  });

  it('runs a real check when the build carries the updater', async () => {
    mockCommands({ app_info: { ...INFO, updatesSupported: true } });
    checkForUpdate.mockResolvedValue(null);
    render(<SettingsView />);

    fireEvent.click(screen.getByRole('tab', { name: 'Updates' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Check for updates' }));

    await waitFor(() => expect(checkForUpdate).toHaveBeenCalledTimes(1));
    expect(await screen.findByText('Up to date')).toBeInTheDocument();
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
