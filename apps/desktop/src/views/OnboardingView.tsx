// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { open } from '@tauri-apps/plugin-dialog';
import { useCallback, useEffect, useMemo, useState } from 'react';

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
  // Onboarding is a form: its choices remain local until Finish commits one
  // complete settings value. Keeping a draft makes rapid changes deterministic
  // without coordinating multiple asynchronous writes.
  const [settingsDraft, setSettingsDraft] = useState<
    Pick<AppSettings, 'activityWindowDays' | 'launchAtLogin'> | undefined
  >();
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
    void (async () => {
      const stored = await getSettings().catch(() => DEFAULT_SETTINGS);
      if (!active) return;
      applyTheme(stored.theme);
      setSettingsDraft({
        activityWindowDays: stored.activityWindowDays,
        launchAtLogin: stored.launchAtLogin,
      });
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
   * The draft preferences and completion flag are written together. Everything
   * else the transition causes — putting this window away, the notification
   * that says where the app went, the first scan — is the shell's, keyed off
   * the same write in `commands::set_settings`. A window cannot both close
   * itself and be sure the notification that replaces it arrived.
   */
  const handleFinish = useCallback(async () => {
    if (!settingsDraft) return;
    const current = await getSettings().catch(() => null);
    if (!current) return;
    const saved = await setSettings({
      ...current,
      ...settingsDraft,
      onboardingCompleted: true,
    }).catch(() => null);
    if (saved) {
      setSettingsDraft({
        activityWindowDays: saved.activityWindowDays,
        launchAtLogin: saved.launchAtLogin,
      });
    }
  }, [settingsDraft]);

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

  if (!settingsDraft) return null;

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
        windowDays={settingsDraft.activityWindowDays}
        onWindowDaysChange={(days) =>
          setSettingsDraft({
            ...settingsDraft,
            activityWindowDays: days,
          })
        }
        launchAtLogin={settingsDraft.launchAtLogin}
        onLaunchAtLoginChange={(enabled) =>
          setSettingsDraft({
            ...settingsDraft,
            launchAtLogin: enabled,
          })
        }
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
