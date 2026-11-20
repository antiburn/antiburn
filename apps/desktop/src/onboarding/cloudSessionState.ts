import {
  CLOUD_BUSY_MESSAGE,
  cloudProjectSelectionKey,
  cloudSourceKey,
  cloudSourceKind,
  type CaptureConsent,
  type CloudDestinationBinding,
  type CloudFolder,
  type CloudOngoingStatus,
  type CloudProjectChoices,
  type CloudProjectGroup,
  type CloudProjectSelection,
  type CloudScreen,
  type CloudSnapshot,
  type CloudSourceSelection,
  type CloudWorkspace,
  type CloudWorkspaceRecovery,
  type DesktopCloudProjectGroup,
  type SyncResult,
} from "../lib/cloud"
import { personalCloudDestination } from "../lib/cloudDestination"
import type { LocalToolsStatus } from "../lib/ipcPayloads"
import { canRetryTeamUploads } from "./cloudTeamRetry"

/** One surface's view state. Start or a confirmed Settings addition grants content consent. */
export interface CloudViewSnapshot {
  /** Fence actions rendered before an account, source, or folder selection change. */
  contextRevision: number
  /** Current home is display-only. Raw source and project paths retain authority. */
  homeDirectory: string | null
  cloud: CloudSnapshot
  screen: CloudScreen
  sync: SyncResult
  /** CLI and skill evidence is independent of Cloud authority and vendor hooks. */
  localTools: LocalToolsStatus | null
  /** A failed status read or subscription cannot establish current tool readiness. */
  localToolsError: boolean
  busy: boolean
  error: string | null
  /** Settings upload changes report errors with the folder controls, not sign-in. */
  uploadActionError?: string | null
  loaded: boolean
  /** Retry the saved request's listing even after a transient warning clears. */
  sourceListingNeedsRetry: boolean
  /** Desktop project discovery with local defaults. No selection write happens on load. */
  projects?: {
    groups: CloudProjectGroup[]
    choices: CloudProjectChoices
    workspaces?: readonly CloudWorkspace[] | undefined
    /** Explicit destination choices that have not been saved. */
    targets?: Readonly<Record<string, string>> | undefined
    loading: boolean
    loaded: boolean
    partial: boolean
    hasMore: boolean
    needsChoiceReview: boolean
    error: string | null
    selectionKey: string
  } | null
  /** Desktop-owned ongoing policy and transfer counters, separate from local agents. */
  ongoing?: CloudOngoingStatus | null
  /** Device upload admission, independent of account and folder choice. */
  deviceUploadEnabled?: boolean
  lastTeamSync?: CloudOngoingStatus | null
  teamStatuses?: Readonly<Record<string, CloudOngoingStatus>> | undefined
  workspaceConnections?: Readonly<Record<string, CloudWorkspaceRecovery>>
}

/** Compare consent-relevant roots without treating display-count changes as edits. */
export function selectedFoldersKey(value: CloudSnapshot) {
  return JSON.stringify(
    value.draft.folders
      .filter((folder) => folder.selected)
      .map(cloudSourceKey)
      .sort(),
  )
}

/** Bind a rendered Start action to authority and exact selection, not display counts. */
export function confirmationContextKey(value: CloudViewSnapshot) {
  return JSON.stringify([
    value.cloud.identity?.id,
    value.cloud.destination?.id,
    value.cloud.destination?.kind,
    value.cloud.status,
    value.cloud.captureEnabled,
    value.cloud.setupError,
    selectedFoldersKey(value.cloud),
    value.projects?.selectionKey,
  ])
}

/** Use only the backend's exact saved paths when it reports stale Team consent. */
export function teamFoldersToConfirm(value: CloudViewSnapshot, destinationId: string) {
  const policy = value.teamStatuses?.[destinationId]
  if (
    value.cloud.status !== "signed_in" ||
    !value.cloud.captureEnabled ||
    value.cloud.setupError ||
    !policy?.folderConfirmationRequired ||
    !policy.savedFolders?.length ||
    !policy.policyRevision ||
    policy.accountId !== value.cloud.identity?.id ||
    policy.destinationId !== destinationId ||
    (value.projects?.workspaces &&
      !value.projects.workspaces.some(
        ({ id, kind }) => id === destinationId && kind === "team",
      ))
  )
    return null
  return { revision: policy.policyRevision, paths: policy.savedFolders.map(({ path }) => path) }
}

