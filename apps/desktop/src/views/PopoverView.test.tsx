// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
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
/** Shell event handlers the view subscribed to, by event name. */
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
vi.mock('@tauri-apps/plugin-dialog', () => ({
  confirm: confirmDialog,
  save: saveDialog,
  open: openDialog,
}));

/** Push a shell event at whatever subscribed to it. */
function emit(name: string, payload: unknown) {
  act(() => (listeners.get(name) ?? []).forEach((handler) => handler({ payload })));
}

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
};

const HEALTHY_STORAGE = { failing: false, message: null };

function repositoryPayload(overrides: Record<string, unknown> = {}) {
  return {
    key: '/home/avery/code/widgets',
    repoName: 'widgets',
    fullName: 'avery/widgets',
    status: 'accessible',
    repoRoot: '/home/avery/code/widgets',
    suspectedPath: null,
    worktreeCount: 1,
    sessionCount: 3,
    wslDistro: null,
    enabled: true,
    ...overrides,
  };
}

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
      case 'get_storage_health':
        return Promise.resolve(HEALTHY_STORAGE);
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
    listeners.clear();
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

    expect(
      await screen.findByRole('heading', { name: 'Session Analytics' }),
    ).toBeInTheDocument();
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('get_session_analytics', {
        agent: 'claude-code',
        sessionId: 'session-abc-123',
        wslDistro: null,
      }),
    );

    // The session pane is a lazy-loaded chunk: its own "Session Analytics"
    // heading briefly shares text with the Suspense fallback's, so wait for a
    // control unique to the loaded pane before treating it as ready.
    fireEvent.click(await screen.findByRole('button', { name: 'Back' }));

    expect(await screen.findByText('Wire the tray popover')).toBeInTheDocument();
    expect(
      screen.queryByRole('heading', { name: 'Session Analytics' }),
    ).not.toBeInTheDocument();
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

  it('opens the full usage view through a provider panel and comes back', async () => {
    render(<PopoverView />);

    fireEvent.click(
      await screen.findByRole('button', { name: 'Anthropic, $1.25 today, estimated' }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'All provider usage' }));

    expect(await screen.findByRole('heading', { name: 'Usage' })).toBeInTheDocument();
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

  it('refreshes usage on the shell’s popover-shown signal, independent of any scan', async () => {
    render(<PopoverView />);
    await screen.findByTestId('provider-usage-cluster');

    const callsBeforeShown = invoke.mock.calls.filter(
      ([command]) => command === 'get_live_usage',
    ).length;

    // `popover:shown` carries no payload — it is a pure signal, unlike the
    // scan events, which carry a status.
    emit('popover:shown', undefined);

    await waitFor(() =>
      expect(
        invoke.mock.calls.filter(([command]) => command === 'get_live_usage').length,
      ).toBeGreaterThan(callsBeforeShown),
    );
    // Not riding the scan pipeline: no scan command was ever asked for.
    expect(invoke).not.toHaveBeenCalledWith('scan_now', expect.anything());
  });

  it('never renders the first-run flow, whatever the flag says', async () => {
    // The flow has its own window now (D-25, `views/OnboardingView.tsx`), and
    // the shell sends the tray click there instead of here. A popover that
    // could still draw it would be a second, unreachable copy.
    mockCommands({ get_settings: { ...SETTINGS, onboardingCompleted: false } });
    render(<PopoverView />);

    await screen.findByText('Wire the tray popover');
    expect(
      screen.queryByRole('heading', { name: 'Everything stays on this machine' }),
    ).not.toBeInTheDocument();
  });
});

/**
 * Attention banners.
 *
 * Every case here starts from a signal the shell actually reports — a
 * repository the system refuses to open, a database that rejected a write.
 * There is no test for a speculative banner because there is no speculative
 * banner.
 */
