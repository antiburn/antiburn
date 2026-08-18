// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { open } from '@tauri-apps/plugin-dialog';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import {
  addScanRoot,
  cancelScan,
  closeCurrentWindow,
  defaultScanRoots,
  DEFAULT_SETTINGS,
  getFolderPermissions,
  getScanStatus,
  getSettings,
  listRepositories,
  listScanRoots,
  onSettingsChanged,
  onScanEvent,
  recheckFolderPermissions,
  removeScanRoot,
  scanNow,
  setRepositoryEnabled,
  setSettings,
  type AppSettings,
  type ScanStatus,
} from '../lib/ipc';
import { applyTheme } from '../lib/appearance';
import type {
  FolderPermissions,
  LocalRepositoryItem,
  LocalRepositoryStatus,
} from '../lib/types/repository';
import { useFolderPermissionFlow } from '../lib/useFolderPermissionFlow';
import { OnboardingFlow } from './onboarding/OnboardingFlow';

/**
 * The first-run window.
 *
 * `OnboardingFlow` is the five screens; this is everything they need from the
 * shell — the roots, the permissions, the repositories, the scan and its
 * status. All of it used to live in `PopoverView`, which owned it for one
 * surface out of four and could not use most of it for the other three. Moving
 * the flow into its own window (`src-tauri/src/onboarding.rs`, D-25) is what
 * let the state follow it.
 *
 * Two things this window does that the popover did for the flow, and one it
 * deliberately does not:
 *
 * - **⌘W closes it.** The app is an accessory with no application menu, so the
 *   standard shortcut has no owner unless a view claims it. The window hides
 *   rather than being destroyed, and the menu-bar item brings it back with the
 *   steps already walked still behind it.
 * - **Escape does nothing.** In the popover, Escape dismissed the window —
 *   correct for a transient tray surface, wrong here: this is a decorated
 *   window in the middle of a task, and a stray keystroke should not put it
 *   away.
 * - **No focus hold around the folder picker.** `withPopoverHold` exists
 *   because the popover hides when it loses focus and the picker takes it. A
 *   decorated window has no such rule, so the picker is opened plainly.
 */