const messages = {
  idle: "",
  opening: "Opening your browser…",
  waiting: "Waiting for browser…",
  returned: "Welcome back. Cloud is coming soon. Continue using Antiburn locally.",
  signed_in: "Your account is verified.",
  signed_out: "You’re signed out on this device.",
  cancelled: "Browser handoff cancelled. You can try again or continue locally.",
  expired: "This browser link expired. Try again to open a new link.",
  browser_failed: "Your browser could not open. Try again or continue locally.",
  offline: "Cloud could not connect. Check your connection and try again.",
  rejected: "Cloud could not verify this connection. Sign in again to continue.",
  invalid_response: "Cloud returned an unexpected response. Try again later.",
  credential_failed:
    "Antiburn could not open your saved sign-in. Unlock your device's password storage, or sign out and sign in again.",
  unavailable: "Cloud is not available in this build. You can continue locally.",
  reauth_required: "Sign in again to use Antiburn Cloud.",
  revocation_unconfirmed:
    "Signed out on this device. Cloud has not confirmed that the connection is closed. Open Cloud settings to finish.",
} as const

/** Map closed desktop states to safe copy without exposing provider errors. */
export const cloudStatusMessage = (status: CloudSnapshot["status"]) => messages[status]

/** Distinguish fresh setup from an unreadable saved-upload state. */
export const cloudUploadStatusUnknown = (value: CloudOngoingStatus | null | undefined) =>
  !!value && !value.storageKnown && value.status !== "consent_required"

/** Retry only when desktop reports a bound job it can retry now. */
export const canRetryPersonalUploads = (value: CloudViewSnapshot) =>
  value.deviceUploadEnabled === true &&
  value.ongoing?.retryAvailable === true &&
  !value.ongoing.paused &&
  !value.ongoing.sourceConfirmationRequired

/** Backfill uses only the current, saved folder rules for one verified workspace. */
export function canBackfillUploads(value: CloudViewSnapshot, teamId?: string) {
  const destination = value.cloud.destination
  const policy = teamId ? value.teamStatuses?.[teamId] : value.ongoing
  return (
    value.cloud.status === "signed_in" &&
    value.cloud.captureEnabled === true &&
    !value.cloud.setupError &&
    value.deviceUploadEnabled === true &&
    !!policy?.storageKnown &&
    !!policy.policyRevision &&
    policy.accountId === value.cloud.identity?.id &&
    policy.destinationId === (teamId ?? destination?.id) &&
    policy.projects.length > 0 &&
    !policy.paused &&
    !policy.folderConfirmationRequired &&
    !policy.sourceConfirmationRequired &&
    policy.status !== "reauth_required" &&
    policy.status !== "storage_unavailable" &&
    (teamId
      ? !!value.projects?.workspaces?.some(({ id, kind }) => id === teamId && kind === "team")
      : destination?.kind === "personal")
  )
}

/** Select the saved policy for the requested workspace. */
export function uploadPolicy(value: CloudViewSnapshot, teamId?: string) {
  return teamId ? value.teamStatuses?.[teamId] : value.ongoing
}

/** Match retry to the displayed workspace and its current saved revision. */
export function retryTarget(
  displayed: CloudViewSnapshot,
  current: CloudViewSnapshot,
  anchor: CloudDestinationBinding | null,
  teamId?: string,
) {
  const policy = teamId === undefined ? displayed.ongoing : displayed.teamStatuses?.[teamId]
  const currentPolicy = teamId === undefined ? current.ongoing : current.teamStatuses?.[teamId]
  const revision = policy?.policyRevision
  if (!anchor || !revision || revision !== currentPolicy?.policyRevision) return null
  if (teamId !== undefined) {
    if (
      teamId === anchor.destinationId ||
      !canRetryTeamUploads(displayed, teamId) ||
      !canRetryTeamUploads(current, teamId)
    )
      return null
  } else if (
    policy.accountId !== anchor.accountId ||
    policy.destinationId !== anchor.destinationId ||
    !canRetryPersonalUploads(displayed) ||
    !canRetryPersonalUploads(current)
  )
    return null
  return { binding: { ...anchor, destinationId: teamId ?? anchor.destinationId }, revision }
}

/** Check that an action still owns the exact policy it was prepared for. */
export function matchesUploadPolicy(
  policy: ReturnType<typeof uploadPolicy>,
  binding: CloudDestinationBinding,
  revision: string,
) {
  return (
    policy?.policyRevision === revision &&
    policy.accountId === binding.accountId &&
    policy.destinationId === binding.destinationId
  )
}

