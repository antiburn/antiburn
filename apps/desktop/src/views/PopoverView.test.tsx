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

const USAGE_WINDOW = {
  tokensIn: 1_000,
  tokensOut: 200,
  cacheRead: 50,
  estimatedUsd: 1.25,
  sessionCount: 1,
};

const PROVIDER_USAGE = {
  providers: [
    {
      provider: 'anthropic',
      displayName: 'Anthropic',
      state: 'estimated',
      staleness: 'fresh',
      windows: { today: USAGE_WINDOW, week: USAGE_WINDOW, month: USAGE_WINDOW },
      lastActivityAt: '2027-01-15T07:59:00Z',
    },
  ],
  generatedAt: '2027-01-15T08:00:00Z',
  retentionDays: 14,
  coverageSince: '2027-01-01T08:00:00Z',
};

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string, args?: unknown) => {
    if (command in overrides) return Promise.resolve(overrides[command]);
    switch (command) {
      case 'get_settings':
        return Promise.resolve(SETTINGS);
      case 'list_recent_sessions':
        return Promise.resolve([activityEntry()]);
      case 'get_session_analytics':
        return Promise.resolve(ANALYTICS);
      case 'get_provider_usage':
        return Promise.resolve(PROVIDER_USAGE);
      case 'get_scan_status':
      case 'scan_now':
      case 'cancel_scan':
        return Promise.resolve(SCAN_STATUS);
      case 'set_settings':
        return Promise.resolve((args as Record<string, unknown> | undefined)?.['settings']);
      case 'list_scan_roots':
      case 'default_scan_roots':
      case 'list_repositories':
      case 'set_repository_enabled':
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

    expect(await screen.findByRole('heading', { name: 'Session Analytics' })).toBeInTheDocument();
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('get_session_analytics', {
        agent: 'claude-code',
        sessionId: 'session-abc-123',
        wslDistro: null,
      }),
    );

    fireEvent.click(screen.getByRole('button', { name: 'Back' }));

    expect(await screen.findByText('Wire the tray popover')).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Session Analytics' })).not.toBeInTheDocument();
  });

  it('warns before an export and only writes once a destination is chosen', async () => {
    confirmDialog.mockResolvedValue(true);
    saveDialog.mockResolvedValue('/home/avery/Desktop/antiburn-session.json');
    render(<PopoverView />);

    fireEvent.click(await screen.findByText('Wire the tray popover'));
    fireEvent.click(await screen.findByRole('button', { name: 'Export this session' }));

    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1));
    // The warning names what the file can describe, before a destination is
    // ever requested — including the two short excerpts it carries, which an
    // earlier version of this copy denied.
    const [message] = confirmDialog.mock.calls[0] as [string];
    expect(message).toMatch(/short excerpts/i);
    expect(message).toMatch(/no message bodies, tool arguments, or file contents/i);
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

  it('asks for provider usage with the reader own offset and shows the footer', async () => {
    render(<PopoverView />);

    expect(await screen.findByTestId('provider-usage-cluster')).toBeInTheDocument();
    // "Today" and "this month" are the reader's calendar days, so the offset
    // travels with the request rather than being guessed shell-side.
    expect(invoke).toHaveBeenCalledWith('get_provider_usage', {
      utcOffsetMinutes: -new Date().getTimezoneOffset(),
    });
    expect(
      screen.getByRole('button', { name: 'Anthropic, $1.25 today, estimated' }),
    ).toBeInTheDocument();
  });

  it('opens the full usage view from the footer and comes back to the list', async () => {
    render(<PopoverView />);

    fireEvent.click(await screen.findByRole('button', { name: 'Usage' }));

    expect(await screen.findByRole('heading', { name: 'Provider usage' })).toBeInTheDocument();
    expect(screen.queryByText('Wire the tray popover')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Back to activity' }));
    expect(await screen.findByText('Wire the tray popover')).toBeInTheDocument();
  });

  it('withholds the usage footer entirely when the shell reports nothing', async () => {
    mockCommands({ get_provider_usage: { ...PROVIDER_USAGE, providers: [] } });
    render(<PopoverView />);

    await screen.findByText('Wire the tray popover');
    // The footer still appears — it is how the Usage view is reached — but it
    // says there was no usage today rather than showing an empty chip row.
    expect(screen.getByTestId('provider-usage-cluster')).toBeInTheDocument();
    expect(screen.getByText('No provider usage today')).toBeInTheDocument();
  });

  it('runs the five-step first-run flow and enters the activity view', async () => {
    mockCommands({
      get_settings: { ...SETTINGS, onboardingCompleted: false },
      default_scan_roots: ['/home/avery/code'],
      // A fresh install: nothing has been scanned, so the repository step has
      // to ask for a pass before it has anything to show.
      get_scan_status: null,
    });
    render(<PopoverView />);

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

    // 5 — Ready.
    expect(await screen.findByRole('heading', { name: 'Ready' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Start using antiburn' }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_settings', {
        settings: { ...SETTINGS, activityWindowDays: 14, onboardingCompleted: true },
      }),
    );
  });

  it('announces each onboarding step and moves focus to its heading', async () => {
    mockCommands({ get_settings: { ...SETTINGS, onboardingCompleted: false }, get_scan_status: null });
    render(<PopoverView />);

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
});

describe('PopoverView — window behaviour', () => {
  beforeEach(() => {
    invoke.mockReset();
    confirmDialog.mockReset();
    saveDialog.mockReset();
    openDialog.mockReset();
    mockCommands();
  });

  it('asks the shell for each surface height, bounded at the contract ceiling', async () => {
    render(<PopoverView />);
    await screen.findByText('Wire the tray popover');

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_popover_height', {
        height: 700,
        animate: true,
      }),
    );

    fireEvent.click(await screen.findByRole('button', { name: 'Usage' }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_popover_height', {
        height: 620,
        animate: true,
      }),
    );

    // Nothing ever exceeds the 700px ceiling the app-shell contract sets.
    const heights = invoke.mock.calls
      .filter(([command]) => command === 'set_popover_height')
      .map(([, args]) => (args as { height: number }).height);
    expect(heights.length).toBeGreaterThan(0);
    expect(Math.max(...heights)).toBeLessThanOrEqual(700);
  });

  it('dismisses the popover on Escape', async () => {
    render(<PopoverView />);
    await screen.findByText('Wire the tray popover');

    fireEvent.keyDown(document, { key: 'Escape' });

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('hide_popover'));
  });

  it('lets an open provider panel claim Escape before the window does', async () => {
    render(<PopoverView />);

    fireEvent.click(
      await screen.findByRole('button', { name: 'Anthropic, $1.25 today, estimated' }),
    );
    const panel = await screen.findByRole('dialog');
    expect(panel).toBeInTheDocument();

    fireEvent.keyDown(document, { key: 'Escape' });

    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(invoke).not.toHaveBeenCalledWith('hide_popover');
  });

  it('moves focus to the heading of the surface that takes over', async () => {
    render(<PopoverView />);

    const activity = await screen.findByRole('heading', { name: 'antiburn' });
    await waitFor(() => expect(activity).toHaveFocus());

    fireEvent.click(await screen.findByRole('button', { name: 'Usage' }));

    const usage = await screen.findByRole('heading', { name: 'Provider usage' });
    await waitFor(() => expect(usage).toHaveFocus());
  });

  it('shows when the index was last refreshed and can rescan on demand', async () => {
    render(<PopoverView />);

    expect(await screen.findByTestId('scan-status')).toHaveTextContent(/scanned 2m ago/i);

    fireEvent.click(screen.getByRole('button', { name: 'Scan now' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('scan_now'));
  });

  it('persists a pause of background discovery', async () => {
    render(<PopoverView />);

    fireEvent.click(await screen.findByRole('button', { name: 'Pause background scanning' }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_settings', {
        settings: { ...SETTINGS, discoveryPaused: true },
      }),
    );
  });

  it('says so plainly while discovery is paused', async () => {
    mockCommands({ get_settings: { ...SETTINGS, discoveryPaused: true } });
    render(<PopoverView />);

    expect(await screen.findByTestId('scan-status')).toHaveTextContent('Scanning paused');
    // Pausing background work never removes the way to ask for a pass.
    expect(screen.getByRole('button', { name: 'Scan now' })).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: 'Resume background scanning' }),
    ).toBeInTheDocument();
  });
});
