// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { open } from '@tauri-apps/plugin-dialog';
import { Settings2 } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';

import { LocalActivityList } from '../components/activity/LocalActivityList';
import type { LocalActivityEntry } from '../components/activity/LocalActivityList';
import { Skeleton } from '../components/ui/Skeleton';
import { renderAgentIcon } from '../lib/agentIcon';
import { indexOfSession, toActivityEntries } from '../lib/activityEntries';
import { applyTheme } from '../lib/appearance';
import {
  addScanRoot,
  defaultScanRoots,
  DEFAULT_SETTINGS,
  getSettings,
  listRecentSessions,
  listScanRoots,
  onScanEvent,
  openSettingsWindow,
  removeScanRoot,
  setSettings,
  type AppSettings,
} from '../lib/ipc';
import { OnboardingFlow } from './popover/OnboardingFlow';
import { SessionPane, type SessionSubject } from './popover/SessionPane';

/**
 * The tray popover.
 *
 * Three surfaces share one 380px window: the first-run flow, the activity list,
 * and one session's analytics. There is no router — a popover is a single
 * place, and a stack of "where I came from" is all the navigation it needs.
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

export function PopoverView() {
  const [settings, setSettingsState] = useState<AppSettings | null>(null);
  const [entries, setEntries] = useState<LocalActivityEntry[] | null>(null);
  const [scanRoots, setScanRoots] = useState<string[]>([]);
  const [defaultRoots, setDefaultRoots] = useState<string[]>([]);
  /** Navigation stack. Empty means the activity list is showing. */
  const [stack, setStack] = useState<SessionSubject[]>([]);

  const current = stack.at(-1) ?? null;
  const windowDays = settings?.activityWindowDays ?? DEFAULT_SETTINGS.activityWindowDays;

  const refreshEntries = useCallback(async (days: number) => {
    const payloads = await listRecentSessions(days);
    setEntries(toActivityEntries(payloads));
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
      const [roots, defaults] = await Promise.all([
        listScanRoots().catch(() => []),
        defaultScanRoots().catch(() => []),
      ]);
      if (!active) return;
      setScanRoots(roots);
      setDefaultRoots(defaults);
      if (stored.onboardingCompleted) {
        await refreshEntries(stored.activityWindowDays).catch(() => setEntries([]));
      }
    })();
    return () => {
      active = false;
    };
  }, [refreshEntries]);

  // A finished scan is the only thing that changes the list behind the reader's
  // back, so that is what the list listens for rather than polling.
  useEffect(() => {
    if (!settings?.onboardingCompleted) return;
    let active = true;
    const pending = onScanEvent((_status, phase) => {
      if (phase !== 'finished' || !active) return;
      void refreshEntries(windowDays).catch(() => {});
    });
    return () => {
      active = false;
      void pending.then((unlisten) => unlisten());
    };
  }, [settings?.onboardingCompleted, windowDays, refreshEntries]);

  const handleAddScanRoot = useCallback(async () => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked !== 'string') return;
    setScanRoots(await addScanRoot(picked));
  }, []);

  const handleRemoveScanRoot = useCallback(async (path: string) => {
    setScanRoots(await removeScanRoot(path));
  }, []);

  const handleFinishOnboarding = useCallback(async () => {
    const base = settings ?? DEFAULT_SETTINGS;
    const saved = await setSettings({ ...base, onboardingCompleted: true });
    setSettingsState(saved);
    setEntries(null);
    await refreshEntries(saved.activityWindowDays).catch(() => setEntries([]));
  }, [settings, refreshEntries]);

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

  // Onboarding gates everything: without it there is nothing scanned to show.
  if (settings && !settings.onboardingCompleted) {
    return (
      <OnboardingFlow
        defaultRoots={defaultRoots}
        scanRoots={scanRoots}
        onAddScanRoot={() => void handleAddScanRoot()}
        onRemoveScanRoot={(path) => void handleRemoveScanRoot(path)}
        onFinish={() => void handleFinishOnboarding()}
      />
    );
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
        <h1 className="type-headline text-label">antiburn</h1>
        <button
          type="button"
          onClick={() => void openSettingsWindow()}
          aria-label="Open settings"
          className="-mr-1.5 ml-auto inline-flex h-6 shrink-0 items-center rounded-control px-1.5 text-label-secondary hover:bg-surface-hover"
        >
          <Settings2 size={14} strokeWidth={2} aria-hidden="true" />
        </button>
      </header>

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
    </div>
  );
}
