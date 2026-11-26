/** Feature-gated desktop Cloud IPC. No credential or source content bytes cross
 * this renderer boundary; saved requests are not capture or upload acceptance.
 */
import { invoke } from "@tauri-apps/api/core"
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
export type CloudSourceKind =
  | "claude_jsonl"
  | "opencode_sqlite_v2"
  | "codex_jsonl"
  | "pi_jsonl"
  | "cursor_cli_agent_jsonl"
  | "cursor_ide_composer"
/** Renderer display data from desktop project/CWD evidence, not a provider source.
 * Desktop code owns grouping, path identity, distinct-session counts, and worktree
 * membership. Include all current path members, including external worktrees and
 * directories recorded after a session changes CWD. Never derive members from
 * file paths in tool calls. This view model is not a desktop command DTO.
 */
export interface CloudProjectGroup {
  id: string
  /** An older group ID can preserve an opt-out, never an approval. */
  legacyOptOutId?: string
  name: string
  path: string
  memberPaths: readonly string[]
  sessionCount: number | null
  worktreeCount: number
  /** Approval covers checkouts of the same Git repository. */
  repositoryWide?: boolean
  /** False when Desktop could not verify the full set of worktree paths. */
  groupingComplete?: boolean
  /** This row was outside the refreshed page chain. It cannot add permission. */
  discoveryStale?: boolean
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
/** One provider directory or database used for capture discovery. */
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
const cloudSourceKind = (source: CloudFolder): CloudSourceKind =>
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
  legacyOptOutId?: string
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
  /** Desktop verified a Git origin for this stable group ID. */
  repositoryWide?: boolean
  /** Renderer-only marker for cached rows outside the current page chain. */
  discoveryStale?: boolean
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
  /** Desktop finished some folder lookups and deferred the rest on this page. */
  retrySamePage?: boolean
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
    const projectKey = (project: CloudProjectSelection) =>
      group.repositoryWide
        ? JSON.stringify([project.sourceKind, project.sourcePath])
        : JSON.stringify(project)
    const projects = new Map(existing.projects.map((project) => [projectKey(project), project]))
    for (const project of group.projects) {
      const key = projectKey(project)
      if (!projects.has(key)) projects.set(key, project)
    }
    groups.set(group.id, {
      ...group,
      ...(group.repositoryWide && { path: existing.path, label: existing.label }),
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
      if (Object.hasOwn(saved, group.id)) return [group.id, saved[group.id]!]
      if (group.legacyOptOutId && saved[group.legacyOptOutId] === false)
        return [group.id, false]
      return [group.id, defaultSelected]
    }),
  )
}
/** Consent for existing and future sessions in approved folders or Git repositories. */
export interface CloudOngoingConsent extends CloudDestinationBinding {
  version: "desktop-project-sync-v1"
  selectedSources: CloudSourceSelection[]
  projects: CloudProjectSelection[]
  /** Confirmed display choices, including exclusions. These do not grant path access. */
  projectChoices: CloudProjectChoices
  fullContent: true
  ongoing: true
  attachments: false
  /** Require the old workspace's confirmed removal revision before granting access. */
  moveFrom?: CloudFolderMoveSource
}

interface CloudFolderMoveSource {
  destinationId: string
  policyRevision: string
}

