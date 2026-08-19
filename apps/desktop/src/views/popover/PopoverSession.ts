// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { LocalActivityEntry } from '../../components/activity/LocalActivityList';
import { toActivityEntries } from '../../lib/activityEntries';
import { applyTheme } from '../../lib/appearance';
import type { AttentionKind } from '../../lib/attention';
import {
  DEFAULT_SETTINGS,
  EMPTY_LIVE_USAGE,
  EMPTY_PROVIDER_USAGE,
  getLiveUsage,
  getProviderUsage,
  getSessionAnalytics,
  getSettings,
  getStorageHealth,
  getSubagentAnalytics,
  HEALTHY_STORAGE,
  hidePopover,
  listRecentSessions,
  listRepositories,
  onScanEvent,
  onSessionsInvalidated,
  onSettingsChanged,
  onStorageHealth,
  openSettingsWindow,
  scanNow,
  setPopoverHeight,
  type AppSettings,
  type LiveUsageSummaryPayload,
  type ProviderUsageSummaryPayload,
  type SessionAnalyticsPayload,
  type StorageHealthPayload,
} from '../../lib/ipc';
import {
  popoverHeightFor,
  prefersReducedMotion,
  type PopoverSurface,
} from '../../lib/popoverHeight';
import { localSessionKey } from '../../lib/presentation/localIdentity';
import type { LocalRepositoryItem, LocalRepositoryStatus } from '../../lib/types/repository';
import type { SessionSubject } from './SessionPane';

/**
 * The imperative boundary between the popover window and the shell.
 *
 * React reads immutable snapshots through `useSyncExternalStore`; IPC calls,
 * event subscriptions, and the window's own keyboard handling stay here,
 * where they belong to the external systems that created them rather than to
 * a component lifecycle. See `views/onboarding/OnboardingSession.ts` for the
 * same shape applied to the first-run window.
 */

/** One session's loaded analytics, tagged with the subject it belongs to. */
type PopoverAnalyticsState = {
  key: string;
  payload: SessionAnalyticsPayload | null;
  error: boolean;
} | null;

export interface PopoverSnapshot {
  settings: AppSettings | null;
  entries: LocalActivityEntry[] | null;
  repositories: LocalRepositoryItem[];
  /** Provider usage, or null while the first snapshot is in flight. */
  usage: ProviderUsageSummaryPayload | null;
  liveUsage: LiveUsageSummaryPayload;
  /** Whether the full Usage view is showing over the activity list. */
  showUsage: boolean;
  storage: StorageHealthPayload;
  /** Banners the reader has waved away this run. */
  dismissed: readonly AttentionKind[];
  /** Navigation stack. Empty means the activity list is showing. */
  stack: SessionSubject[];
  /**
   * The most recently settled (or failed) analytics load, tagged with its
   * subject's key.
   *
   * One field rather than three, and it carries its own key: "still loading"
   * is then *derived* by the reader (the key does not match the subject on
   * top of the stack) instead of being a flag this class has to flip on the
   * way in — which is what keeps opening a session from cascading renders.
   */
  analytics: PopoverAnalyticsState;
}

/**
 * Identity key for a subject's analytics load. Stable across re-navigation.
 *
 * Scoped by environment as well as agent and id: the same session id can
 * exist natively and inside a WSL distribution, and without the environment
 * in the key a subject moving between the two would keep showing the other
 * environment's stale (or loading) analytics.
 *
 * A sub-agent id is only unique within its launching session, so its key
 * carries the parent's local identity too.
 */
export function sessionKey(subject: SessionSubject): string {
  return subject.subagent
    ? JSON.stringify([
        'subagent',
        localSessionKey(subject.agent, subject.subagent.parentSessionId, subject.wslDistro),
        subject.subagent.subagentId,
      ])
    : localSessionKey(subject.agent, subject.sessionId, subject.wslDistro);
}

/** Load one subject's analytics. Sub-agents come from their own command. */
async function loadAnalytics(subject: SessionSubject): Promise<SessionAnalyticsPayload | null> {
  if (subject.subagent) {
    return getSubagentAnalytics(
      subject.agent,
      subject.subagent.parentSessionId,
      subject.subagent.subagentId,
      subject.wslDistro,
    );
  }
  return getSessionAnalytics(subject.agent, subject.sessionId, subject.wslDistro);
}

/** Narrow the shell's status string to the repository list's union. */
function repositoryStatus(status: string): LocalRepositoryStatus {
  switch (status) {
    case 'accessible':
    case 'permission_denied':
    case 'not_cloned':
    case 'disabled':
      return status;
    default:
      return 'accessible';
  }
}

export class PopoverSession {
  private listeners = new Set<() => void>();
  private started = false;
  private generation = 0;
  private analyticsToken = 0;

  private stopSettingsListening: (() => void) | null = null;
  private stopSessionsInvalidatedListening: (() => void) | null = null;
  private stopStorageHealthListening: (() => void) | null = null;
  private stopScanListening: (() => void) | null = null;

