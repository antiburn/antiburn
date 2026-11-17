/** Feature-gated native Cloud IPC. No credential or native source bytes cross
 * this renderer boundary; saved requests are not capture or upload acceptance.
 */
import { invoke } from "@tauri-apps/api/core"
import { open } from "@tauri-apps/plugin-dialog"
import { appInfo, hasShell } from "./ipc"
import { listen } from "./tauriEvents"

/** Closed native auth states. A signed-in account still needs content consent. */
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
  | "reauth_required"
  | "revocation_unconfirmed"
/** Verified destination display data. Only a granted personal workspace is usable. */
export interface CloudWorkspace {
  id: string
  name: string
  kind: "personal" | "team"
}
/** Nested setup view, separate from the host's persisted onboarding step. */
export type CloudScreen = "handoff" | "identity" | "folders" | "complete"
interface CloudFolder {
  sessionCount?: number | null
  sessionCountIsPartial?: boolean
  path: string
  selected: boolean
}
/** Account-bound UI preferences, never permission to read or upload content. */
export interface CloudDraft {
  screen: CloudScreen
  folders: CloudFolder[]
}
/** Bind renderer intent to the current internal account and granted destination. */
export interface CloudDestinationBinding {
  accountId: string
  destinationId: string
}
/** One explicit confirmation of exact roots and full content for a saved request.
 * This existing consent excludes attachments and must not be reused for new formats.
 */
export interface CaptureConsent extends CloudDestinationBinding {
  version: "native-full-content-v1"
  fullContent: true
  attachments: false
  selectedRoots: string[]
}
/** Native account state, not a build gate. authEnabled requires valid service
 * configuration; captureEnabled permits saved requests, never content upload.
 */
export interface CloudSnapshot {
  authEnabled: boolean
  syncEnabled: boolean
  captureEnabled?: boolean
  status: CloudStatus
  identity: { id: string; login: string; avatar?: string } | null
  expiresAt?: number | null
  grantExpiresAt?: number | null
  destination?: CloudWorkspace | null
  revocationPending?: boolean
  localCleanupPending?: boolean
  localResetAvailable?: boolean
  remoteRevocationUnconfirmed?: boolean
  setupError?: string | null
  pendingSync?: { requestId: string; result: SyncResult } | null
  draft: CloudDraft
}
/** Local coding-agent integration status, not Cloud authentication or upload status. */
export type AgentReadiness = "pending" | "ready" | "unavailable" | "failed"
/** Native returns saved-request states only; operation progress is for isolated previews. */
export type SyncResult =
  | {
      status:
        | "unavailable"
        | "upload_unavailable"
        | "idle"
        | "starting"
        | "ambiguous"
        | "failed"
        | "cancelled"
    }
  | {
      status: "accepted" | "syncing" | "partial" | "complete"
      operationId: string
      completed: number
      total: number
    }
/**
 * Native commands for setup and Settings.
 * Attach watch before read: commands, including reads, can emit state before
 * their reply. Implementations keep secrets native and return safe errors.
 * Saved requests survive observer teardown and account cleanup.
 */
export interface CloudPort {
  /** Refresh native authority. A state event can precede this reply. */
  read(): Promise<CloudSnapshot>
  /** Observe safe snapshots until the returned stop function is called. */
  watch(listener: (value: CloudSnapshot) => void): Promise<() => void>
  /** Open browser approval, or the information page when service config is absent. */
  begin(): Promise<CloudSnapshot>
  /** Cancel a pending browser attempt only; keep established auth and saved requests. */
  cancel(): Promise<CloudSnapshot>
  /** Save preferences only for the exact current personal account binding. */
  save(
    draft: CloudDraft,
    binding: CloudDestinationBinding,
    foldersEdited?: boolean,
  ): Promise<CloudSnapshot>
  /** Close local access first, then revoke this grant and finish secure cleanup. */
  signOut(): Promise<CloudSnapshot>
  /** Permit reconnect only without saved proof; retain the remote-revocation warning. */
  resetLocal(): Promise<CloudSnapshot>
  /** Ask for explicit native source directories without inferring repository sources. */
  pick(): Promise<string[]>
  /** Read local integration readiness independently of saved-request reconciliation. */
  agent(): Promise<AgentReadiness>
  /** Save exact account/root consent before listing. Reuse this ID after uncertainty. */
  prepareSync(folders: CloudFolder[], requestId: string, consent: CaptureConsent): Promise<void>
  /** List a saved native selection only; this build cannot capture or upload bytes. */
  start(folders: CloudFolder[], requestId: string, consent: CaptureConsent): Promise<SyncResult>
  /** Read saved state only. Reconciliation must not rescan or submit an upload. */
  reconcile(requestId: string): Promise<SyncResult>
  /** Mark the original account's saved selection cancelled without deleting bytes. */
  cancelSync(requestId: string): Promise<SyncResult>
}
/** Safe busy rejection means native work did not start; never surface other raw errors. */
export const CLOUD_BUSY_MESSAGE = "Cloud is busy. Try again."
/** Closed renderer default for a browser or a native build without Cloud. */
export const emptyCloudSnapshot: CloudSnapshot = {
  authEnabled: false,
  syncEnabled: false,
  captureEnabled: false,
  status: "idle",
  identity: null,
  draft: { screen: "handoff", folders: [] },
}
/** Read the Rust capability, not an environment flag or service availability. */
async function cloudBuildEnabled() {
  return hasShell() && (await appInfo())?.cloudEnabled === true
}
/** Keep account IPC unreachable in ordinary builds, including browser previews. */
async function cloudCommand(command: string, args?: Record<string, unknown>) {
  if (!(await cloudBuildEnabled())) return emptyCloudSnapshot
  return args ? invoke<CloudSnapshot>(command, args) : invoke<CloudSnapshot>(command)
}
/** Keep saved-request IPC gated separately from simulated operation progress. */
async function cloudSyncCommand(
  command: string,
  args: Record<string, unknown>,
): Promise<SyncResult> {
  if (!(await cloudBuildEnabled())) return { status: "unavailable" }
  return invoke<SyncResult>(command, args)
}
/** Invoke Cloud commands only when Rust AppInfo confirms the non-default feature. */
export const nativeCloudPort: CloudPort = {
  read: () => cloudCommand("cloud_status"),
  watch: async (listener) =>
    (await cloudBuildEnabled())
      ? listen<CloudSnapshot>("cloud:changed", (event) => listener(event.payload))
      : () => {},
  begin: () => cloudCommand("cloud_begin"),
  cancel: () => cloudCommand("cloud_cancel"),
  save: (value, binding, foldersEdited = true) =>
    cloudCommand("cloud_save_draft", { value, binding, foldersEdited }),
  signOut: () => cloudCommand("cloud_sign_out"),
  resetLocal: () => cloudCommand("cloud_reset_local"),
  pick: async () => {
    if (!(await cloudBuildEnabled())) return []
    const selected = await open({
      directory: true,
      multiple: true,
      title: "Choose Claude projects/<project> source directories",
    })
    return selected === null ? [] : Array.isArray(selected) ? selected : [selected]
  },
  agent: async () => {
    if (!(await cloudBuildEnabled())) return "unavailable"
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
  prepareSync: async (folders, requestId, consent) => {
    await cloudSyncCommand("cloud_prepare_sync", { folders, requestId, consent })
  },
  start: (_folders, requestId) => cloudSyncCommand("cloud_start_sync", { requestId }),
  reconcile: (requestId) => cloudSyncCommand("cloud_reconcile_sync", { requestId }),
  cancelSync: (requestId) => cloudSyncCommand("cloud_cancel_sync", { requestId }),
}