/** Edit one saved folder group without replacing other approved bindings. */
interface CloudProjectChange extends CloudDestinationBinding {
  groupId: string
  selected: boolean
  projects: CloudProjectSelection[]
  folderPaths: string[]
  expectedPolicyRevision: string
  /** Reject a move's removal if it would change any other saved binding. */
  expectedRemovals?: CloudProjectSelection[]
  /** Require the old workspace's confirmed removal revision before granting access. */
  moveFrom?: CloudFolderMoveSource
}
/** Save the exact reviewed paths and explicit new groups for one Team. */
interface CloudTeamConfirmation extends CloudDestinationBinding {
  expectedPolicyRevision: string
  folderPaths: string[]
  additions?: Record<string, CloudProjectSelection[]>
}
/** Desktop upload state. Transfer counters and queued work do not prove acceptance. */
export interface CloudOngoingStatus {
  /** Device admission is separate from saved folder approval and retry. */
  deviceUploadEnabled?: boolean
  /** Counters and transfer progress are current only after a complete storage read. */
  storageKnown: boolean
  /** Saved source changes still hold this policy, independent of device pause. */
  sourceConfirmationRequired?: boolean
  status:
    | "consent_required"
    | "ready"
    | "syncing"
    | "paused"
    | "offline"
    | "reauth_required"
    | "upload_unavailable"
    | "route_denied"
    | "fenced"
    | "interrupted"
    | "storage_unavailable"
  accountId: string | null
  destinationId: string | null
  consentVersion: string | null
  policyRevision?: string | null
  /** Saved Team policies and legacy cleanup state, even if uploads are unavailable. */
  teamStatuses?: CloudOngoingStatus[]
  workspaceName?: string | undefined
  /** Native group IDs for turning off saved folders outside discovery. */
  savedFolders?: { groupId: string; path: string }[] | undefined
  /** The verified current Team membership does not match this saved folder approval. */
  folderConfirmationRequired?: boolean
  /** Desktop found saved work that the user can retry for this exact destination. */
  retryAvailable?: boolean
  /** Saved uploads for which Desktop has scheduled automatic recovery or verification. */
  automaticRecoveryCount?: number
  /** Desktop has a backfill scan pending for this policy. */
  backfillInProgress?: boolean
  /** An explicit scan still has work that Desktop can advance automatically. */
  fullSyncInProgress?: boolean
  /** A background sweep is discovering or processing uploads for this policy. */
  backgroundSyncInProgress?: boolean
  /** Measured work for the current background sweep, separate from the last explicit scan. */
  backgroundProgress?: CloudOngoingStatus["uploadProgress"]
  /** Cloud acceptance for sources found by the last explicit backfill for this policy.
   * The total is null until the scan finishes, or if discovery could not count every source.
   */
  uploadProgress?: {
    uploaded: number
    total: number | null
    /** Sessions found so far, not a final count. */
    found?: number
    /** Sessions attributed to an exact approved folder group during this scan. */
    folderFound?: Readonly<Record<string, number>>
    blocked?: number
    unsupported?: number
    scanIncomplete?: boolean
    /** Required compressed bytes across this scan, retained after acceptance.
     * Matching receipts from before the scan add no new bytes. The total stays
     * null until discovery and all required plans are known. Sent bytes follow
     * current receipts and can decrease if provisional parts expire.
     */
    transfer?: { sentBytes: number; totalBytes: number | null }
  }
  /** Confirmed compressed bytes for one queued upload, not the total for all folders. */
  uploadTransfer?: { sentBytes: number; totalBytes: number }
  /** Local recovery and remote warning are independent of saved upload consent. */
  localResetAvailable?: boolean
  remoteRevocationUnconfirmed?: boolean
  paused: boolean
  selectedSources: CloudSourceSelection[]
  projects: CloudProjectSelection[]
  /** Exact folder bindings without repository display anchors. Older replies use projects. */
  exactProjects?: CloudProjectSelection[]
  /** Confirmed repository group IDs. Project paths for these rules are display anchors. */
  repositoryGroupIds?: string[]
  /** Saved provider bindings for each repository group. */
  repositoryFolders?: { groupId: string; projects: CloudProjectSelection[] }[]
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
  confirmTeamFolders?(confirmation: CloudTeamConfirmation): Promise<CloudOngoingStatus>
  setProject(change: CloudProjectChange): Promise<CloudOngoingStatus>
  setEnabled(enabled: boolean, binding?: CloudDestinationBinding): Promise<CloudOngoingStatus>
  /** Retry only the named destination and the policy revision shown at confirmation. */
  retry(
    binding: CloudDestinationBinding,
    expectedPolicyRevision: string,
  ): Promise<CloudOngoingStatus>
  /** Scan only folders saved for the named destination and policy revision. */
  backfill?(
    binding: CloudDestinationBinding,
    expectedPolicyRevision: string,
  ): Promise<CloudOngoingStatus>
}
/** Desktop account state, not a build gate. authEnabled requires valid service
 * configuration. captureEnabled permits explicit desktop capture on this platform;
 * syncEnabled reports a saved, unpaused ongoing policy, not upload acceptance.
 */
export interface CloudSnapshot {
  authEnabled: boolean
  /** A configured system-browser dashboard can open without device approval. */
  dashboardAvailable?: boolean
  sessionReadsAllowed?: boolean
  sessionReadUpdateAvailable?: boolean
  captureUpdateAvailable?: boolean
  accessUpdate?: "waiting" | "denied" | "failed" | null
  grantId?: string | null
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
  draft: CloudDraft
}
/** Additional actions a signed-in device can request in the browser. */
export type CloudAccessUpdate = "session_reads" | "capture"
/** Cleanup state for a legacy Team connection. Upload failure alone does not allow reset. */
export interface CloudWorkspaceRecovery {
  accountId: string
  workspaceId: string
  workspaceName?: string | undefined
  localResetAvailable?: boolean | undefined
  remoteRevocationUnconfirmed?: boolean | undefined
}
/** State of one folder confirmation. Transfer status comes from CloudOngoingStatus. */
export type SyncResult = { status: "unavailable" | "idle" | "starting" | "ambiguous" }

/**
 * Desktop commands for setup and Settings.
 * Attach watch before read: commands, including reads, can emit state before
 * their reply. Implementations keep secrets in desktop code and return safe errors.
 * Saved requests survive observer teardown and account cleanup.
 */