/** Use only the current saved policy shown before the Backfill action. */
export function backfillTarget(
  displayed: CloudViewSnapshot,
  current: CloudViewSnapshot,
  anchor: CloudDestinationBinding | null,
  teamId?: string,
) {
  const revision = uploadPolicy(displayed, teamId)?.policyRevision
  if (!anchor || !revision || displayed.contextRevision !== current.contextRevision) return null
  const binding = { ...anchor, destinationId: teamId ?? anchor.destinationId }
  const policy = uploadPolicy(current, teamId)
  if (
    !binding.destinationId ||
    !matchesUploadPolicy(policy, binding, revision) ||
    !canBackfillUploads(displayed, teamId) ||
    !canBackfillUploads(current, teamId) ||
    policy?.backfillInProgress
  )
    return null
  return { binding, revision, team: teamId ? (policy ?? undefined) : undefined }
}

/** Missing storage evidence must not turn a cached state into a current Ready claim. */
export const cloudOngoingStatusMessage = (value: CloudOngoingStatus) => {
  if (cloudUploadStatusUnknown(value)) return "Saved upload status is unavailable"
  return {
    consent_required: "Cloud sync not started",
    ready: "Background sync enabled",
    syncing: "Sessions syncing",
    paused: "Cloud uploads paused",
    offline: "Cloud is offline",
    reauth_required: "Reconnect to Cloud",
    upload_unavailable: "Cloud upload unavailable",
    fenced: "Some uploads need attention",
    interrupted: "Some sessions could not be prepared",
    storage_unavailable: "Could not open saved uploads",
  }[value.status]
}

/** Accept only the exact nonsecret admission error, not arbitrary desktop/provider text. */
export function isCloudBusy(error: unknown) {
  return (
    error === CLOUD_BUSY_MESSAGE ||
    (error instanceof Error && error.message === CLOUD_BUSY_MESSAGE)
  )
}

/** Map upload errors to safe copy for the action that failed. */
export function uploadActionError(error: unknown, backfill: boolean, team: boolean) {
  const message = error instanceof Error ? error.message : error
  if (isCloudBusy(error)) return CLOUD_BUSY_MESSAGE
  if (message === "Cloud folder choices changed. Reload before retrying saved work.")
    return backfill
      ? "Cloud folder choices changed. Reopen Settings and try again."
      : "Cloud folder choices changed. Reopen Settings before retrying."
  if (backfill) return "Could not start backfill. Try again."
  return team
    ? "Could not retry saved Team uploads. Saved work is kept."
    : "Could not change Cloud uploads. Try again."
}

/** Account and source changes invalidate requests independently of view-only updates. */
export function cloudContextChange(previous: CloudSnapshot, next: CloudSnapshot) {
  const resetProjects =
    next.identity?.id !== previous.identity?.id ||
    next.destination?.id !== previous.destination?.id ||
    next.destination?.kind !== previous.destination?.kind ||
    !next.identity ||
    next.status !== "signed_in"
  return {
    resetProjects,
    contextChanged:
      resetProjects ||
      selectedFoldersKey(next) !== selectedFoldersKey(previous) ||
      !!next.setupError,
  }
}

/** Build the view update for a desktop account event. The session owns request IDs and timers. */
export function accountViewPatch(
  previous: CloudViewSnapshot,
  value: CloudSnapshot,
  options: {
    resetProjects: boolean
    contextChanged: boolean
    ongoing: boolean
    projectSyncStarted: boolean
    preparedRequestId: string | null
  },
): Partial<CloudViewSnapshot> {
  const { resetProjects, contextChanged, ongoing, projectSyncStarted, preparedRequestId } =
    options
  const retryInvalidated =
    contextChanged ||
    (value.pendingSync &&
      (value.pendingSync.requestId !== preparedRequestId ||
        value.pendingSync.result.status === "cancelled"))
  const screen = value.identity
    ? value.draft.screen === "handoff"
      ? "identity"
      : value.draft.screen
    : "handoff"
  return {
    cloud: value,
    ...(resetProjects
      ? {
          projects: null,
          ongoing: null,
          lastTeamSync: null,
          teamStatuses: {},
          workspaceConnections: {},
          uploadActionError: null,
        }
      : value.setupError && previous.projects
        ? { projects: { ...previous.projects, loading: false } }
        : {}),
    screen:
      value.identity && (ongoing ? projectSyncStarted || value.syncEnabled : value.pendingSync)
        ? "complete"
        : screen,
    loaded: true,
    ...(previous.error === CLOUD_BUSY_MESSAGE ? { error: null } : {}),
    ...(retryInvalidated ? { sourceListingNeedsRetry: false } : {}),
    ...(!ongoing && value.identity && value.pendingSync
      ? { sync: value.pendingSync.result }
      : contextChanged
        ? { sync: { status: "unavailable" as const } }
        : {}),
  }
}

