/** Feature-gated desktop Cloud IPC. No credential or source content bytes cross
 * this renderer boundary; saved requests are not capture or upload acceptance.
 */
import { invoke } from "@tauri-apps/api/core"
import { open } from "@tauri-apps/plugin-dialog"
import { appInfo, hasShell } from "./ipc"
import { listen } from "./tauriEvents"
import type { LocalToolsStatus } from "./ipcPayloads"

/** Closed desktop auth states. A signed-in account still needs content consent. */
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
/** Original provider source formats, not installed runtime versions. */
export type CloudSourceKind = "claude_jsonl" | "opencode_sqlite_v2"
/** Renderer display data from desktop project/CWD evidence, not a provider source.
 * Desktop code owns grouping, path identity, distinct-session counts, and worktree
 * membership. Include all current path members, including external worktrees and
 * directories recorded after a session changes CWD. Never derive members from
 * file paths in tool calls. This view model is not a desktop command DTO.
 */
export interface CloudProjectGroup {
  id: string
  name: string
  path: string
  memberPaths: readonly string[]
  sessionCount: number | null
  worktreeCount: number
  captureSupported?: boolean
  /** Freeze this group's exact paths and source bindings, including when unchecked. */
  confirmationKey?: string
}
/** Explicit local row choices. An absent choice shows a new group as selected;
 * neither that default nor a legacy upload preference grants content consent.
 */
export type CloudProjectChoices = Readonly<Record<string, boolean>>
/** Shorten the exact home prefix for display. Keep raw paths for selections and consent. */
export function cloudDisplayPath(
  path: string,
  homeDirectory: string | null | undefined,
): string {
  const home = homeDirectory?.replace(/[\\/]+$/, "")
  if (!home) return path
  if (path === home) return "~"
  return path.startsWith(`${home}/`) || path.startsWith(`${home}\\`)
    ? `~${path.slice(home.length)}`
    : path
}
/** Freeze only the selected groups' current path members for explicit approval.
 * Do not expand directories, discover worktrees, persist defaults, or infer access.
 * Desktop code must validate path boundaries and consent before it starts work.
 */
export function selectedCloudProjectPaths(
  groups: readonly CloudProjectGroup[],
  choices: CloudProjectChoices,
): string[] {
  return [
    ...new Set(
      groups.filter((group) => choices[group.id] ?? true).flatMap((group) => group.memberPaths),
    ),
  ].sort()
}
/** Compare the exact selected members, not display counts or row ordering.
 * A new external worktree or a new recorded CWD changes this confirmation key.
 */
export function cloudProjectSelectionKey(
  groups: readonly CloudProjectGroup[],
  choices: CloudProjectChoices,
): string {
  return JSON.stringify(selectedCloudProjectPaths(groups, choices))
}
/** One exact source approved in the picker: a project directory or database file. */
export interface CloudSourceSelection {
  sourceKind: CloudSourceKind
  path: string
}
/** Persisted source preference. Missing kind keeps the legacy Claude-only meaning. */
export interface CloudFolder {
  sessionCount?: number | null
  sessionCountIsPartial?: boolean
  path: string
  selected: boolean
  sourceKind?: CloudSourceKind
}
/** Normalize only an absent legacy kind; desktop code rejects unknown kinds. */
export const cloudSourceKind = (source: CloudFolder): CloudSourceKind =>
  source.sourceKind ?? "claude_jsonl"
