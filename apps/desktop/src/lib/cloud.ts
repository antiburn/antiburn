import { invoke } from "@tauri-apps/api/core"
import { open } from "@tauri-apps/plugin-dialog"
import { hasShell } from "./ipc"
import { listen } from "./tauriEvents"

export type CloudStatus =
  | "idle"
  | "opening"
  | "waiting"
  | "returned"
  | "signed_in"
  | "signed_out"
  | "cancelled"
  | "expired"
  | "browser_failed"
  | "offline"
  | "rejected"
  | "invalid_response"
  | "credential_failed"
  | "unavailable"
export type CloudScreen = "handoff" | "identity" | "folders" | "complete"
interface CloudFolder {
  sessionCount?: number | null
  sessionCountIsPartial?: boolean
  path: string
  selected: boolean
}
export interface CloudDraft {
  screen: CloudScreen
  folders: CloudFolder[]
}
export interface CloudSnapshot {
  authEnabled: boolean
  syncEnabled: boolean
  status: CloudStatus
  identity: { id: string; login: string; avatar?: string } | null
  expiresAt?: number | null
  pendingSync?: { requestId: string; result: SyncResult } | null
  draft: CloudDraft
}
export type AgentReadiness = "pending" | "ready" | "unavailable" | "failed"
export type SyncResult =
  | { status: "unavailable" | "idle" | "starting" | "ambiguous" | "failed" | "cancelled" }
  | {
      status: "accepted" | "syncing" | "partial" | "complete"
      operationId: string
      completed: number
      total: number
    }
export interface CloudPort {
  read(): Promise<CloudSnapshot>
  watch(listener: (value: CloudSnapshot) => void): Promise<() => void>
  begin(): Promise<CloudSnapshot>
  cancel(): Promise<CloudSnapshot>
  save(draft: CloudDraft, foldersEdited?: boolean): Promise<CloudSnapshot>
  signOut(): Promise<CloudSnapshot>
  pick(): Promise<string[]>
  agent(): Promise<AgentReadiness>
  prepareSync(folders: CloudFolder[], requestId: string): Promise<void>
  start(folders: CloudFolder[], requestId: string): Promise<SyncResult>
  reconcile(requestId: string): Promise<SyncResult>
  cancelSync(requestId: string): Promise<SyncResult>
}
export const emptyCloudSnapshot: CloudSnapshot = {
  authEnabled: false,
  syncEnabled: false,
  status: "idle",
  identity: null,
  draft: { screen: "handoff", folders: [] },
}
export const nativeCloudPort: CloudPort = {
  read: () => (hasShell() ? invoke("cloud_status") : Promise.resolve(emptyCloudSnapshot)),
  watch: (listener) =>
    hasShell()
      ? listen<CloudSnapshot>("cloud:changed", (event) => listener(event.payload))
      : Promise.resolve(() => {}),
  begin: () =>
    hasShell()
      ? invoke("cloud_begin")
      : Promise.resolve({ ...emptyCloudSnapshot, status: "unavailable" }),
  cancel: () => invoke("cloud_cancel"),
  save: (value, foldersEdited = true) => invoke("cloud_save_draft", { value, foldersEdited }),
  signOut: () => invoke("cloud_sign_out"),
  pick: async () => {
    const selected = await open({
      directory: true,
      multiple: true,
      title: "Choose folders for Cloud",
    })
    return selected === null ? [] : Array.isArray(selected) ? selected : [selected]
  },
  agent: async () => {
    if (!hasShell()) return "unavailable"
    const statuses = await invoke<{ phase: string }[]>("get_agent_integrations")
    if (statuses.some((value) => value.phase === "active")) return "ready"
    if (
      statuses.some((value) =>
        ["registered", "awaiting_trust", "reload_required", "pending_native_gate"].includes(
          value.phase,
        ),
      )
    )
      return "pending"
    return "unavailable"
  },
  prepareSync: async () => {
    throw new Error("Cloud sync is unavailable.")
  },
  start: async () => {
    await invoke("cloud_start_sync")
    return { status: "unavailable" }
  },
  reconcile: () => Promise.resolve({ status: "unavailable" }),
  cancelSync: () => Promise.resolve({ status: "unavailable" }),
}
