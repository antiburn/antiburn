// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { FolderPlus, RefreshCw, X } from 'lucide-react';
import { useCallback, useState, useSyncExternalStore } from 'react';

import { FolderPermissionNotice } from '../../components/repositories/FolderPermissionNotice';
import { LocalRepositoryList } from '../../components/repositories/LocalRepositoryList';
import { Card } from '../../components/ui/Card';
import { PaneHeader } from '../../components/ui/Pane';
import { PushButton } from '../../components/ui/PushButton';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { StatusText } from '../../components/ui/StatusText';
import { openFolderAccessSettings, scanNow } from '../../lib/ipc';
import { scanStatusStore } from '../../lib/scanStatusStore';
import type { LocalRepositoryItem } from '../../lib/types/repository';
import { useFolderPermissionFlow } from '../../lib/useFolderPermissionFlow';
import { scanStatusLabel } from '../popover/ScanStatusBar';
import { SourcesSession } from './SourcesSession';

/**
 * Sources: which repositories antiburn watches, and where it looks for them.
 *
 * Inclusion is opt-out — a repository found on disk is watched unless it is
 * turned off — and turning one off does more than hide a row: the shell also
 * records the path in the engine's opt-out store, so the *next scan* skips its
 * sessions entirely.
 *
 * The repository list, scan roots and folder permissions live in
 * `SourcesSession` (the external-system boundary this component subscribes
 * to); scan *status* is a separate subscription to `scanStatusStore`, shared
 * with `GeneralPane` — the two panes never mount at once.
 */

export interface SourcesPaneProps {
  discoveryPaused: boolean;
}

export function SourcesPane({ discoveryPaused }: SourcesPaneProps) {
  const [session] = useState(() => new SourcesSession());
  const { repositories, scanRoots, permissions, scanning } = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
  );
  const scanStatus = useSyncExternalStore(
    scanStatusStore.subscribe,
    scanStatusStore.getSnapshot,
  );

  const handleRescanSessions = useCallback(async () => {
    const status = await scanNow().catch(() => null);
    if (status) scanStatusStore.set(status);
  }, []);

  // Granting is the one path that can add repositories the reader is waiting
  // for, so each grant refreshes the list rather than making them wait for the
  // whole queue.
  const permissionFlow = useFolderPermissionFlow(permissions.deferred, () => {
    void session.refresh();
  });
  const [rechecking, setRechecking] = useState(false);

  const handleRecheck = useCallback(async () => {
    setRechecking(true);
    await session.recheck();
    setRechecking(false);
  }, [session]);

  const handleCopyDiagnostics = useCallback(() => session.copyDiagnostics(), [session]);

  const handleToggle = useCallback(
    (item: LocalRepositoryItem, enabled: boolean) => session.toggleRepository(item, enabled),
    [session],
  );

  const handleLocate = useCallback(() => session.locate(), [session]);

  const handleRemoveRoot = useCallback((path: string) => session.removeRoot(path), [session]);

  const handleRefresh = useCallback(() => session.refresh(), [session]);

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
              {scanStatusLabel(scanStatus, discoveryPaused)}
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