/** Classify a desktop upload event without borrowing authority from another account. */
export function ongoingDestination(cloud: CloudSnapshot, status: CloudOngoingStatus) {
  if (!status.accountId && !status.destinationId) return "device"
  const { identity, destination, status: authStatus } = cloud
  const personal = personalCloudDestination(destination ?? null)
  if (!identity || !personal || authStatus !== "signed_in" || status.accountId !== identity.id)
    return null
  if (status.destinationId === personal.id) return "personal"
  return status.destinationId && status.consentVersion === "desktop-project-sync-v1"
    ? "team"
    : null
}

/** Bind source and project work to the signed-in Personal workspace. */
export function projectBinding(
  cloud: CloudSnapshot,
  allowSetupError: boolean,
): CloudDestinationBinding | null {
  const { identity, destination, status, setupError } = cloud
  const personal = personalCloudDestination(destination ?? null)
  return identity && personal && status === "signed_in" && (allowSetupError || !setupError)
    ? { accountId: identity.id, destinationId: personal.id }
    : null
}

/** Merge new workspace recovery facts without clearing a known display name. */
export function workspaceRecoveryPatch(
  snapshot: CloudViewSnapshot,
  recovery: CloudWorkspaceRecovery,
): Partial<CloudViewSnapshot> | null {
  if (
    recovery.localResetAvailable === undefined &&
    recovery.remoteRevocationUnconfirmed === undefined
  )
    return null
  const id = recovery.workspaceId
  return {
    workspaceConnections: {
      ...snapshot.workspaceConnections,
      [id]: {
        ...recovery,
        workspaceName:
          recovery.workspaceName ?? snapshot.workspaceConnections?.[id]?.workspaceName,
      },
    },
  }
}

/** Keep saved Team labels and known folders when an older event omits display metadata. */
export function teamStatusPatch(
  snapshot: CloudViewSnapshot,
  id: string,
  status: CloudOngoingStatus,
): Partial<CloudViewSnapshot> {
  const previous = snapshot.teamStatuses?.[id]
  const savedFolders =
    status.savedFolders ??
    previous?.savedFolders?.filter((folder) =>
      status.projects.some((project) => project.projectPath === folder.path),
    )
  const team = {
    ...status,
    workspaceName: status.workspaceName ?? previous?.workspaceName,
    savedFolders,
  }
  return {
    teamStatuses: { ...snapshot.teamStatuses, [id]: team },
    ...(snapshot.lastTeamSync?.destinationId === id ? { lastTeamSync: team } : {}),
  }
}

/** Require desktop project members and returned source roots, not content consent. */
export function supportedProjectSelections(
  group: DesktopCloudProjectGroup,
  sources: CloudSourceSelection[],
): CloudProjectSelection[] {
  return group.captureSupported
    ? group.projects.filter(
        (project) =>
          group.folderPaths.includes(project.projectPath) &&
          sources.some(
            (source) =>
              source.sourceKind === project.sourceKind && source.path === project.sourcePath,
          ),
      )
    : []
}

/** Use typed desktop selections only. Suggestions from other agents stay display-only. */
export function selectedProjects(
  groups: DesktopCloudProjectGroup[],
  sources: CloudSourceSelection[],
  choices: CloudProjectChoices,
): CloudProjectSelection[] {
  const selections = groups
    .filter((group) => choices[group.id] ?? true)
    .flatMap((group) => supportedProjectSelections(group, sources))
  return [
    ...new Map(selections.map((project) => [JSON.stringify(project), project])).values(),
  ].sort(
    (a, b) =>
      a.sourceKind.localeCompare(b.sourceKind) ||
      a.sourcePath.localeCompare(b.sourcePath) ||
      a.projectPath.localeCompare(b.projectPath),
  )
}

/** Include provider/source bindings in confirmation, not just visible folder names. */
export function projectSelectionKey(
  groups: CloudProjectGroup[],
  choices: CloudProjectChoices,
  selections: CloudProjectSelection[],
  targets: Record<string, string>,
  workspaces: readonly CloudWorkspace[] | undefined,
) {
  return JSON.stringify([
    cloudProjectSelectionKey(groups, choices),
    selections,
    Object.entries(targets).sort(),
    (workspaces ?? []).map((workspace) => [workspace.id, workspace.kind]).sort(),
  ])
}

