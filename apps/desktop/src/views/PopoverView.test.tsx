// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { PopoverView } from './PopoverView';

/**
 * The popover's flow, driven entirely through the mocked command layer.
 *
 * These are the tests that would catch a rename on either side of the IPC
 * boundary: every assertion names a command and the arguments it is called
 * with, so a shell-side signature change fails here rather than at runtime.
 */

const invoke = vi.hoisted(() => vi.fn());
const confirmDialog = vi.hoisted(() => vi.fn());
const saveDialog = vi.hoisted(() => vi.fn());
const openDialog = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri: () => true }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  confirm: confirmDialog,
  save: saveDialog,
  open: openDialog,
}));

const SETTINGS = {
  theme: 'system' as const,
  activityWindowDays: 7,
  onboardingCompleted: true,
  launchAtLogin: false,
  autoUpdate: true,
};

function activityEntry(overrides: Record<string, unknown> = {}) {
  return {
    agent: 'claude-code',
    sessionId: 'session-abc-123',
    repo: 'widgets',
    timestamp: new Date(Date.now() - 60_000).toISOString(),
    isActive: false,
    surface: 'cli',
    wslDistro: null,
    title: 'Wire the tray popover',
    hasForkParent: false,
    forkChildCount: 0,
    subagentCount: 0,
    cost: {
      totalUsd: 1.25,
      inputUsd: 0.5,
      outputUsd: 0.5,
      cacheReadUsd: 0.15,
      cacheWriteUsd: 0.1,
    },
    models: ['claude-opus-4-6'],
    activeSecs: 900,
    durationSecs: 1800,
    ...overrides,
  };
}

const ANALYTICS = {
  // A session with nothing analyzable is enough to exercise the flow: the view
  // still renders its chrome, which is what these tests navigate through.
  summary: null,
  supportsAnalytics: true,
  title: 'Wire the tray popover',
  wslDistro: null,
  isActive: false,
  cost: null,
  models: [],
  skills: [],
  orchestration: null,
  relations: null,
  sourcePath: '/home/avery/.claude/projects/widgets/session-abc-123.jsonl',
};

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string) => {
    if (command in overrides) return Promise.resolve(overrides[command]);
    switch (command) {
      case 'get_settings':
        return Promise.resolve(SETTINGS);
      case 'list_recent_sessions':
        return Promise.resolve([activityEntry()]);
      case 'get_session_analytics':
        return Promise.resolve(ANALYTICS);
      case 'list_scan_roots':
      case 'default_scan_roots':
        return Promise.resolve([]);
      default:
        return Promise.resolve(null);
    }
  });
}

describe('PopoverView', () => {
  beforeEach(() => {
    invoke.mockReset();
    confirmDialog.mockReset();
    saveDialog.mockReset();
    openDialog.mockReset();
    mockCommands();
  });

  it('lists the sessions the shell reports for the stored window', async () => {
    render(<PopoverView />);

    expect(await screen.findByText('Wire the tray popover')).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('list_recent_sessions', { windowDays: 7 });
    // The cost pill's wording is derived here from the payload's components;
    // the shell sends values, never copy.
    expect(screen.getByLabelText('Estimated cost $1.25')).toBeInTheDocument();
  });

  it('opens a session, loads its analytics, and comes back to the list', async () => {
    render(<PopoverView />);

    fireEvent.click(await screen.findByText('Wire the tray popover'));

    expect(await screen.findByText('Session Health')).toBeInTheDocument();
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('get_session_analytics', {
        agent: 'claude-code',
        sessionId: 'session-abc-123',
        wslDistro: null,
      }),
    );

    fireEvent.click(screen.getByText('Session Health'));

    expect(await screen.findByText('Wire the tray popover')).toBeInTheDocument();
    expect(screen.queryByText('Session Health')).not.toBeInTheDocument();
  });

  it('warns before an export and only writes once a destination is chosen', async () => {
    confirmDialog.mockResolvedValue(true);
    saveDialog.mockResolvedValue('/home/avery/Desktop/antiburn-session.json');
    render(<PopoverView />);

    fireEvent.click(await screen.findByText('Wire the tray popover'));
    fireEvent.click(await screen.findByRole('button', { name: 'Export this session' }));

    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1));
    // The warning names what the file can describe, before a destination is
    // ever requested.
    const [message] = confirmDialog.mock.calls[0] as [string];
    expect(message).toMatch(/no message content/i);
    expect(confirmDialog.mock.invocationCallOrder[0]).toBeLessThan(
      saveDialog.mock.invocationCallOrder[0] as number,
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('export_session', {
        agent: 'claude-code',
        sessionId: 'session-abc-123',
        wslDistro: null,
        destPath: '/home/avery/Desktop/antiburn-session.json',
      }),
    );
  });

  it('declining the export warning never opens a save dialog', async () => {
    confirmDialog.mockResolvedValue(false);
    render(<PopoverView />);

    fireEvent.click(await screen.findByText('Wire the tray popover'));
    fireEvent.click(await screen.findByRole('button', { name: 'Export this session' }));

    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1));
    expect(saveDialog).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalledWith('export_session', expect.anything());
  });

  it('confirms a removal, deletes only local records, and returns to the list', async () => {
    confirmDialog.mockResolvedValue(true);
    render(<PopoverView />);

    fireEvent.click(await screen.findByText('Wire the tray popover'));
    fireEvent.click(await screen.findByRole('button', { name: 'Delete this session' }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('delete_session_data', {
        agent: 'claude-code',
        sessionId: 'session-abc-123',
        wslDistro: null,
      }),
    );
    const [message] = confirmDialog.mock.calls[0] as [string];
    expect(message).toMatch(/transcript file is not touched/i);
    expect(await screen.findByText('Wire the tray popover')).toBeInTheDocument();
  });

  it('reveals the provider transcript rather than a copy of it', async () => {
    render(<PopoverView />);

    fireEvent.click(await screen.findByText('Wire the tray popover'));
    fireEvent.click(await screen.findByRole('button', { name: 'Reveal in file manager' }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('reveal_source', {
        path: '/home/avery/.claude/projects/widgets/session-abc-123.jsonl',
      }),
    );
  });

  it('runs the first-run flow and starts scanning when it finishes', async () => {
    mockCommands({
      get_settings: { ...SETTINGS, onboardingCompleted: false },
      default_scan_roots: ['/home/avery/code'],
      set_settings: { ...SETTINGS, onboardingCompleted: true },
    });
    render(<PopoverView />);

    fireEvent.click(await screen.findByRole('button', { name: 'Continue' }));
    expect(await screen.findByRole('heading', { name: 'Where to look' })).toBeInTheDocument();
    // The engine's own default roots are listed, so the reader can see the
    // common cases are already covered.
    expect(screen.getByText('/home/avery/code')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Start scanning' }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_settings', {
        settings: { ...SETTINGS, onboardingCompleted: true },
      }),
    );
  });
});