/** Compare provider and path together without treating display counts as consent. */
export const cloudSourceKey = (source: CloudFolder) =>
  JSON.stringify([cloudSourceKind(source), source.path])
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
/** Desktop recorded project/CWD selection for one supported source. */
export interface CloudProjectSelection {
  sourceKind: CloudSourceKind
  sourcePath: string
  projectPath: string
}
/** Desktop project group. Other agents supply suggestions only, not upload proof. */
export interface DesktopCloudProjectGroup {
  id: string
  environment: string
  path: string
  label: string
  folderPaths: string[]
  worktreeCount: number
  agents: string[]
  sessionCount: number
  captureSessionCount: number
  captureSupported: boolean
  groupingComplete: boolean
  projects: CloudProjectSelection[]
}
/** Desktop keyset position. Keep execution environments separate. */
export interface CloudProjectCursor {
  environment: string
  cwd: string
}
/** One desktop metadata page, never authority to upload source content. */
interface CloudProjectDiscovery {
  groups: DesktopCloudProjectGroup[]
  sourceRoots: CloudSourceSelection[]
  nextCursor: CloudProjectCursor | null
  partial: boolean
}
/** Merge desktop pages by stable group ID without making new membership claims.
 * Cached counts cover separate CWD pages; capture counts can repeat across pages.
 */
export function mergeCloudProjectGroups(
  previous: readonly DesktopCloudProjectGroup[],
  next: readonly DesktopCloudProjectGroup[],
): DesktopCloudProjectGroup[] {
  const groups = new Map(previous.map((group) => [group.id, group]))
  for (const group of next) {
    const existing = groups.get(group.id)
    if (!existing) {
      groups.set(group.id, group)
      continue
    }
    const projects = new Map(
      [...existing.projects, ...group.projects].map((project) => [
        JSON.stringify(project),
        project,
      ]),
    )
    groups.set(group.id, {
      ...group,
      folderPaths: [...new Set([...existing.folderPaths, ...group.folderPaths])].sort(),
      agents: [...new Set([...existing.agents, ...group.agents])].sort(),
      sessionCount: existing.sessionCount + group.sessionCount,
      captureSessionCount: Math.max(existing.captureSessionCount, group.captureSessionCount),
      captureSupported: existing.captureSupported || group.captureSupported,
      groupingComplete: existing.groupingComplete && group.groupingComplete,
      projects: [...projects.values()],
    })
  }
  return [...groups.values()]
}
/** Keep unconfirmed local edits first, then the account's confirmed group choices.
 * New groups default to ON in memory only. Provider-file preferences and path
 * approvals do not substitute for this separate UI choice map.
 */
export function cloudProjectChoices(
  groups: readonly CloudProjectGroup[],
  saved: CloudProjectChoices,
  current: CloudProjectChoices,
  defaultSelected = true,
): CloudProjectChoices {
  return Object.fromEntries(
    groups.map((group) => {
      if (Object.hasOwn(current, group.id)) return [group.id, current[group.id]!]
      return [group.id, Object.hasOwn(saved, group.id) ? saved[group.id]! : defaultSelected]
    }),
  )
}
/** Separate consent for existing and future sessions under exact source/path bindings. */
export interface CloudOngoingConsent extends CloudDestinationBinding {
  version: "desktop-project-sync-v1"
  selectedSources: CloudSourceSelection[]
  projects: CloudProjectSelection[]
  /** Confirmed display choices, including exclusions. These do not grant path access. */
  projectChoices: CloudProjectChoices
  fullContent: true
  ongoing: true
  attachments: false
}
/** Edit one saved folder group without replacing other approved bindings. */
interface CloudProjectChange extends CloudDestinationBinding {
  groupId: string
  selected: boolean
  projects: CloudProjectSelection[]
  folderPaths: string[]
  expectedPolicyRevision: string
}
/** Desktop upload state. Transfer counters and queued work do not prove acceptance. */
export interface CloudOngoingStatus {
  /** Counters and transfer progress are current only after a complete storage read. */
  storageKnown: boolean
  status:
    | "consent_required"
    | "ready"
    | "syncing"
    | "paused"
    | "offline"
    | "reauth_required"
    | "upload_unavailable"
    | "fenced"
    | "interrupted"
    | "storage_unavailable"
  accountId: string | null
  destinationId: string | null
  consentVersion: string | null
  policyRevision?: string | null
  paused: boolean
  selectedSources: CloudSourceSelection[]
  projects: CloudProjectSelection[]
  /** Confirmed account-bound choices. Older status replies can omit this metadata;
   * an existing policy without it requires review instead of selecting unknown groups.
   */
  projectChoices?: CloudProjectChoices
  queuedCount: number
  interruptedCount: number
  /** Included in interruptedCount, but upload retry cannot recover these jobs.
   * Older replies omit this count; treat it as zero.
   */
  recoveryRequiredCount?: number
  fencedCount: number
  acceptedCount: number
  suppressedCount: number
  progress: {
    negotiated: boolean
    totalCompressedBytes: number
    requiredTransferBytes: number
    reusedBytes: number
    confirmedUploadBytes: number
  }
}
/** Desktop-owned ongoing work survives renderer teardown. Attach status observation
 * before reads. Enable and confirmed folder additions grant content permission.
 */
