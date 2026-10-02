import { invoke, isTauri } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"
import { onSessionIndexChanged } from "./sessionIpc"

export const REMOTE_HOST_LIMIT = 8

type RemoteHostErrorCategory =
  | "sshUnavailable"
  | "authenticationFailed"
  | "hostKeyFailed"
  | "helperMissing"
  | "incompatibleProtocol"
  | "unsupportedPlatform"
  | "cacheLimit"
  | "transferFailed"
  | "analysisFailed"
  | "cancelled"
  | "unknown"

interface RemoteHostError {
  category: RemoteHostErrorCategory
  message: string
}

export interface RemoteHost {
  id: string
  sshAlias: string
  displayName: string | null
  automaticSyncEnabled: boolean
  status: "idle" | "syncing" | "error"
  lastSuccessfulSyncEpoch: number | null
  cachedSessionCount: number
  lastError: RemoteHostError | null
}

export interface RemoteHostPreflight {
  status: "ready" | RemoteHostErrorCategory
  supportedAgents: ("claude-code" | "codex")[]
  platform: "linux" | null
  architecture: "x86_64" | "aarch64" | null
  /** The release that shipped the installed helper. Null when it did not answer. */
  helperVersion: string | null
  message: string | null
}

export type RemoteSyncIntervalSecs = 0 | 60 | 300 | 900 | 1800 | 3600

export interface RemoteSyncStatus {
  intervalSecs: RemoteSyncIntervalSecs
  active: { hostId: string; completed: number; total: number } | null
  pendingHostIds: string[]
}

export interface RemoteHostsSnapshot {
  hosts: readonly RemoteHost[]
  loaded: boolean
  loading: boolean
  sync: RemoteSyncStatus
}

const EMPTY_SYNC: RemoteSyncStatus = { intervalSecs: 300, active: null, pendingHostIds: [] }
const NOOP_UNLISTEN: UnlistenFn = () => undefined

async function getRemoteHosts(): Promise<RemoteHost[]> {
  if (!isTauri()) return []
  return (await invoke<RemoteHost[] | null>("get_remote_hosts")) ?? []
}

export async function checkRemoteHost(sshAlias: string): Promise<RemoteHostPreflight> {
  if (!isTauri())
    return {
      status: "ready",
      supportedAgents: ["claude-code", "codex"],
      platform: "linux",
      architecture: "x86_64",
      helperVersion: null,
      message: null,
    }
  return invoke<RemoteHostPreflight>("check_remote_host", { sshAlias })
}

export async function addRemoteHost(
  sshAlias: string,
  displayName: string | null,
): Promise<RemoteHost> {
  return invoke<RemoteHost>("add_remote_host", { sshAlias, displayName })
}

export async function updateRemoteHost(
  id: string,
  sshAlias: string,
  displayName: string | null,
): Promise<RemoteHost> {
  return invoke<RemoteHost>("update_remote_host", { id, sshAlias, displayName })
}

export async function setRemoteHostSyncEnabled(id: string, enabled: boolean): Promise<void> {
  await invoke("set_remote_host_sync_enabled", { id, enabled })
}

export async function removeRemoteHost(id: string): Promise<void> {
  await invoke("remove_remote_host", { id })
}

export async function scanRemoteHost(id: string): Promise<void> {
  await invoke("scan_remote_host", { id })
}

export async function openRemoteHelperDownloads(): Promise<void> {
  await invoke("open_remote_helper_downloads")
}

async function getRemoteSyncStatus(): Promise<RemoteSyncStatus> {
  if (!isTauri()) return EMPTY_SYNC
  return (await invoke<RemoteSyncStatus | null>("get_remote_sync_status")) ?? EMPTY_SYNC
}

async function setRemoteSyncInterval(
  seconds: RemoteSyncIntervalSecs,
): Promise<RemoteSyncStatus> {
  if (!isTauri()) return { ...EMPTY_SYNC, intervalSecs: seconds }
  return invoke<RemoteSyncStatus>("set_remote_sync_interval", { seconds })
}

async function onRemoteHostsChanged(
  handler: (hosts: RemoteHost[]) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return NOOP_UNLISTEN
  return listen<RemoteHost[]>("remote-hosts-changed", (event) => handler(event.payload))
}

async function onRemoteSyncStatus(
  handler: (status: RemoteSyncStatus) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return NOOP_UNLISTEN
  return listen<RemoteSyncStatus>("remote-sync-status", (event) => handler(event.payload))
}

class RemoteHostsStore {
  private snapshot: RemoteHostsSnapshot = {
    hosts: [],
    loaded: false,
    loading: false,
    sync: EMPTY_SYNC,
  }
  private listeners = new Set<() => void>()
  private generation = 0
  private hostsRevision = 0
  private syncRevision = 0
  private stops: UnlistenFn[] = []

  getSnapshot = (): RemoteHostsSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) void this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  refresh = async (): Promise<void> => {
    const generation = this.generation
    const revision = ++this.hostsRevision
    this.publish({ loading: true })
    try {
      const hosts = await getRemoteHosts()
      if (generation !== this.generation) return
      if (revision === this.hostsRevision) this.publish({ hosts, loaded: true, loading: false })
      else this.publish({ loading: false })
    } catch {
      if (generation === this.generation) this.publish({ loading: false })
    }
  }

  setInterval = async (seconds: RemoteSyncIntervalSecs): Promise<void> => {
    const generation = this.generation
    const revision = ++this.syncRevision
    const sync = await setRemoteSyncInterval(seconds)
    if (generation === this.generation && revision === this.syncRevision) this.publish({ sync })
  }

  private start = async (): Promise<void> => {
    const generation = ++this.generation
    this.publish({ loading: true })
    const listeners = await Promise.allSettled([
      onRemoteHostsChanged((hosts) => {
        if (generation !== this.generation) return
        this.hostsRevision += 1
        this.publish({ hosts, loaded: true, loading: false })
      }),
      onRemoteSyncStatus((sync) => {
        if (generation !== this.generation) return
        this.syncRevision += 1
        this.publish({ sync })
      }),
      onSessionIndexChanged(() => {
        if (generation === this.generation) void this.refresh()
      }),
    ])
    const listenerStops = listeners.flatMap((result) =>
      result.status === "fulfilled" ? [result.value] : [],
    )
    if (generation !== this.generation) {
      for (const stop of listenerStops) stop()
      return
    }
    this.stops.push(...listenerStops)
    const hostsRevision = this.hostsRevision
    const syncRevision = this.syncRevision
    const [hostsResult, syncResult] = await Promise.allSettled([
      getRemoteHosts(),
      getRemoteSyncStatus(),
    ])
    if (generation !== this.generation) return
    this.publish({
      ...(hostsResult.status === "fulfilled" && hostsRevision === this.hostsRevision
        ? { hosts: hostsResult.value, loaded: true }
        : {}),
      ...(syncResult.status === "fulfilled" && syncRevision === this.syncRevision
        ? { sync: syncResult.value }
        : {}),
      loading: false,
    })
  }

  private stop(): void {
    this.generation += 1
    for (const stop of this.stops.splice(0)) stop()
  }

  private publish(change: Partial<RemoteHostsSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change }
    for (const listener of this.listeners) listener()
  }
}

export const remoteHosts = new RemoteHostsStore()

export function remoteHostLabel(host: Pick<RemoteHost, "displayName" | "sshAlias">): string {
  return host.displayName?.trim() || host.sshAlias
}
