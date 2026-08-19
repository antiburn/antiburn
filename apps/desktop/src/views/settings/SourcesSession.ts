// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { open } from "@tauri-apps/plugin-dialog"

import {
  addScanRoot,
  getConsentDiagnostics,
  getFolderPermissions,
  listRepositories,
  listScanRoots,
  recheckFolderPermissions,
  refreshRepositories,
  removeScanRoot,
  setRepositoryEnabled,
  type RepositoryItemPayload,
} from "../../lib/ipc"
import type {
  FolderPermissions,
  LocalRepositoryItem,
  LocalRepositoryStatus,
} from "../../lib/types/repository"

const EMPTY_PERMISSIONS: FolderPermissions = {
  deferred: [],
  granted: [],
  supported: false,
}

export type SourcesSnapshot = {
  repositories: LocalRepositoryItem[]
  scanRoots: string[]
  permissions: FolderPermissions
  /** True while the repository list is being (re)built from disk. */
  scanning: boolean
}

/** Narrow the shell's status string to the list's union. */
function statusOf(payload: RepositoryItemPayload): LocalRepositoryStatus {
  switch (payload.status) {
    case "accessible":
    case "permission_denied":
    case "not_cloned":
    case "disabled":
      return payload.status
    default:
      return "accessible"
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
  }))
}

/**
 * The imperative boundary behind the Sources pane.
 *
 * React reads immutable snapshots through `useSyncExternalStore`; the
 * repository list, scan roots and folder permissions — and every command that
 * changes them — live here instead. Scan *status* is deliberately not part of
 * this session: `scanStatusStore` (`lib/scanStatusStore.ts`) already owns that
 * subscription for both panes that show it, and subscribing to it a second
 * time here would just be two listeners racing to report the same events.
 */
export class SourcesSession {
  private listeners = new Set<() => void>()
  private started = false
  private generation = 0

  private snapshot: SourcesSnapshot = {
    repositories: [],
    scanRoots: [],
    permissions: EMPTY_PERMISSIONS,
    scanning: true,
  }

  getSnapshot = (): SourcesSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (!this.started) void this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  /**
   * Re-derive the repository list from what is on disk right now.
   *
   * A pass that just ran may have granted or lost access; the notice above
   * the list has to agree with the list, so this always ends with a
   * permissions read too.
   */
  refresh = async (): Promise<void> => {
    this.update({ scanning: true })
    const repos = await refreshRepositories().catch(() => [])
    this.update({ repositories: toItems(repos), scanning: false })
    await this.loadPermissions()
  }

  loadPermissions = async (): Promise<void> => {
    const next = await getFolderPermissions().catch(() => null)
    if (next) this.update({ permissions: next })
  }

  toggleRepository = async (item: LocalRepositoryItem, enabled: boolean): Promise<void> => {
    const repos = await setRepositoryEnabled(item.key, enabled).catch(
      () => [] as RepositoryItemPayload[],
    )
    if (repos.length > 0) this.update({ repositories: toItems(repos) })
  }

  /**
   * "Locate" points the scanner at the folder a missing repository lives in,
   * rather than at the repository itself: the engine's scan roots are
   * directories it walks, and the parent is what makes the clone — and its
   * siblings — findable on the next pass.
   */
  locate = async (): Promise<void> => {
    const picked = await open({ directory: true, multiple: false })
    if (typeof picked !== "string") return
    this.update({ scanRoots: await addScanRoot(picked) })
    await this.refresh()
  }

  removeRoot = async (path: string): Promise<void> => {
    this.update({ scanRoots: await removeScanRoot(path) })
  }

  /**
   * Look for access granted in System Settings rather than through antiburn.
   *
   * The way out of a remembered refusal: macOS will not prompt again, so the
   * only path is the system pane, and nothing notices that until something
   * looks. This is the looking.
   */
  recheck = async (): Promise<void> => {
    const found = await recheckFolderPermissions().catch(() => [])
    if (found.length > 0) await this.refresh()
    else await this.loadPermissions()
  }

  /** Probe history, for a bug report. */
  copyDiagnostics = async (): Promise<void> => {
    const probes = await getConsentDiagnostics().catch(() => [])
    const text = probes
      .map((probe) => `${probe.outcome}\t${probe.elapsedMs}ms\t${probe.target}`)
      .join("\n")
    await navigator.clipboard.writeText(text || "No folder-access probes this run.")
  }

  private start = async (): Promise<void> => {
    this.started = true
    const generation = ++this.generation

    const [repos, scanRoots, permissions] = await Promise.all([
      listRepositories().catch(() => []),
      listScanRoots().catch(() => []),
      getFolderPermissions()
        .then((value) => value ?? EMPTY_PERMISSIONS)
        .catch(() => EMPTY_PERMISSIONS),
    ])
    if (generation !== this.generation) return

    this.update({
      repositories: toItems(repos),
      scanRoots,
      permissions,
      scanning: false,
    })
  }

  private stop(): void {
    this.started = false
    this.generation += 1
  }

  private update(change: Partial<SourcesSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change }
    for (const listener of this.listeners) listener()
  }
}
