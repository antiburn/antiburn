// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { open } from '@tauri-apps/plugin-dialog';
import { FolderPlus, X } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';

import { LocalRepositoryList } from '../../components/repositories/LocalRepositoryList';
import { Card } from '../../components/ui/Card';
import { PaneHeader } from '../../components/ui/Pane';
import { PushButton } from '../../components/ui/PushButton';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { StatusText } from '../../components/ui/StatusText';
import {
  addScanRoot,
  listRepositories,
  listScanRoots,
  refreshRepositories,
  removeScanRoot,
  setRepositoryEnabled,
  type RepositoryItemPayload,
} from '../../lib/ipc';
import type { LocalRepositoryItem, LocalRepositoryStatus } from '../../lib/types/repository';

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
  const [repositories, setRepositories] = useState<LocalRepositoryItem[]>([]);
  const [scanRoots, setScanRoots] = useState<string[]>([]);
  const [scanning, setScanning] = useState(true);

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

  const handleRefresh = useCallback(async () => {
    setScanning(true);
    const repos = await refreshRepositories().catch(() => []);
    setRepositories(toItems(repos));
    setScanning(false);
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