export interface CloudOngoingPort {
  groups(
    binding: CloudDestinationBinding,
    after: CloudProjectCursor | null,
  ): Promise<CloudProjectDiscovery>
  status(): Promise<CloudOngoingStatus | null>
  watch(listener: (value: CloudOngoingStatus) => void): Promise<() => void>
  enable(consent: CloudOngoingConsent): Promise<CloudOngoingStatus>
  setProject(change: CloudProjectChange): Promise<CloudOngoingStatus>
  pause(binding: CloudDestinationBinding): Promise<CloudOngoingStatus>
  retry(binding: CloudDestinationBinding): Promise<CloudOngoingStatus>
}
/** One explicit confirmation for a saved request. V1 stays Claude-only. V2
 * includes proved session-owned database blobs, never external files or URLs.
 */
export type CaptureConsent = CloudDestinationBinding & {
  fullContent: true
  attachments: false
} & (
    | { version: "native-full-content-v1"; selectedRoots: string[] }
    | { version: "native-full-content-v2"; selectedSources: CloudSourceSelection[] }
  )
/** Desktop account state, not a build gate. authEnabled requires valid service
 * configuration. captureEnabled permits explicit desktop capture on this platform;
 * syncEnabled reports a saved, unpaused ongoing policy, not upload acceptance.
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
/** Legacy saved-request states. Ongoing uploads use CloudOngoingStatus. */
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
 * Desktop commands for setup and Settings.
 * Attach watch before read: commands, including reads, can emit state before
 * their reply. Implementations keep secrets in desktop code and return safe errors.
 * Saved requests survive observer teardown and account cleanup.
 */
export interface CloudPort {
  /** Current desktop home for display only. Missing proof keeps raw paths visible. */
  displayHome?(): Promise<string | null>
  /** Desktop grouped project path. Legacy saved-request fixtures may omit it. */
  ongoing?: CloudOngoingPort | undefined
  /** Refresh desktop authority. A state event can precede this reply. */
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
  /** Ask for exact sources of one provider. Default hints never authorize reads. */
  pick(sourceKind?: CloudSourceKind): Promise<string[]>
  /** Observe CLI and skill setup independently of Cloud and vendor integrations.
   * Attach the listener before reading; neither operation installs or repairs tools.
   */
  localTools: {
    read(): Promise<LocalToolsStatus>
    watch(listener: (status: LocalToolsStatus) => void): Promise<() => void>
  }
  /** Save exact account/root consent before listing. Reuse this ID after uncertainty. */
  prepareSync(folders: CloudFolder[], requestId: string, consent: CaptureConsent): Promise<void>
  /** List a legacy saved selection only; this command does not capture or upload bytes. */
  start(folders: CloudFolder[], requestId: string, consent: CaptureConsent): Promise<SyncResult>
  /** Read saved state only. Reconciliation must not rescan or submit an upload. */
  reconcile(requestId: string): Promise<SyncResult>
  /** Mark the original account's saved selection cancelled without deleting bytes. */
  cancelSync(requestId: string): Promise<SyncResult>
}
/** Safe busy rejection means desktop work did not start; never surface other raw errors. */
export const CLOUD_BUSY_MESSAGE = "Cloud is busy. Try again."
/** Closed renderer default for a browser or a desktop build without Cloud. */
export const emptyCloudSnapshot: CloudSnapshot = {
  authEnabled: false,
  syncEnabled: false,
  captureEnabled: false,
  status: "idle",
  identity: null,
  draft: { screen: "handoff", folders: [] },
}
/** Last verified binary capability, never account authority or content consent. */
let verifiedCloudBuild = false
/** Read the Rust capability, not an environment flag or service availability. */
async function cloudBuildEnabled() {
  verifiedCloudBuild = hasShell() && (await appInfo())?.cloudEnabled === true
  return verifiedCloudBuild
}
/** Submit actions immediately after a verified desktop read. The binary capability
 * cannot change during a process. Do not put another AppInfo IPC round trip
 * between Start and desktop ownership, because Done can close the renderer.
 */