  private snapshot: PopoverSnapshot = {
    settings: null,
    entries: null,
    repositories: [],
    usage: null,
    liveUsage: EMPTY_LIVE_USAGE,
    showUsage: false,
    storage: HEALTHY_STORAGE,
    dismissed: [],
    stack: [],
    analytics: null,
  };

  getSnapshot = (): PopoverSnapshot => this.snapshot;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    if (!this.started) this.start();
    return () => {
      this.listeners.delete(listener);
      if (this.listeners.size === 0) this.stop();
    };
  };

  /* -----------------------------------------------------------------------
   * Navigation
   * -------------------------------------------------------------------- */

  openSession = (subject: SessionSubject): void => {
    this.update({ stack: [...this.snapshot.stack, subject] });
    this.syncHeight();
    this.loadAnalyticsFor(subject);
  };

  goBack = (): void => {
    this.update({ stack: this.snapshot.stack.slice(0, -1) });
    this.syncHeight();
    const top = this.snapshot.stack.at(-1);
    if (top) this.loadAnalyticsFor(top);
  };

  /** Replace the top of the stack, for the newer/older traversal. */
  replaceTop = (subject: SessionSubject): void => {
    this.update({ stack: [...this.snapshot.stack.slice(0, -1), subject] });
    this.syncHeight();
    this.loadAnalyticsFor(subject);
  };

  /** The shown session's local records were deleted: go back and re-list. */
  sessionDeleted = (): void => {
    this.goBack();
    void this.refreshEntries(this.windowDays()).catch(() => {});
  };

  setShowUsage = (show: boolean): void => {
    this.update({ showUsage: show });
    this.syncHeight();
  };

  dismissBanner = (id: AttentionKind): void => {
    if (this.snapshot.dismissed.includes(id)) return;
    this.update({ dismissed: [...this.snapshot.dismissed, id] });
  };

  /** Run a discovery pass. The source-access banner's only action. */
  rescan = async (): Promise<void> => {
    await scanNow().catch(() => null);
  };

  /* -----------------------------------------------------------------------
   * Lifecycle
   * -------------------------------------------------------------------- */

  private start(): void {
    this.started = true;
    const generation = ++this.generation;

    void this.loadInitial(generation);
    void this.listenSettings(generation);
    void this.listenSessionsInvalidated(generation);
    void this.listenStorageHealth(generation);
    void this.listenScanEvent(generation);

    // ⌘, opens Settings — the platform's standard preferences shortcut, which
    // an accessory app with no application menu has to own itself. Bound
    // alongside Escape on `window`, deliberately: it is the last object in an
    // event's propagation path, so every surface listening on `document` has
    // already had its chance to claim the key first.
    window.addEventListener('keydown', this.onWindowKeyDown);
  }

  private stop(): void {
    this.started = false;
    this.generation += 1;
    this.stopSettingsListening?.();
    this.stopSettingsListening = null;
    this.stopSessionsInvalidatedListening?.();
    this.stopSessionsInvalidatedListening = null;
    this.stopStorageHealthListening?.();
    this.stopStorageHealthListening = null;
    this.stopScanListening?.();
    this.stopScanListening = null;
    window.removeEventListener('keydown', this.onWindowKeyDown);
  }

  // First load: preferences decide the theme and the visible window, so
  // everything else waits on them.
  private loadInitial = async (generation: number): Promise<void> => {
    const stored = await getSettings().catch(() => DEFAULT_SETTINGS);
    if (generation !== this.generation) return;
    applyTheme(stored.theme);
    this.update({ settings: stored });
    const health = await getStorageHealth().catch(() => HEALTHY_STORAGE);
    if (generation !== this.generation) return;
    this.update({ storage: health });
    // The repository list is read on first paint rather than waiting for a
    // scan to finish, because the source-access banner needs it — a blocked
    // repository is exactly the case where no scan will ever complete to
    // deliver the news. Not awaited: it is a store read that nothing below
    // depends on, and the activity list is what a reader opened the popover
    // for.
    void this.refreshRepositoryList();
    await Promise.all([
      this.refreshEntries(stored.activityWindowDays).catch(() => this.update({ entries: [] })),
      this.refreshUsage(),
    ]);
  };

  // Settings are written in the settings window but rendered here: the theme,
  // the day window, and the pause state all change what this window shows.
  // The shell broadcasts every write, and the popover restyles and re-queries
  // as needed instead of waiting for its next mount (which never comes — the
  // window lives for the whole run).
  private listenSettings = async (generation: number): Promise<void> => {
    const unlisten = await onSettingsChanged((settings) => {
      if (generation !== this.generation) return;
      const previousDays = this.windowDays();
      applyTheme(settings.theme);
      this.update({ settings });
      if (settings.activityWindowDays !== previousDays) {
        void this.refreshEntries(settings.activityWindowDays).catch(() => {});
      }
    });
    if (generation !== this.generation) {
      unlisten();
      return;
    }
    this.stopSettingsListening = unlisten;
  };

  // Sessions can leave the index without a scan — a repository opt-out purges
  // its rows on the spot — and the list must not keep showing them.
  private listenSessionsInvalidated = async (generation: number): Promise<void> => {
    const unlisten = await onSessionsInvalidated(() => {
      if (generation !== this.generation) return;
      void this.refreshEntries(this.windowDays()).catch(() => {});
      void this.refreshUsage();
      void this.refreshRepositoryList();
    });
    if (generation !== this.generation) {
      unlisten();
      return;
    }
    this.stopSessionsInvalidatedListening = unlisten;
  };

  // Storage health changes rarely and matters immediately, so it is pushed
  // rather than polled. Only changes are emitted, so this is not a per-tick
  // event.
  private listenStorageHealth = async (generation: number): Promise<void> => {
    const unlisten = await onStorageHealth((status) => {
      if (generation !== this.generation) return;
      this.update({ storage: status });
      // A failure that recovers should not leave its banner dismissed, or the
      // next failure would arrive silently.
      if (!status.failing) {
        this.update({ dismissed: this.snapshot.dismissed.filter((id) => id !== 'storage') });
      }
    });
    if (generation !== this.generation) {
      unlisten();
      return;
    }
    this.stopStorageHealthListening = unlisten;
  };

  // The scan is the only thing that changes what is on screen behind the
  // reader's back, so that is what the list listens for rather than polling.
  // Only `finished` matters here: no surface in this window draws a pass in
  // progress, so the intermediate phases have nothing to say.
  private listenScanEvent = async (generation: number): Promise<void> => {
    const unlisten = await onScanEvent((_status, phase) => {
      if (generation !== this.generation) return;
      if (phase !== 'finished') return;
      void this.refreshEntries(this.windowDays()).catch(() => {});
      void this.refreshUsage();
      void this.refreshRepositoryList();
    });
    if (generation !== this.generation) {
      unlisten();
      return;
    }
    this.stopScanListening = unlisten;
  };

  private onWindowKeyDown = (event: KeyboardEvent): void => {
    if (event.key === 'Escape') {
      // A surface with something nearer to close — an open provider panel —
      // claims the key by calling `preventDefault`. Anything left over
      // dismisses the popover, which is the keyboard's only way out of a tray
      // window.
      if (event.defaultPrevented) return;
      void hidePopover().catch(() => {});
      return;
    }
    if ((event.metaKey || event.ctrlKey) && event.key === ',') {
      event.preventDefault();
      void openSettingsWindow();
    }
  };

  /* -----------------------------------------------------------------------
   * Refreshers
   * -------------------------------------------------------------------- */

  private refreshEntries = async (days: number): Promise<void> => {
    const payloads = await listRecentSessions(days);
    this.update({ entries: toActivityEntries(payloads) });
  };

  private refreshUsage = async (): Promise<void> => {
    // Two commands, refreshed together and settled independently: the limit
    // half is allowed to be missing without the spend half waiting on it, and
    // a source that throws leaves the estimate surface untouched.
    const [spend, limits] = await Promise.all([
      getProviderUsage().catch(() => EMPTY_PROVIDER_USAGE),
      getLiveUsage().catch(() => EMPTY_LIVE_USAGE),
    ]);
    this.update({ usage: spend, liveUsage: limits });
    // Usage arriving can flip the derived surface to 'usage' on its own — the
    // reader may have already asked to see it before there was anything to
    // show — so the height request has to follow this update too, not only
    // the click-driven ones.
    this.syncHeight();
  };

  private refreshRepositoryList = async (): Promise<void> => {
    const payloads = await listRepositories().catch(() => []);
    this.update({
      repositories: payloads.map((payload) => ({
        ...payload,
        status: repositoryStatus(payload.status),
      })),
    });
  };

  private loadAnalyticsFor = (subject: SessionSubject): void => {
    const key = sessionKey(subject);
    const generation = this.generation;
    const token = ++this.analyticsToken;
    loadAnalytics(subject)
      .then((payload) => {
        if (generation !== this.generation || token !== this.analyticsToken) return;
        this.update({ analytics: { key, payload, error: false } });
      })
      .catch(() => {
        if (generation !== this.generation || token !== this.analyticsToken) return;
        this.update({ analytics: { key, payload: null, error: true } });
      });
  };

  /* -----------------------------------------------------------------------
   * Derived
   * -------------------------------------------------------------------- */

  private windowDays(): number {
    return this.snapshot.settings?.activityWindowDays ?? DEFAULT_SETTINGS.activityWindowDays;
  }

  private surface(): PopoverSurface {
    const { showUsage, usage, stack } = this.snapshot;
    return showUsage && usage ? 'usage' : stack.length > 0 ? 'session' : 'activity';
  }

  // Reduced motion is a webview preference, so the decision is made here and
  // the shell simply honours it.
  private syncHeight(): void {
    void setPopoverHeight(popoverHeightFor(this.surface()), !prefersReducedMotion()).catch(
      () => {},
    );
  }

  private update(change: Partial<PopoverSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change };
    for (const listener of this.listeners) listener();
  }
}