describe('PopoverView — attention banners', () => {
  beforeEach(() => {
    invoke.mockReset();
    confirmDialog.mockReset();
    saveDialog.mockReset();
    openDialog.mockReset();
    listeners.clear();
    mockCommands();
  });

  it('says nothing when nothing is wrong', async () => {
    mockCommands({ list_repositories: [repositoryPayload()] });
    render(<PopoverView />);

    await screen.findByText('Wire the tray popover');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('surfaces a blocked repository, and Review opens Settings at Sources', async () => {
    mockCommands({
      list_repositories: [repositoryPayload({ status: 'permission_denied' })],
    });
    render(<PopoverView />);

    const banner = await screen.findByRole('status');
    expect(banner).toHaveTextContent(/blocking antiburn from reading widgets/i);

    fireEvent.click(screen.getByRole('button', { name: 'Review' }));

    // Not just "open Settings": the banner lands the reader on the pane that
    // can do something about what it reported.
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('open_settings_window', { pane: 'sources' }),
    );
  });

  it('reads the repository list on first paint rather than waiting for a scan', async () => {
    // A blocked repository is precisely the case where a scan may never
    // complete, so the banner cannot depend on one finishing.
    mockCommands({
      list_repositories: [repositoryPayload({ status: 'permission_denied' })],
    });
    render(<PopoverView />);

    await screen.findByRole('status');
    expect(invoke).toHaveBeenCalledWith('list_repositories');
  });

  it('stays dismissed once the reader waves it away', async () => {
    mockCommands({
      list_repositories: [repositoryPayload({ status: 'permission_denied' })],
    });
    render(<PopoverView />);

    await screen.findByRole('status');
    fireEvent.click(screen.getByRole('button', { name: /^Dismiss the/ }));

    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());

    // A scan finishing re-reads the repository list; the banner must not come
    // back from the dead because of it.
    emit('scan:finished', SCAN_STATUS);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('list_repositories'));
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('surfaces a storage failure with a retry that runs a scan', async () => {
    mockCommands({
      get_storage_health: {
        failing: true,
        message: 'The session index could not be written: disk full',
      },
    });
    render(<PopoverView />);

    const banner = await screen.findByRole('status');
    expect(banner).toHaveTextContent(/disk full/);
    expect(banner).toHaveTextContent(/Nothing already indexed is lost/);

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('scan_now'));
  });

  it('takes the storage banner away when the shell reports a recovery', async () => {
    mockCommands({
      get_storage_health: {
        failing: true,
        message: 'The session index could not be written: disk full',
      },
    });
    render(<PopoverView />);

    await screen.findByRole('status');

    emit('storage:health', { failing: false, message: null });

    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });

  it('shows a recovered-then-failed store again, even after a dismissal', async () => {
    mockCommands({
      get_storage_health: {
        failing: true,
        message: 'The session index could not be written: disk full',
      },
    });
    render(<PopoverView />);

    await screen.findByRole('status');
    fireEvent.click(screen.getByRole('button', { name: /^Dismiss the/ }));
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());

    // Recovery clears the dismissal, so the *next* failure is not silent.
    emit('storage:health', { failing: false, message: null });
    emit('storage:health', {
      failing: true,
      message: 'The scan bookkeeping could not be written: database is locked',
    });

    expect(await screen.findByRole('status')).toHaveTextContent(/database is locked/);
  });
});

describe('PopoverView — window behaviour', () => {
  beforeEach(() => {
    invoke.mockReset();
    confirmDialog.mockReset();
    saveDialog.mockReset();
    openDialog.mockReset();
    listeners.clear();
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

    fireEvent.click(
      await screen.findByRole('button', { name: 'Anthropic, $1.25 today, estimated' }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'All provider usage' }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('set_popover_height', {
        height: 780,
        animate: true,
      }),
    );

    // Nothing exceeds the ceiling, which is now above the contract's 700 for
    // the Usage surface alone (D-22). The shell clamps to the same number.
    const heights = invoke.mock.calls
      .filter(([command]) => command === 'set_popover_height')
      .map(([, args]) => (args as { height: number }).height);
    expect(heights.length).toBeGreaterThan(0);
    expect(Math.max(...heights)).toBeLessThanOrEqual(780);
  });

  it('dismisses the popover on Escape', async () => {
    render(<PopoverView />);
    await screen.findByText('Wire the tray popover');

    fireEvent.keyDown(document, { key: 'Escape' });

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('hide_popover'));
  });

  it('opens Settings on the platform preferences shortcut', async () => {
    render(<PopoverView />);
    await screen.findByText('Wire the tray popover');

    fireEvent.keyDown(document, { key: ',', metaKey: true });

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('open_settings_window', { pane: null }),
    );
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

    fireEvent.click(
      await screen.findByRole('button', { name: 'Anthropic, $1.25 today, estimated' }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'All provider usage' }));

    const usage = await screen.findByRole('heading', { name: 'Usage' });
    await waitFor(() => expect(usage).toHaveFocus());
  });
});
