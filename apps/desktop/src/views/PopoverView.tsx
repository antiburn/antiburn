// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { open } from '@tauri-apps/plugin-dialog';
import { Settings2 } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';

import { AlertTriangle } from 'lucide-react';

import { LocalActivityList } from '../components/activity/LocalActivityList';
import type { LocalActivityEntry } from '../components/activity/LocalActivityList';
import { ProviderUsageCluster } from '../components/providerUsage';
import { Banner } from '../components/ui/Banner';
import { Skeleton } from '../components/ui/Skeleton';
import { renderAgentIcon } from '../lib/agentIcon';
import { indexOfSession, toActivityEntries } from '../lib/activityEntries';
import { applyTheme } from '../lib/appearance';
import { attentionBanners, type AttentionKind } from '../lib/attention';
import {
  addScanRoot,
  cancelScan,
  defaultScanRoots,
  DEFAULT_SETTINGS,
  EMPTY_PROVIDER_USAGE,
  getProviderUsage,
  getScanStatus,
  getSettings,
  getStorageHealth,
  HEALTHY_STORAGE,
  hidePopover,
  listRecentSessions,
  listRepositories,
  listScanRoots,
  onScanEvent,
  onStorageHealth,
  openSettingsWindow,
  removeScanRoot,
  scanNow,
  setPopoverHeight,
  setRepositoryEnabled,
  setSettings,
  type AppSettings,
  type ProviderUsageSummaryPayload,
  type ScanStatus,
  type StorageHealthPayload,
} from '../lib/ipc';
import { popoverHeightFor, prefersReducedMotion, type PopoverSurface } from '../lib/popoverHeight';
import type { LocalRepositoryItem, LocalRepositoryStatus } from '../lib/types/repository';
import { OnboardingFlow } from './popover/OnboardingFlow';
import { ScanStatusBar } from './popover/ScanStatusBar';
import { SessionPane, type SessionSubject } from './popover/SessionPane';
import { UsageView } from './popover/UsageView';

/**
 * The tray popover.
 *
 * Four surfaces share one 380px window: the first-run flow, the activity list,
 * one session's analytics, and local provider usage. There is no router — a
 * popover is a single place, and a stack of "where I came from" is all the
 * navigation it needs.
 *
 * Three things are owned here rather than by any one surface, because they are
 * properties of *the window*:
 *
 * - **Height.** Each surface declares one (`lib/popoverHeight`) and the shell
 *   animates between them, bounded at 700px.
 * - **Escape.** The keyboard's way out of a tray popover. It dismisses the
 *   window unless a surface has already handled the key for something nearer —
 *   an open provider panel, say — which those surfaces signal by calling
 *   `preventDefault`.
 * - **Focus.** Swapping surfaces by conditional render leaves focus on `<body>`
 *   and tells a screen-reader user nothing. Every surface marks its heading,
 *   and that heading is focused when the surface changes.
 *
 * A fourth thing is owned here for a different reason: the **attention
 * banners** above the activity list. What they may say is decided by
 * `lib/attention`, from signals the shell reports; dismissal is held here,
 * because "I have seen this" is a fact about this run of the popover and not
 * something worth persisting.
 */