/** Fence one Settings confirmation even while its group is excluded from Start. */
export function projectConfirmationKey(
  group: DesktopCloudProjectGroup,
  selections: CloudProjectSelection[],
) {
  return JSON.stringify([
    group.id,
    group.environment,
    group.path,
    group.groupingComplete,
    [...group.folderPaths].sort(),
    selections
      .map((project) =>
        JSON.stringify([project.sourceKind, project.sourcePath, project.projectPath]),
      )
      .sort(),
  ])
}

/** Keep exact saved Personal bindings eligible for source recovery. */
export function reviewableProjectSelections(
  selections: CloudProjectSelection[],
  policy: CloudOngoingStatus | null | undefined,
) {
  return selections.filter((project) =>
    policy?.projects.some(
      (saved) =>
        saved.sourceKind === project.sourceKind &&
        saved.sourcePath === project.sourcePath &&
        saved.projectPath === project.projectPath,
    ),
  )
}

/** Match saved grants by directory boundary, never by partial folder name. */
export function hasSavedProjectPermission(
  group: DesktopCloudProjectGroup | undefined,
  policy: Pick<CloudOngoingStatus, "projects"> | null | undefined,
  exact: boolean,
) {
  return (
    group?.environment === "native" &&
    !!policy?.projects.some((project) =>
      [group.path, ...group.folderPaths].some(
        (path) =>
          path === project.projectPath ||
          (!exact &&
            (path.startsWith(`${project.projectPath.replace(/\/+$/, "")}/`) ||
              project.projectPath.startsWith(`${path.replace(/\/+$/, "")}/`))),
      ),
    )
  )
}

/** Merge source roots without losing provider type or changing their order. */
export function mergeProjectSources(
  previous: CloudSourceSelection[],
  page: CloudSourceSelection[],
) {
  return [
    ...new Map(
      [...previous, ...page].map((source) => [
        JSON.stringify([source.sourceKind, source.path]),
        source,
      ]),
    ).values(),
  ]
}

/** Project display groups from the same selection used for confirmation. */
export function projectViewGroups(
  groups: DesktopCloudProjectGroup[],
  sources: CloudSourceSelection[],
): CloudProjectGroup[] {
  return groups.map((group) => {
    const selections = supportedProjectSelections(group, sources)
    return {
      id: group.id,
      name: group.label,
      path: group.path,
      memberPaths: group.folderPaths,
      worktreeCount: group.worktreeCount,
      sessionCount: Math.max(group.sessionCount, group.captureSessionCount),
      captureSupported: selections.length > 0,
      confirmationKey: projectConfirmationKey(group, selections),
    }
  })
}

/** Leave saved grants unchanged and require explicit choices for an existing policy. */
export function projectSyncChoices(
  snapshot: CloudViewSnapshot,
  targets: Record<string, string>,
  pendingOnly: boolean,
  hasSavedProject: (id: string) => boolean,
) {
  const explicitOnly =
    pendingOnly ||
    snapshot.ongoing?.consentVersion === "desktop-project-sync-v1" ||
    Object.keys(snapshot.teamStatuses ?? {}).length > 0
  const choicesForStart = { ...snapshot.projects?.choices }
  for (const group of snapshot.projects?.groups ?? []) {
    if (hasSavedProject(group.id) || (explicitOnly && !targets[group.id]))
      choicesForStart[group.id] = false
  }
  return { choicesForStart, explicitOnly }
}

/** Build the legacy request for the exact selected roots and destination. */
export function legacyCaptureConsent(
  binding: CloudDestinationBinding,
  selected: CloudFolder[],
): CaptureConsent {
  return {
    ...binding,
    fullContent: true,
    attachments: false,
    ...(selected.some((folder) => cloudSourceKind(folder) === "opencode_sqlite_v2")
      ? {
          version: "native-full-content-v2" as const,
          selectedSources: selected
            .map((folder) => ({ sourceKind: cloudSourceKind(folder), path: folder.path }))
            .sort(
              (a, b) =>
                a.sourceKind.localeCompare(b.sourceKind) || a.path.localeCompare(b.path),
            ),
        }
      : {
          version: "native-full-content-v1" as const,
          selectedRoots: selected.map((folder) => folder.path).sort(),
        }),
  }
}