function cloudOngoingCommand(
  command: string,
  args: Record<string, unknown>,
): Promise<CloudOngoingStatus> {
  if (hasShell() && verifiedCloudBuild) return invoke<CloudOngoingStatus>(command, args)
  return cloudBuildEnabled().then((enabled) => {
    if (!enabled) throw new Error("Cloud is unavailable in this build.")
    return invoke<CloudOngoingStatus>(command, args)
  })
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
export const desktopCloudPort: CloudPort = {
  displayHome: async () => {
    if (!hasShell()) return null
    const info = await appInfo()
    return info?.homeDirectory ?? null
  },
  ongoing: {
    groups: async (binding, after) => {
      if (!(await cloudBuildEnabled()))
        return { groups: [], sourceRoots: [], nextCursor: null, partial: false }
      return invoke<CloudProjectDiscovery>("cloud_project_groups", { binding, after })
    },
    status: async () =>
      (await cloudBuildEnabled()) ? invoke<CloudOngoingStatus>("cloud_sync_status") : null,
    watch: async (listener) =>
      (await cloudBuildEnabled())
        ? listen<CloudOngoingStatus>("cloud:sync-changed", (event) => listener(event.payload))
        : () => {},
    enable: (consent) => cloudOngoingCommand("cloud_enable_sync", { consent }),
    setProject: (change) => cloudOngoingCommand("cloud_set_sync_project", { change }),
    pause: (binding) => cloudOngoingCommand("cloud_pause_sync", { binding }),
    retry: (binding) => cloudOngoingCommand("cloud_retry_sync", { binding }),
  },
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
  pick: async (sourceKind = "claude_jsonl") => {
    if (!(await cloudBuildEnabled())) return []
    const database = sourceKind === "opencode_sqlite_v2"
    const defaultPath = database
      ? await invoke<string | null>("cloud_source_picker_directory")
      : null
    const selected = await open({
      directory: !database,
      multiple: true,
      title: database
        ? "Choose OpenCode V2 databases"
        : "Choose Claude projects/<project> source directories",
      ...(defaultPath ? { defaultPath } : {}),
    })
    return selected === null ? [] : Array.isArray(selected) ? selected : [selected]
  },
  localTools: {
    read: async () =>
      hasShell()
        ? invoke<LocalToolsStatus>("get_local_tools_status")
        : {
            revision: 0,
            phase: "unavailable",
            cliInstalled: false,
            cliTested: false,
            skillInstalled: false,
            reason: null,
          },
    watch: async (listener) =>
      hasShell()
        ? listen<LocalToolsStatus>("local-tools:changed", (event) => listener(event.payload))
        : () => {},
  },
  prepareSync: async (folders, requestId, consent) => {
    await cloudSyncCommand("cloud_prepare_sync", { folders, requestId, consent })
  },
  start: (_folders, requestId) => cloudSyncCommand("cloud_start_sync", { requestId }),
  reconcile: (requestId) => cloudSyncCommand("cloud_reconcile_sync", { requestId }),
  cancelSync: (requestId) => cloudSyncCommand("cloud_cancel_sync", { requestId }),
}