/** Placeholder rows while the first list load is in flight. */
function ActivitySkeleton() {
  return (
    <div aria-hidden data-testid="activity-skeleton" className="space-y-1 px-2 pt-10">
      {[0, 1, 2, 3].map((row) => (
        <div key={row} className="flex items-start gap-3 px-2 py-2">
          <Skeleton className="mt-0.5 h-[18px] w-[18px] shrink-0" />
          <div className="min-w-0 flex-1 space-y-1.5">
            <Skeleton className="h-3.5 w-44" />
            <Skeleton className="h-3 w-28" />
          </div>
        </div>
      ))}
    </div>
  );
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

export function PopoverView() {
  const [settings, setSettingsState] = useState<AppSettings | null>(null);
  const [entries, setEntries] = useState<LocalActivityEntry[] | null>(null);
  const [scanRoots, setScanRoots] = useState<string[]>([]);
  const [defaultRoots, setDefaultRoots] = useState<string[]>([]);
  const [repositories, setRepositories] = useState<LocalRepositoryItem[]>([]);
  const [scanStatus, setScanStatus] = useState<ScanStatus | null>(null);
  /** Navigation stack. Empty means the activity list is showing. */
  const [stack, setStack] = useState<SessionSubject[]>([]);
  /**
   * Provider usage, or null while the first snapshot is in flight. The footer
   * is withheld rather than shown empty until then: an empty usage footer and
   * a not-yet-loaded one look identical, and one of them is a lie.
   */
  const [usage, setUsage] = useState<ProviderUsageSummaryPayload | null>(null);
  /** Whether the full Usage view is showing over the activity list. */
  const [showUsage, setShowUsage] = useState(false);
  /** Whether the local database is still accepting writes. */
  const [storage, setStorage] = useState<StorageHealthPayload>(HEALTHY_STORAGE);
  /** Banners the reader has waved away this run. */
  const [dismissed, setDismissed] = useState<readonly AttentionKind[]>([]);

  const current = stack.at(-1) ?? null;
  const windowDays = settings?.activityWindowDays ?? DEFAULT_SETTINGS.activityWindowDays;
  const onboarding = settings != null && !settings.onboardingCompleted;

  const refreshEntries = useCallback(async (days: number) => {
    const payloads = await listRecentSessions(days);
    setEntries(toActivityEntries(payloads));
  }, []);

  const refreshUsage = useCallback(async () => {
    setUsage(await getProviderUsage().catch(() => EMPTY_PROVIDER_USAGE));
  }, []);

  const refreshRepositoryList = useCallback(async () => {
    const payloads = await listRepositories().catch(() => []);
    setRepositories(
      payloads.map((payload) => ({ ...payload, status: repositoryStatus(payload.status) })),
    );
  }, []);

  // First load: preferences decide the theme and the visible window, so
  // everything else waits on them.
  useEffect(() => {
    let active = true;
    void (async () => {
      const stored = await getSettings().catch(() => DEFAULT_SETTINGS);
      if (!active) return;
      applyTheme(stored.theme);
      setSettingsState(stored);
      const [roots, defaults, status, health] = await Promise.all([
        listScanRoots().catch(() => []),
        defaultScanRoots().catch(() => []),
        getScanStatus().catch(() => null),
        getStorageHealth().catch(() => HEALTHY_STORAGE),
      ]);
      if (!active) return;
      setScanRoots(roots);
      setDefaultRoots(defaults);
      setScanStatus(status);
      setStorage(health);
      // The repository list is read on first paint rather than waiting for a
      // scan to finish: onboarding's Repositories step needs it, and so does
      // the source-access banner — a blocked repository is exactly the case
      // where no scan will ever complete to deliver the news. Not awaited: it
      // is a store read that nothing below depends on, and the activity list
      // is what a reader opened the popover for.
      void refreshRepositoryList();
      if (stored.onboardingCompleted) {
        await Promise.all([
          refreshEntries(stored.activityWindowDays).catch(() => setEntries([])),
          refreshUsage(),
        ]);
      }
    })();
    return () => {
      active = false;
    };
  }, [refreshEntries, refreshUsage, refreshRepositoryList]);

  // Storage health changes rarely and matters immediately, so it is pushed
  // rather than polled. Only changes are emitted, so this is not a per-tick
  // event.
  useEffect(() => {
    let active = true;
    const pending = onStorageHealth((status) => {
      if (!active) return;
      setStorage(status);
      // A failure that recovers should not leave its banner dismissed, or the
      // next failure would arrive silently.
      if (!status.failing) setDismissed((previous) => previous.filter((id) => id !== 'storage'));
    });
    return () => {
      active = false;
      void pending.then((unlisten) => unlisten());
    };
  }, []);

  // The scan is the only thing that changes what is on screen behind the
  // reader's back, so that is what the list listens for rather than polling.
  // Every phase is kept, not just `finished`: the status line's whole job is to
  // show the pass while it runs.
  useEffect(() => {
    let active = true;
    const pending = onScanEvent((status, phase) => {
      if (!active) return;
      setScanStatus(status);
      if (phase !== 'finished') return;
      void refreshEntries(windowDays).catch(() => {});
      void refreshUsage();
      void refreshRepositoryList();
    });
    return () => {
      active = false;
      void pending.then((unlisten) => unlisten());
    };
  }, [windowDays, refreshEntries, refreshUsage, refreshRepositoryList]);

  /* ---------------------------------------------------------------------
   * Window behaviour: which surface is showing, how tall it is, and Escape
   * ------------------------------------------------------------------ */

  const surface: PopoverSurface = onboarding
    ? 'onboarding'
    : showUsage && usage
      ? 'usage'
      : current
        ? 'session'
        : 'activity';

  useEffect(() => {
    // Reduced motion is a webview preference, so the decision is made here and
    // the shell simply honours it.
    void setPopoverHeight(popoverHeightFor(surface), !prefersReducedMotion()).catch(() => {});
  }, [surface]);

  const surfaceRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    // Conditional render swaps the whole surface; without this, focus is left
    // on <body> and a keyboard or screen-reader user has to walk back in from
    // the top of the document every time.
    surfaceRef.current?.querySelector<HTMLElement>('[data-view-heading]')?.focus();
  }, [surface]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      // A surface with something nearer to close — an open provider panel —
      // claims the key by calling `preventDefault`. Anything left over
      // dismisses the popover, which is the keyboard's only way out of a tray
      // window.
      if (event.defaultPrevented) return;
      void hidePopover().catch(() => {});
    };
    // Bound to `window`, deliberately: it is the last object in the event's
    // propagation path, so every surface listening on `document` has already
    // had its chance to claim the key. Dismissing the whole window is the
    // coarsest possible response to Escape and must therefore be the last.
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  /* ---------------------------------------------------------------------
   * Actions
   * ------------------------------------------------------------------ */

  const handleAddScanRoot = useCallback(async () => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked !== 'string') return;
    setScanRoots(await addScanRoot(picked));
  }, []);

  const handleRemoveScanRoot = useCallback(async (path: string) => {
    setScanRoots(await removeScanRoot(path));
  }, []);

  const handleRescan = useCallback(async () => {
    const status = await scanNow().catch(() => null);
    if (status) setScanStatus(status);
  }, []);

  /* ---------------------------------------------------------------------
   * Attention banners
   * ------------------------------------------------------------------ */

  const banners = attentionBanners({ repositories, storage }).filter(
    (banner) => !dismissed.includes(banner.id),
  );

  const dismissBanner = useCallback((id: AttentionKind) => {
    setDismissed((previous) => (previous.includes(id) ? previous : [...previous, id]));
  }, []);

  const handleCancelScan = useCallback(async () => {
    const status = await cancelScan().catch(() => null);
    if (status) setScanStatus(status);
  }, []);

  const applySettings = useCallback(
    async (change: Partial<AppSettings>) => {
      const base = settings ?? DEFAULT_SETTINGS;
      const saved = await setSettings({ ...base, ...change }).catch(() => ({
        ...base,
        ...change,
      }));
      setSettingsState(saved);
      return saved;
    },
    [settings],
  );

  const handleToggleRepository = useCallback(
    async (item: LocalRepositoryItem, enabled: boolean) => {
      const payloads = await setRepositoryEnabled(item.key, enabled).catch(() => []);
      if (payloads.length === 0) return;
      setRepositories(
        payloads.map((payload) => ({ ...payload, status: repositoryStatus(payload.status) })),
      );
    },
    [],
  );

  const handleFinishOnboarding = useCallback(async () => {
    const saved = await applySettings({ onboardingCompleted: true });
    setEntries(null);
    await Promise.all([
      refreshEntries(saved.activityWindowDays).catch(() => setEntries([])),
      refreshUsage(),
    ]);
  }, [applySettings, refreshEntries, refreshUsage]);

  const openSession = useCallback((subject: SessionSubject) => {
    setStack((previous) => [...previous, subject]);
  }, []);

  const goBack = useCallback(() => {
    setStack((previous) => previous.slice(0, -1));
  }, []);

  /** Replace the top of the stack, for the newer/older traversal. */
  const replaceTop = useCallback((subject: SessionSubject) => {
    setStack((previous) => [...previous.slice(0, -1), subject]);
  }, []);

  const subjectFor = useCallback((entry: LocalActivityEntry): SessionSubject => {
    return {
      agent: entry.agent,
      sessionId: entry.sessionId ?? '',
      wslDistro: entry.wslDistro ?? null,
      ...(entry.title ? { title: entry.title } : {}),
      isActive: entry.isActive,
    };
  }, []);

  /* ---------------------------------------------------------------------
   * Surfaces
   * ------------------------------------------------------------------ */

  function body() {
    // Onboarding gates everything: without it there is nothing scanned to show.
    if (onboarding) {
      return (
        <OnboardingFlow
          defaultRoots={defaultRoots}
          scanRoots={scanRoots}
          onAddScanRoot={() => void handleAddScanRoot()}
          onRemoveScanRoot={(path) => void handleRemoveScanRoot(path)}
          repositories={repositories}
          onToggleRepository={(item, enabled) => void handleToggleRepository(item, enabled)}
          onDiscover={() => void handleRescan()}
          onCancelScan={() => void handleCancelScan()}
          scanStatus={scanStatus}
          windowDays={windowDays}
          onWindowDaysChange={(days) => void applySettings({ activityWindowDays: days })}
          onFinish={() => void handleFinishOnboarding()}
        />
      );
    }

    // Usage sits over the list rather than in the session stack: it is a second
    // way of reading the same activity, not a place a session leads to.
    if (showUsage && usage) {
      return <UsageView summary={usage} onBack={() => setShowUsage(false)} />;
    }

    if (current) {
      // Traversal only applies to a session that is actually in the list; a
      // sub-agent or a fork opened from elsewhere has no neighbours.
      const position = current.subagent
        ? -1
        : indexOfSession(entries ?? [], current.agent, current.sessionId, current.wslDistro);
      const neighbour = (offset: number) => {
        const entry = position >= 0 ? entries?.[position + offset] : undefined;
        if (!entry?.sessionId) return undefined;
        return () => replaceTop(subjectFor(entry));
      };

      return (
        <SessionPane
          subject={current}
          onBack={goBack}
          onPrev={neighbour(-1)}
          onNext={neighbour(1)}
          onOpenSession={openSession}
          onDeleted={() => {
            goBack();
            void refreshEntries(windowDays).catch(() => {});
          }}
        />
      );
    }

    return (
      <div className="flex h-full flex-col">
        <header className="flex h-11 shrink-0 items-center gap-2 px-4">
          <h1
            data-view-heading
            tabIndex={-1}
            className="type-headline text-label outline-none"
          >
            antiburn
          </h1>
          <button
            type="button"
            onClick={() => void openSettingsWindow()}
            aria-label="Open settings"
            className="-mr-1.5 ml-auto inline-flex h-6 shrink-0 items-center rounded-control px-1.5 text-label-secondary hover:bg-surface-hover"
          >
            <Settings2 size={14} strokeWidth={2} aria-hidden="true" />
          </button>
        </header>

        <ScanStatusBar
          status={scanStatus}
          paused={settings?.discoveryPaused ?? false}
          onRescan={() => void handleRescan()}
          onCancel={() => void handleCancelScan()}
          onTogglePaused={(paused) => void applySettings({ discoveryPaused: paused })}
        />

        {banners.length > 0 && (
          <div className="shrink-0 space-y-1 px-2 pb-1.5">
            {banners.map((banner) => (
              <Banner
                key={banner.id}
                icon={AlertTriangle}
                message={banner.message}
                actionLabel={banner.actionLabel}
                onAction={() => {
                  if (banner.action.kind === 'rescan') {
                    void handleRescan();
                    return;
                  }
                  void openSettingsWindow(banner.action.pane);
                }}
                onDismiss={() => dismissBanner(banner.id)}
                dismissLabel={banner.dismissLabel}
              />
            ))}
          </div>
        )}

        <div className="min-h-0 flex-1">
          {entries == null ? (
            <ActivitySkeleton />
          ) : (
            <LocalActivityList
              entries={entries}
              days={windowDays}
              onOpenSettings={() => void openSettingsWindow()}
              onOpenSession={(entry) => {
                if (!entry.sessionId) return;
                openSession(subjectFor(entry));
              }}
              renderAgentIcon={renderAgentIcon}
            />
          )}
        </div>

        {usage && (
          <ProviderUsageCluster
            providers={usage.providers}
            onViewAll={() => setShowUsage(true)}
          />
        )}
      </div>
    );
  }

  return (
    <div ref={surfaceRef} className="h-full">
      {body()}
    </div>
  );
}
