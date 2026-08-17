// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { open } from '@tauri-apps/plugin-dialog';
import { FolderPlus, RefreshCw, X } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';

import { FolderPermissionNotice } from '../../components/repositories/FolderPermissionNotice';
import { LocalRepositoryList } from '../../components/repositories/LocalRepositoryList';
import { Card } from '../../components/ui/Card';
import { PaneHeader } from '../../components/ui/Pane';
import { PushButton } from '../../components/ui/PushButton';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { StatusText } from '../../components/ui/StatusText';
import {
  addScanRoot,
  getFolderPermissions,
  getScanStatus,
  listRepositories,
  listScanRoots,
  getConsentDiagnostics,
  onScanEvent,
  openFolderAccessSettings,
  recheckFolderPermissions,
  refreshRepositories,
  removeScanRoot,
  scanNow,
  setRepositoryEnabled,
  type RepositoryItemPayload,
  type ScanStatus,
} from '../../lib/ipc';
import type {
  FolderPermissions,
  LocalRepositoryItem,
  LocalRepositoryStatus,
} from '../../lib/types/repository';
import { useFolderPermissionFlow } from '../../lib/useFolderPermissionFlow';
import { scanStatusLabel } from '../popover/ScanStatusBar';
import { useAppSettings } from './useAppSettings';

/**
 * Sources: which repositories antiburn watches, and where it looks for them.
 *
 * Inclusion is opt-out — a repository found on disk is watched unless it is
 * turned off — and turning one off does more than hide a row: the shell also
 * records the path in the engine's opt-out store, so the *next scan* skips its
 * sessions entirely.
 */

/** Narrow the shell's status string to the list's union. */
function statusOf(payload: RepositoryItemPayload): LocalRepositoryStatus {
  switch (payload.status) {
    case 'accessible':
    case 'permission_denied':
    case 'not_cloned':
    case 'disabled':
      return payload.status;
    default:
      return 'accessible';
  }
}

function toItems(payloads: readonly RepositoryItemPayload[]): LocalRepositoryItem[] {
  return payloads.map((payload) => ({
    key: payload.key,
    repoName: payload.repoName,
    fullName: payload.fullName,
    status: statusOf(payload),
    repoRoot: payload.repoRoot,
    suspectedPath: payload.suspectedPath,
    worktreeCount: payload.worktreeCount,
    sessionCount: payload.sessionCount,
    wslDistro: payload.wslDistro,
    enabled: payload.enabled,
  }));
}