export function OnboardingView() {
  const [settings, setSettingsState] = useState<AppSettings | null>(null);
  const settingsRef = useRef<AppSettings>(DEFAULT_SETTINGS);
  const confirmedSettingsRef = useRef<AppSettings>(DEFAULT_SETTINGS);
  const settingsWriteTail = useRef<Promise<void>>(Promise.resolve());
  const pendingSettingsWrites = useRef(0);
  const latestSettingsWrite = useRef(0);
  const [scanRoots, setScanRoots] = useState<string[]>([]);
  const [defaultRoots, setDefaultRoots] = useState<string[]>([]);
  const [permissions, setPermissions] = useState<FolderPermissions>({
    deferred: [],
    granted: [],
    supported: false,
  });
  const [repositories, setRepositories] = useState<LocalRepositoryItem[]>([]);
  const [scanStatus, setScanStatus] = useState<ScanStatus | null>(null);
  const [recheckingPermissions, setRecheckingPermissions] = useState(false);

  const windowDays = settings?.activityWindowDays ?? DEFAULT_SETTINGS.activityWindowDays;

  const refreshRepositoryList = useCallback(async () => {
    const payloads = await listRepositories().catch(() => []);
    setRepositories(
      payloads.map((payload) => ({ ...payload, status: repositoryStatus(payload.status) })),
    );
  }, []);

  // First load. Preferences decide the theme, so everything else waits on
  // them; the rest settles together because no step reads another's answer.
  useEffect(() => {
    let active = true;
    let sawSettingsEvent = false;
    const pendingSettings = onSettingsChanged((stored) => {
      sawSettingsEvent = true;
      // A local write emits this event before its invoke promise resolves. An
      // older event must not overwrite a newer optimistic choice queued behind
      // it; the final local response below becomes authoritative instead.
      if (!active || pendingSettingsWrites.current > 0) return;
      settingsRef.current = stored;
      confirmedSettingsRef.current = stored;
      applyTheme(stored.theme);
      setSettingsState(stored);
    });
    void (async () => {
      // Establish the event subscription before taking the initial snapshot;
      // otherwise a write from Settings can land between those two operations
      // and be replaced by the older read.
      await pendingSettings.catch(() => null);
      const stored = await getSettings().catch(() => DEFAULT_SETTINGS);
      if (!active) return;
      if (!sawSettingsEvent && pendingSettingsWrites.current === 0) {
        settingsRef.current = stored;
        confirmedSettingsRef.current = stored;
        applyTheme(stored.theme);
        setSettingsState(stored);
      }
      const [roots, defaults, status, folders] = await Promise.all([
        listScanRoots().catch(() => []),
        defaultScanRoots().catch(() => []),
        getScanStatus().catch(() => null),
        getFolderPermissions().catch(() => null),
      ]);
      if (!active) return;
      setScanRoots(roots);
      setDefaultRoots(defaults);
      setScanStatus(status);
      if (folders) setPermissions(folders);
      // Not awaited: the Repositories step is two screens away and this is a
      // store read nothing above depends on.
      void refreshRepositoryList();
    })();
    return () => {
      active = false;
      void pendingSettings.then((unlisten) => unlisten());
    };
  }, [refreshRepositoryList]);

  // The scan is the only thing that changes what is on screen behind the
  // reader's back. Every phase is kept, not just `finished` — the Historical
  // scan step's whole job is to show the pass while it runs.
  useEffect(() => {
    let active = true;
    const pending = onScanEvent((status, phase) => {
      if (!active) return;
      setScanStatus(status);
      if (phase !== 'finished') return;
      void refreshRepositoryList();
    });
    return () => {
      active = false;
      void pending.then((unlisten) => unlisten());
    };
  }, [refreshRepositoryList]);

  // ⌘W closes the window. Routed through a close *request*, which the shell
  // turns into a hide, so this takes the same path as the title-bar button.
  // Escape must NOT close — see the component docblock.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (
        (event.metaKey || event.ctrlKey) &&
        !event.altKey &&
        event.key.toLowerCase() === 'w'
      ) {
        event.preventDefault();
        void closeCurrentWindow();
      }
    }
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, []);

  const applySettings = useCallback((change: Partial<AppSettings>) => {
    const next = { ...settingsRef.current, ...change };
    settingsRef.current = next;
    setSettingsState(next);

    const writeNumber = latestSettingsWrite.current + 1;
    latestSettingsWrite.current = writeNumber;
    pendingSettingsWrites.current += 1;

    const write = settingsWriteTail.current.then(async () => {
      try {
        return { saved: await setSettings(next), persisted: true };
      } catch {
        return { saved: confirmedSettingsRef.current, persisted: false };
      }
    });
    const settled = write.then(({ saved, persisted }) => {
      if (persisted) confirmedSettingsRef.current = saved;
      pendingSettingsWrites.current -= 1;
      if (writeNumber === latestSettingsWrite.current) {
        settingsRef.current = saved;
        applyTheme(saved.theme);
        setSettingsState(saved);
      }
      return saved;
    });
    settingsWriteTail.current = settled.then(() => undefined);
    return settled;
  }, []);

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
    // A pass settles which folders are still out of reach; the Sources and
    // Repositories steps show that directly, so it has to be re-read rather
    // than assumed.
    const next = await getFolderPermissions().catch(() => null);
    if (next) setPermissions(next);
  }, []);

  // Each grant refreshes the repository list the reader is watching, rather
  // than making them wait for every folder in the queue.
  const permissionFlow = useFolderPermissionFlow(permissions.deferred, () => {
    void handleRescan();
    void refreshRepositoryList();
  });

  /**
   * Look for access granted in System Settings rather than through antiburn.
   *
   * The way out of a remembered refusal: macOS will not prompt again, so the
   * only path is the system pane, and nothing notices that until something
   * looks. Whatever it finds has to reach the step the reader is on, or the
   * control reads as broken in exactly the state it exists to fix.
   */
  const handleRecheckPermissions = useCallback(async () => {
    setRecheckingPermissions(true);
    const found = await recheckFolderPermissions().catch(() => []);
    if (found.length > 0) {
      await handleRescan();
      await refreshRepositoryList();
    } else {
      const next = await getFolderPermissions().catch(() => null);
      if (next) setPermissions(next);
    }
    setRecheckingPermissions(false);
  }, [handleRescan, refreshRepositoryList]);

  const handleCancelScan = useCallback(async () => {
    const status = await cancelScan().catch(() => null);
    if (status) setScanStatus(status);
  }, []);

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

  /**
   * Finish.
   *
   * Only the flag is written here. Everything else the transition causes —
   * putting this window away, the notification that says where the app went,
   * the first scan — is the shell's, keyed off the same write in
   * `commands::set_settings`. A window cannot both close itself and be sure the
   * notification that replaces it arrived.
   */
  const handleFinish = useCallback(async () => {
    await applySettings({ onboardingCompleted: true });
  }, [applySettings]);

  /**
   * Default roots antiburn has not actually read, because the operating system
   * is still guarding the folder they sit in.
   */
  const blockedRoots = useMemo(() => {
    if (!permissions.supported || permissions.deferred.length === 0) return [];
    const blocked = permissions.deferred.map((entry) => entry.dir);
    return defaultRoots.filter((root) =>
      blocked.some((dir) => root.split(/[\\/]/).includes(dir)),
    );
  }, [defaultRoots, permissions]);

  return (
    <div className="h-full">
      <OnboardingFlow
        defaultRoots={defaultRoots}
        blockedRoots={blockedRoots}
        permissions={permissions}
        permissionFlow={permissionFlow}
        onRecheckPermissions={() => void handleRecheckPermissions()}
        recheckingPermissions={recheckingPermissions}
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
        launchAtLogin={settings?.launchAtLogin ?? DEFAULT_SETTINGS.launchAtLogin}
        onLaunchAtLoginChange={(enabled) => void applySettings({ launchAtLogin: enabled })}
        onFinish={() => void handleFinish()}
      />
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
