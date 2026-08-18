// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
  DEFAULT_SETTINGS,
  EMPTY_LIVE_USAGE,
  EMPTY_PROVIDER_USAGE,
  getLiveUsage,
  getProviderUsage,
  getSettings,
  getStorageHealth,
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
  type StorageHealthPayload,
} from '../lib/ipc';
import { isFloatingHudEnabled, openOverlayWindow } from '../lib/overlayWindow';
import { isMacOS } from '../lib/platform';
import {
  popoverHeightFor,
  prefersReducedMotion,
  type PopoverSurface,
} from '../lib/popoverHeight';
import type { LocalRepositoryItem, LocalRepositoryStatus } from '../lib/types/repository';
import { SessionPane, type SessionSubject } from './popover/SessionPane';
import { UsageView } from './popover/UsageView';

/**
 * The tray popover.
 *
 * Three surfaces share one 380px window: the activity list, one session's
 * analytics, and local provider usage. There is no router — a popover is a
 * single place, and a stack of "where I came from" is all the navigation it
 * needs.
 *
 * There used to be a fourth. The first-run flow now has its own window
 * (`views/OnboardingView.tsx`, `src-tauri/src/onboarding.rs`, D-25), and with
 * it went the scan roots, folder permissions, and repository toggling this
 * component carried for one surface out of four. What is left of that here is
 * only what the attention banners genuinely read.
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
  const [repositories, setRepositories] = useState<LocalRepositoryItem[]>([]);
  /** Navigation stack. Empty means the activity list is showing. */
  const [stack, setStack] = useState<SessionSubject[]>([]);
  /**
   * Provider usage, or null while the first snapshot is in flight. The footer
   * is withheld rather than shown empty until then: an empty usage footer and
   * a not-yet-loaded one look identical, and one of them is a lie.
   */
  const [usage, setUsage] = useState<ProviderUsageSummaryPayload | null>(null);
  /**
   * The provider's own limit figures. Unlike `usage` this starts at the empty
   * summary rather than null: the limit half is genuinely optional, so "no
   * source has anything" is a real state and not a loading one.
   */
  const [liveUsage, setLiveUsage] = useState<LiveUsageSummaryPayload>(EMPTY_LIVE_USAGE);
  /** Whether the full Usage view is showing over the activity list. */
  const [showUsage, setShowUsage] = useState(false);
  /** Whether the local database is still accepting writes. */
  const [storage, setStorage] = useState<StorageHealthPayload>(HEALTHY_STORAGE);
  /** Banners the reader has waved away this run. */
  const [dismissed, setDismissed] = useState<readonly AttentionKind[]>([]);

  const current = stack.at(-1) ?? null;
  const windowDays = settings?.activityWindowDays ?? DEFAULT_SETTINGS.activityWindowDays;

  // The settings-changed subscription compares against the days it last saw
  // without re-subscribing on every change.
  const windowDaysRef = useRef(windowDays);
  useEffect(() => {
    windowDaysRef.current = windowDays;
  }, [windowDays]);

  const refreshEntries = useCallback(async (days: number) => {
    const payloads = await listRecentSessions(days);
    setEntries(toActivityEntries(payloads));
  }, []);

  const refreshUsage = useCallback(async () => {
    // Two commands, refreshed together and settled independently: the limit
    // half is allowed to be missing without the spend half waiting on it, and
    // a source that throws leaves the estimate surface untouched.
    const [spend, limits] = await Promise.all([
      getProviderUsage().catch(() => EMPTY_PROVIDER_USAGE),
      getLiveUsage().catch(() => EMPTY_LIVE_USAGE),
    ]);
    setUsage(spend);
    setLiveUsage(limits);
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
      const health = await getStorageHealth().catch(() => HEALTHY_STORAGE);
      if (!active) return;
      setStorage(health);
      // The repository list is read on first paint rather than waiting for a
      // scan to finish, because the source-access banner needs it — a blocked
      // repository is exactly the case where no scan will ever complete to
      // deliver the news. Not awaited: it is a store read that nothing below
      // depends on, and the activity list is what a reader opened the popover
      // for.
      void refreshRepositoryList();
      await Promise.all([
        refreshEntries(stored.activityWindowDays).catch(() => setEntries([])),
        refreshUsage(),
      ]);
    })();
    return () => {
      active = false;
    };
  }, [refreshEntries, refreshUsage, refreshRepositoryList]);

  // Restore the floating HUD on startup when its preference is on. The
  // popover is the one webview that exists from launch, so it is the window
  // that restores it; macOS-only for now, like the window itself
  // (docs/deviations.md D-28).
  useEffect(() => {
    if (isMacOS() && isFloatingHudEnabled()) void openOverlayWindow().catch(() => {});
  }, []);

  // Settings are written in the settings window but rendered here: the theme,
  // the day window, and the pause state all change what this window shows.
  // The shell broadcasts every write, and the popover restyles and re-queries
  // as needed instead of waiting for its next mount (which never comes — the
  // window lives for the whole run).
  useEffect(() => {
    let active = true;
    const pending = onSettingsChanged((stored) => {
      if (!active) return;
      const previousDays = windowDaysRef.current;
      applyTheme(stored.theme);
      setSettingsState(stored);
      if (stored.activityWindowDays !== previousDays) {
        void refreshEntries(stored.activityWindowDays).catch(() => {});
      }
    });
    return () => {
      active = false;
      void pending.then((unlisten) => unlisten());
    };
  }, [refreshEntries]);

  // Sessions can leave the index without a scan — a repository opt-out purges
  // its rows on the spot — and the list must not keep showing them.
  useEffect(() => {
    let active = true;
    const pending = onSessionsInvalidated(() => {
      if (!active) return;
      void refreshEntries(windowDaysRef.current).catch(() => {});
      void refreshUsage();
      void refreshRepositoryList();
    });
    return () => {
      active = false;
      void pending.then((unlisten) => unlisten());
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
      if (!status.failing)
        setDismissed((previous) => previous.filter((id) => id !== 'storage'));
    });
    return () => {
      active = false;
      void pending.then((unlisten) => unlisten());
    };
  }, []);

  // The scan is the only thing that changes what is on screen behind the
  // reader's back, so that is what the list listens for rather than polling.
  // Only `finished` matters here: no surface in this window draws a pass in
  // progress, so the intermediate phases have nothing to say.
  useEffect(() => {
    let active = true;
    const pending = onScanEvent((_status, phase) => {
      if (!active) return;
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

  const surface: PopoverSurface =
    showUsage && usage ? 'usage' : current ? 'session' : 'activity';

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

  // ⌘, opens Settings — the platform's standard preferences shortcut, which
  // an accessory app with no application menu has to own itself.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key === ',') {
        event.preventDefault();
        void openSettingsWindow();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  /* ---------------------------------------------------------------------
   * Actions
   * ------------------------------------------------------------------ */

  /** Run a discovery pass. The source-access banner's only action. */
  const handleRescan = useCallback(async () => {
    await scanNow().catch(() => null);
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
    // Usage sits over the list rather than in the session stack: it is a second
    // way of reading the same activity, not a place a session leads to.
    if (showUsage && usage) {
      return <UsageView summary={usage} live={liveUsage} onBack={() => setShowUsage(false)} />;
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
          <h1 data-view-heading tabIndex={-1} className="type-headline text-label outline-none">
            antiburn
          </h1>
        </header>

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
              // The affordance sits on the day-range label, so it lands on the
              // pane that owns "Show the last" rather than the last-open pane.
              onOpenSettings={() => void openSettingsWindow('general')}
              onOpenSession={(entry) => {
                if (!entry.sessionId) return;
                openSession(subjectFor(entry));
              }}
              renderAgentIcon={renderAgentIcon}
            />
          )}
        </div>

        <ProviderUsageCluster
          providers={usage?.providers ?? []}
          live={liveUsage}
          onViewAll={() => setShowUsage(true)}
          onOpenSettings={() => void openSettingsWindow()}
        />
      </div>
    );
  }

  return (
    <div ref={surfaceRef} className="h-full">
      {body()}
    </div>
  );
}