export function SourcesPane() {
  const { settings: appSettings } = useAppSettings();
  const [repositories, setRepositories] = useState<LocalRepositoryItem[]>([]);
  const [scanRoots, setScanRoots] = useState<string[]>([]);
  const [scanning, setScanning] = useState(true);
  const [scanStatus, setScanStatus] = useState<ScanStatus | null>(null);
  const [permissions, setPermissions] = useState<FolderPermissions>({
    deferred: [],
    granted: [],
    supported: false,
  });

  // The main view no longer carries a scan status line; this pane is where a
  // reader checks on and re-runs scanning, so it stays live via scan events.
  useEffect(() => {
    let active = true;
    void getScanStatus()
      .then((status) => {
        if (active) setScanStatus(status);
      })
      .catch(() => undefined);
    const unlisten = onScanEvent((status) => setScanStatus(status));
    return () => {
      active = false;
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  const handleRescanSessions = useCallback(async () => {
    const status = await scanNow().catch(() => null);
    if (status) setScanStatus(status);
  }, []);

  useEffect(() => {
    let active = true;
    void (async () => {
      const [repos, roots] = await Promise.all([
        listRepositories().catch(() => []),
        listScanRoots().catch(() => []),
      ]);
      if (!active) return;
      setRepositories(toItems(repos));
      setScanRoots(roots);
      setScanning(false);
    })();
    return () => {
      active = false;
    };
  }, []);

  const loadPermissions = useCallback(async () => {
    const next = await getFolderPermissions().catch(() => null);
    if (next) setPermissions(next);
  }, []);

  useEffect(() => {
    let active = true;
    void getFolderPermissions()
      .then((next) => {
        if (active && next) setPermissions(next);
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, []);

  const handleRefresh = useCallback(async () => {
    setScanning(true);
    const repos = await refreshRepositories().catch(() => []);
    setRepositories(toItems(repos));
    setScanning(false);
    // A pass that just ran may have granted or lost access; the notice above
    // the list has to agree with the list.
    await loadPermissions();
  }, [loadPermissions]);

  // Granting is the one path that can add repositories the reader is waiting
  // for, so each grant refreshes the list rather than making them wait for the
  // whole queue.
  const permissionFlow = useFolderPermissionFlow(permissions.deferred, () => {
    void handleRefresh();
  });
  const [rechecking, setRechecking] = useState(false);

  /**
   * Look for access granted in System Settings rather than through antiburn.
   *
   * The way out of a remembered refusal: macOS will not prompt again, so the
   * only path is the system pane, and nothing notices that until something
   * looks. This is the looking.
   */
  const handleRecheck = useCallback(async () => {
    setRechecking(true);
    const found = await recheckFolderPermissions().catch(() => []);
    if (found.length > 0) await handleRefresh();
    else await loadPermissions();
    setRechecking(false);
  }, [handleRefresh, loadPermissions]);

  /** Probe history, for a bug report. */
  const handleCopyDiagnostics = useCallback(async () => {
    const probes = await getConsentDiagnostics().catch(() => []);
    const text = probes
      .map((probe) => `${probe.outcome}\t${probe.elapsedMs}ms\t${probe.target}`)
      .join('\n');
    await navigator.clipboard.writeText(text || 'No folder-access probes this run.');
  }, []);

  const handleToggle = useCallback(async (item: LocalRepositoryItem, enabled: boolean) => {
    const repos = await setRepositoryEnabled(item.key, enabled).catch(
      () => [] as RepositoryItemPayload[],
    );
    if (repos.length > 0) setRepositories(toItems(repos));
  }, []);

  /**
   * "Locate" points the scanner at the folder a missing repository lives in,
   * rather than at the repository itself: the engine's scan roots are
   * directories it walks, and the parent is what makes the clone — and its
   * siblings — findable on the next pass.
   */
  const handleLocate = useCallback(async () => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked !== 'string') return;
    setScanRoots(await addScanRoot(picked));
    await handleRefresh();
  }, [handleRefresh]);

  const handleRemoveRoot = useCallback(async (path: string) => {
    setScanRoots(await removeScanRoot(path));
  }, []);

  return (
    <>
      <PaneHeader title="Sources" />
      <div className="space-y-6">
        {permissions.supported && permissions.deferred.length > 0 ? (
          <FolderPermissionNotice
            deferred={permissions.deferred}
            phase={permissionFlow.phase}
            current={permissionFlow.current}
            position={permissionFlow.position}
            total={permissionFlow.total}
            recordedDenials={permissionFlow.recordedDenials}
            onRequest={permissionFlow.start}
            onOpenSettings={() => void openFolderAccessSettings()}
            onRecheck={() => void handleRecheck()}
            onCopyDiagnostics={() => void handleCopyDiagnostics()}
            rechecking={rechecking}
          />
        ) : null}

        <SectionGroup
          title="Scanning"
          trailing={
            <StatusText tone="secondary">
              {scanStatusLabel(scanStatus, appSettings.discoveryPaused)}
            </StatusText>
          }
        >
          <Card>
            <div className="flex items-center justify-between gap-3 px-4 py-3">
              <p className="type-footnote text-label-secondary">
                Looks for new sessions on this machine and re-reads ones that changed.
              </p>
              <PushButton
                className="shrink-0 gap-1.5"
                disabled={scanStatus?.running === true}
                onClick={() => void handleRescanSessions()}
              >
                <RefreshCw size={12} aria-hidden="true" />
                {scanStatus?.running ? 'Scanning…' : 'Rescan'}
              </PushButton>
            </div>
          </Card>
        </SectionGroup>

        <SectionGroup
          title="Scan folders"
          trailing={
            <StatusText tone="secondary">
              {scanRoots.length === 0
                ? 'Defaults only'
                : `${scanRoots.length} extra ${scanRoots.length === 1 ? 'folder' : 'folders'}`}
            </StatusText>
          }
        >
          <Card>
            <div className="space-y-2 px-4 py-3">
              <p className="type-footnote text-label-secondary">
                Agent session stores and the usual code directories are searched automatically.
                Add a folder only if you keep repositories somewhere else.
              </p>
              {scanRoots.length > 0 && (
                <ul className="space-y-1">
                  {scanRoots.map((root) => (
                    <li key={root} className="flex items-center gap-2">
                      <span
                        dir="rtl"
                        title={root}
                        className="min-w-0 flex-1 truncate text-left type-footnote text-label"
                      >
                        <bdi>{root}</bdi>
                      </span>
                      <button
                        type="button"
                        onClick={() => void handleRemoveRoot(root)}
                        aria-label={`Stop scanning ${root}`}
                        className="shrink-0 rounded p-0.5 text-label-tertiary hover:bg-surface-hover hover:text-label-secondary"
                      >
                        <X size={12} strokeWidth={2.5} aria-hidden="true" />
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              <PushButton className="gap-1.5" onClick={() => void handleLocate()}>
                <FolderPlus size={12} aria-hidden="true" />
                Add a folder…
              </PushButton>
            </div>
          </Card>
        </SectionGroup>

        <SectionGroup title="Repositories">
          <Card className="h-[280px]">
            <LocalRepositoryList
              repositories={repositories}
              loading={scanning}
              onToggleRepository={(item, enabled) => void handleToggle(item, enabled)}
              onRefresh={() => void handleRefresh()}
              onLocate={() => void handleLocate()}
            />
          </Card>
        </SectionGroup>
      </div>
    </>
  );
}