export interface CloudPort {
  /** Current desktop home for display only. Missing proof keeps raw paths visible. */
  displayHome?(): Promise<string | null>
  /** Open the configured dashboard root without sending device proof. */
  openDashboard?(): Promise<void>
  /** Open the browser setup route on the configured resource origin without device proof. */
  openSetup?(): Promise<void>
  /** List verified destinations for this account. Listing does not grant folder consent. */
  workspaces?(binding: CloudDestinationBinding): Promise<CloudWorkspace[] | null>
  /** Clear an old Team connection locally. This does not close its remote grant. */
  resetWorkspace?(binding: CloudDestinationBinding, workspaceId: string): Promise<CloudStatus>
  /** Observe cleanup for old Team connections only. */
  watchWorkspace?(listener: (result: CloudWorkspaceRecovery) => void): Promise<() => void>
  /** Read project folders and manage ongoing upload consent. */
  ongoing: CloudOngoingPort
  /** Refresh desktop authority. A state event can precede this reply. */
  read(): Promise<CloudSnapshot>
  /** Observe safe snapshots until the returned stop function is called. */
  watch(listener: (value: CloudSnapshot) => void): Promise<() => void>
  /** Open browser approval, or the information page when service config is absent. */
  begin(accessUpdate?: CloudAccessUpdate): Promise<CloudSnapshot>
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
  /** Observe CLI and skill setup independently of Cloud and vendor integrations.
   * Attach the listener before reading; neither operation installs or repairs tools.
   */
  localTools: {
    read(): Promise<LocalToolsStatus>
    watch(listener: (status: LocalToolsStatus) => void): Promise<() => void>
  }
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
function cloudOngoingCommand<T = CloudOngoingStatus>(
  command: string,
  args: Record<string, unknown>,
): Promise<T> {
  if (hasShell() && verifiedCloudBuild) return invoke<T>(command, args)
  return cloudBuildEnabled().then((enabled) => {
    if (!enabled) throw new Error("Cloud is unavailable in this build.")
    return invoke<T>(command, args)
  })
}
/** Keep account IPC unreachable in ordinary builds, including browser previews. */
async function cloudCommand(command: string, args?: Record<string, unknown>) {
  if (!(await cloudBuildEnabled())) return emptyCloudSnapshot
  return args ? invoke<CloudSnapshot>(command, args) : invoke<CloudSnapshot>(command)
}
/** Invoke Cloud commands only when Rust AppInfo confirms the non-default feature. */
export const desktopCloudPort: CloudPort = {
  displayHome: async () => {
    if (!hasShell()) return null
    const info = await appInfo()
    return info?.homeDirectory ?? null
  },
  openDashboard: async () => {
    if (!(await cloudBuildEnabled())) throw new Error("Cloud is unavailable.")
    await invoke("cloud_open_dashboard")
  },
  openSetup: async () => {
    if (!(await cloudBuildEnabled())) throw new Error("Cloud is unavailable.")
    await invoke("cloud_open_dashboard", { setup: true })
  },
  workspaces: async (binding) =>
    (await cloudBuildEnabled())
      ? invoke<CloudWorkspace[] | null>("cloud_workspaces", { binding })
      : null,
  resetWorkspace: async (binding, workspaceId) => {
    if (!(await cloudBuildEnabled())) return "unavailable"
    return invoke<CloudStatus>("cloud_reset_workspace", { binding, workspaceId })
  },
  watchWorkspace: async (listener) =>
    (await cloudBuildEnabled())
      ? listen<CloudWorkspaceRecovery>("cloud:workspace-changed", (event) =>
          listener(event.payload),
        )
      : () => {},
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
    confirmTeamFolders: (confirmation) =>
      cloudOngoingCommand("cloud_reapprove_team_sync", { confirmation }),
    setProject: (change) => cloudOngoingCommand("cloud_set_sync_project", { change }),
    setEnabled: (enabled, binding) =>
      cloudOngoingCommand("cloud_set_sync_enabled", { enabled, ...(binding && { binding }) }),
    retry: (binding, expectedPolicyRevision) =>
      cloudOngoingCommand("cloud_retry_sync", { binding, expectedPolicyRevision }),
    backfill: (binding, expectedPolicyRevision) =>
      cloudOngoingCommand("cloud_backfill_sync", { binding, expectedPolicyRevision }),
  },
  read: () => cloudCommand("cloud_status"),
  watch: async (listener) =>
    (await cloudBuildEnabled())
      ? listen<CloudSnapshot>("cloud:changed", (event) => listener(event.payload))
      : () => {},
  begin: (accessUpdate) =>
    cloudCommand("cloud_begin", accessUpdate ? { accessUpdate } : undefined),
  cancel: () => cloudCommand("cloud_cancel"),
  save: (value, binding, foldersEdited = true) =>
    cloudCommand("cloud_save_draft", { value, binding, foldersEdited }),
  signOut: () => cloudCommand("cloud_sign_out"),
  resetLocal: () => cloudCommand("cloud_reset_local"),
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
}
