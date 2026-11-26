import {
  CLOUD_BUSY_MESSAGE,
  cloudProjectSelectionKey,
  cloudSourceKey,
  type CloudDestinationBinding,
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
  /** Pause is the only Cloud change allowed while the backfill request waits. */
  canPauseBackfill?: boolean
  error: string | null
  /** The current error came from an account read or watch, not a user action. */
  accountReadFailed?: boolean
  /** Settings upload changes report errors with the folder controls, not sign-in. */
  uploadActionError?: string | null
  loaded: boolean
  /** Desktop project discovery with local defaults. No selection write happens on load. */
  projects?: {
    groups: CloudProjectGroup[]
    choices: CloudProjectChoices
    workspaces?: readonly CloudWorkspace[] | undefined
    /** Explicit destination choices that have not been saved. */
    targets?: Readonly<Record<string, string>> | undefined
    loading: boolean
    /** Discovery is still running after the initial quick attempts. */
    retrying?: boolean
    /** Extra pages do not invalidate the verified choices already shown. */
    paging?: "loading" | "retrying" | "pending" | undefined
    retryStage?: "setup" | "workspaces" | "folders" | undefined
    scopeInvalid?: boolean
    loaded: boolean
    partial: boolean
    hasMore: boolean
    needsChoiceReview: boolean
    error: string | null
    /** The current list remains usable after a folder-only refresh failure. */
    refreshError?: string | undefined
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
function selectedFoldersKey(value: CloudSnapshot) {
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

function exactProjectKey(project: CloudProjectSelection) {
  return JSON.stringify([project.sourceKind, project.sourcePath, project.projectPath])
}

type FolderRouteBinding = CloudProjectSelection & { repositoryGroupId?: string }

/** Associate requested anchors with repository IDs for receipt checks only. */
export function expectedFolderRoute(
  groups: DesktopCloudProjectGroup[],
  projects: CloudProjectSelection[],
  selectedGroups?: readonly CloudProjectGroup[],
): FolderRouteBinding[] {
  const selectedIds = selectedGroups && new Set(selectedGroups.map(({ id }) => id))
  const groupsByProject = new Map<string, Set<string | undefined>>()
  for (const group of groups) {
    if (selectedIds && !selectedIds.has(group.id)) continue
    for (const project of group.projects) {
      const key = exactProjectKey(project)
      const ids = groupsByProject.get(key) ?? new Set<string | undefined>()
      ids.add(group.repositoryWide ? group.id : undefined)
      groupsByProject.set(key, ids)
    }
  }
  return projects.flatMap((project) => {
    const ids = groupsByProject.get(exactProjectKey(project)) ?? [undefined]
    return Array.from(ids, (repositoryGroupId) =>
      repositoryGroupId ? { ...project, repositoryGroupId } : project,
    )
  })
}

/** Read only the provider bindings native saved for this repository ID. */
export function savedRepositoryProjects(policy: CloudOngoingStatus, id: string) {
  if (!policy.repositoryGroupIds?.includes(id)) return []
  return policy.repositoryFolders?.find(({ groupId }) => groupId === id)?.projects ?? []
}

type SavedProjectBindings = Pick<
  CloudOngoingStatus,
  "projects" | "exactProjects" | "repositoryGroupIds"
>

/** Without repository rules, old replies contain only exact projects. */
export function savedExactProjects(policy: SavedProjectBindings) {
  return policy.exactProjects ?? policy.projects
}

/** Keep all exact and repository source bindings when renewing the saved Team policy. */
export function savedFolderRoute(policy: CloudOngoingStatus | undefined): FolderRouteBinding[] {
  if (!policy) return []
  return [
    ...savedExactProjects(policy),
    ...(policy.repositoryFolders ?? []).flatMap(({ groupId, projects }) =>
      projects.map((project) => ({ ...project, repositoryGroupId: groupId })),
    ),
  ]
}

/** Confirm the saved source bindings and any repository scopes for one route. */
export function folderRouteConfirmed(
  status: CloudOngoingStatus | null | undefined,
  binding: CloudDestinationBinding,
  selected: FolderRouteBinding[],
) {
  if (
    status?.accountId !== binding.accountId ||
    status.destinationId !== binding.destinationId ||
    !status.storageKnown ||
    status.consentVersion !== "desktop-project-sync-v1" ||
    !selected.length
  )
    return false
  return selected.every(savedFolderBindingMatcher(status))
}

/** Match one exact source binding or the source anchor of a saved repository rule. */
function savedFolderBindingMatcher(status: CloudOngoingStatus) {
  const saved = new Set(savedExactProjects(status).map(exactProjectKey))
  return ({ repositoryGroupId, ...project }: FolderRouteBinding) =>
    repositoryGroupId
      ? savedRepositoryProjects(status, repositoryGroupId).some(
          (anchor) =>
            anchor.sourceKind === project.sourceKind &&
            anchor.sourcePath === project.sourcePath,
        )
      : saved.has(exactProjectKey(project))
}

/** Verify that the observed policy still matches the completed native write. */
export function confirmsSavedWrite(
  current: CloudOngoingStatus | null | undefined,
  reply: CloudOngoingStatus,
  binding: CloudDestinationBinding,
  projects: FolderRouteBinding[],
) {
  return (
    !!reply.policyRevision &&
    matchesUploadPolicy(current, binding, reply.policyRevision) &&
    !current?.folderConfirmationRequired &&
    !current?.sourceConfirmationRequired &&
    folderRouteConfirmed(reply, binding, projects) &&
    (current === reply || folderRouteConfirmed(current, binding, projects))
  )
}

/** Save the exact route evidence for the final onboarding check. */
export interface RequestedFolderRoute {
  target: CloudDestinationBinding
  name: string
  expected: FolderRouteBinding[]
}

/** Recheck saved routes on later attempts. Drop grants Desktop confirms are gone. */
export function checkOnboardingFolderRoutes(
  routes: readonly RequestedFolderRoute[],
  priorSavedRoutes: readonly RequestedFolderRoute[],
  policyFor: (id: string) => CloudOngoingStatus | null | undefined,
) {
  const savedRoutes = new Map<string, RequestedFolderRoute>()
  const pending = new Map<string, string>()
  let needsReview = false
  const remember = (route: RequestedFolderRoute) =>
    savedRoutes.set(JSON.stringify([route.target, route.expected]), route)
  for (const route of priorSavedRoutes) {
    const policy = policyFor(route.target.destinationId)
    const known = hasFolderRuleEvidence(policy, route.target)
    if (known && !hasRequestedFolderRule(route, policy)) continue
    remember(route)
    if (
      known &&
      policy.storageKnown &&
      !policy.folderConfirmationRequired &&
      !policy.sourceConfirmationRequired
    )
      continue
    pending.set(route.target.destinationId, route.name)
    needsReview = true
  }
  for (const route of routes) {
    const { target, name, expected } = route
    const policy = policyFor(target.destinationId)
    if (hasFolderRuleEvidence(policy, target) && hasRequestedFolderRule(route, policy))
      remember(route)
    if (policy?.folderConfirmationRequired || policy?.sourceConfirmationRequired)
      needsReview = true
    else if (folderRouteConfirmed(policy, target, expected)) continue
    pending.set(target.destinationId, name)
  }
  const nextStep = needsReview
    ? "Review saved folders in Settings before trying again."
    : "Review the remaining choices and try again."
  const error = pending.size
    ? `Could not confirm folder sync for ${[...pending.values()].join(", ")}. ${nextStep}`
    : null
  return { error, savedRoutes: [...savedRoutes.values()] }
}

/** Team policy metadata can prove saved rules even when upload counters are unavailable. */
function hasFolderRuleEvidence(
  policy: CloudOngoingStatus | null | undefined,
  target: CloudDestinationBinding,
): policy is CloudOngoingStatus {
  return (
    policy?.accountId === target.accountId &&
    policy.destinationId === target.destinationId &&
    (policy.storageKnown || policy.status === "consent_required" || !!policy.savedFolders)
  )
}

/** Check whether the latest policy still grants any binding in this onboarding route. */
function hasRequestedFolderRule(route: RequestedFolderRoute, policy: CloudOngoingStatus) {
  return (
    policy.consentVersion === "desktop-project-sync-v1" &&
    route.expected.some(savedFolderBindingMatcher(policy))
  )
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
  waiting: "Waiting for browser approval. Finish in your browser, then return here.",
  returned: "Browser step complete. Checking your sign-in…",
  signed_in: "Your account is verified.",
  signed_out: "You’re signed out on this device.",
  cancelled: "Sign-in cancelled.",
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

/** A closed account grant can retain proof that must be cleaned up before reconnecting. */
export const cloudAccountCleanupPending = (value: CloudSnapshot) =>
  !!(value.revocationPending || value.localCleanupPending)

export const cloudSignOutBeforeSignIn = "Sign out on this device before you sign in again."

/** Distinguish fresh setup from an unreadable saved-upload state. */
export const cloudUploadStatusUnknown = (value: CloudOngoingStatus | null | undefined) =>
  !!value && !value.storageKnown && value.status !== "consent_required"

/** Retry only when desktop reports a bound job it can retry now. */
export const canRetryPersonalUploads = (value: CloudViewSnapshot) =>
  value.deviceUploadEnabled === true &&
  value.ongoing?.retryAvailable === true &&
  !value.ongoing.paused &&
  !value.ongoing.sourceConfirmationRequired

/** Start a full scan only for an idle, verified workspace with saved folders. */
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
    !policy.backfillInProgress &&
    !policy.fullSyncInProgress &&
    !policy.backgroundSyncInProgress &&
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
    !canBackfillUploads(current, teamId)
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
    route_denied: "Cloud denied uploads to this workspace",
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

const folderSaveFailures = {
  changed: {
    message: "The folders or session files changed. Review the folder choices.",
    recovery: "refresh",
  },
  incomplete: {
    message: "Could not verify every selected folder.",
    recovery: "refresh",
  },
  connection: {
    message: "Could not reach Cloud to save your folders.",
    recovery: "retry",
  },
  unexpected: {
    message: "Could not save the folder choices.",
    recovery: "retry",
  },
  team: {
    message: "Review saved folders before saving new Team choices.",
    recovery: "refresh",
  },
} as const

/** Explain known Desktop save failures without exposing provider errors or paths. */
export function folderSaveError(message: unknown) {
  switch (message) {
    case CLOUD_BUSY_MESSAGE:
      return CLOUD_BUSY_MESSAGE
    case "Turn off all folders under the old Team policy before approving new uploads.":
      return folderSaveFailures.team.message
    case "A session source changed. Reload projects and confirm again.":
    case "The project folders changed. Reload projects and confirm again.":
    case "Cloud sources changed. Confirm sync again.":
    case "The saved workspace or folder changed.":
      return folderSaveFailures.changed.message
    case "Project discovery is incomplete. Restore access or choose fewer sources and retry.":
    case "Could not confirm a supported session for this project. Reload projects and try again.":
      return folderSaveFailures.incomplete.message
    case "Connect to Cloud before confirming sync.":
    case "Cloud upload is unavailable. No source content was uploaded.":
      return folderSaveFailures.connection.message
    default:
      return folderSaveFailures.unexpected.message
  }
}

/** Match an action to a known folder save error, not an unrelated upload error. */
export function folderSaveRecovery(message: string | null | undefined) {
  switch (message) {
    case CLOUD_BUSY_MESSAGE:
      return "check"
    case "Could not start sync.":
    case "Could not save folders. Try again.":
      return "retry"
    case "Restore saved sources or turn off their folders.":
    case "The saved folder choices changed.":
    case "The saved folder or workspace changed. Review the folder choices and try again.":
    case "A selected team is unavailable. Check the folder choices.":
    case "Sync stopped in the old workspace. Check the saved folder before choosing a workspace.":
    case "Could not confirm where this folder syncs. Check the saved folder before choosing a workspace.":
    case "Could not confirm that the old folder rule is off. Check the saved folder before trying again.":
    case "Could not verify the Team folder rules. Check the workspace and try again.":
      return "refresh"
    case "Could not update the folder. Try again.":
    case "Enable folders for one workspace at a time. No folder choices were saved.":
    case "Confirm the saved Team folders before moving this folder.":
    case "Sync stopped for this folder. Choose a workspace to sync it again.":
      return "review"
    default:
      return Object.values(folderSaveFailures).find((failure) => failure.message === message)
        ?.recovery
  }
}

/** Map upload errors to safe copy for the action that failed. */
export function uploadActionError(error: unknown, backfill: boolean, team: boolean) {
  const message = error instanceof Error ? error.message : error
  if (isCloudBusy(error)) return CLOUD_BUSY_MESSAGE
  if (message === "Cloud folder choices changed. Reload before retrying saved work.")
    return backfill
      ? "The saved folder choices changed."
      : "Cloud folder choices changed. Reopen Settings before retrying."
  if (backfill) return "Could not start sync."
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
  const sourcesChanged = cloudProjectSourceKey(previous) !== cloudProjectSourceKey(next)
  return {
    resetProjects,
    sourcesChanged,
    contextChanged: resetProjects || sourcesChanged || !!next.setupError,
  }
}

/** Include provider opt-outs, not just the paths selected for upload. */
export function cloudProjectSourceKey(cloud: CloudSnapshot) {
  return JSON.stringify(
    cloud.draft.folders.map((folder) => [cloudSourceKey(folder), folder.selected]).sort(),
  )
}

/** Build the view update for a desktop account event. The session owns the timers. */
export function accountViewPatch(
  previous: CloudViewSnapshot,
  value: CloudSnapshot,
  options: {
    resetProjects: boolean
    contextChanged: boolean
    projectSyncState: "pending" | "started" | undefined
  },
): Partial<CloudViewSnapshot> {
  const { resetProjects, contextChanged, projectSyncState } = options
  const screen = value.identity
    ? value.draft.screen === "handoff"
      ? "identity"
      : value.draft.screen
    : "handoff"
  const complete =
    projectSyncState === "started" || (projectSyncState !== "pending" && value.syncEnabled)
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
      : value.setupError && !previous.cloud.setupError && previous.projects
        ? { projects: { ...previous.projects, loading: false } }
        : {}),
    screen:
      value.identity && complete
        ? "complete"
        : projectSyncState === "pending" && screen === "complete"
          ? previous.screen
          : screen,
    loaded: true,
    accountReadFailed: false,
    ...(previous.accountReadFailed || previous.error === CLOUD_BUSY_MESSAGE
      ? { error: null }
      : {}),
    ...(contextChanged ? { sync: { status: "unavailable" as const } } : {}),
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
  return group.captureSupported && group.groupingComplete
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
    repositoryGroupIds(groups.filter((group) => choices[group.id] ?? true)).sort(),
    selections,
    Object.entries(targets).sort(),
    (workspaces ?? []).map((workspace) => [workspace.id, workspace.kind]).sort(),
  ])
}

/** Get the repository IDs in the displayed selection. */
function repositoryGroupIds(groups: readonly CloudProjectGroup[]) {
  return groups.filter((group) => group.repositoryWide).map(({ id }) => id)
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
    group.repositoryWide === true,
    [...group.folderPaths].sort(),
    selections.map(exactProjectKey).sort(),
  ])
}

/** Keep only saved Personal bindings eligible for source recovery. */
export function reviewableProjectSelections(
  selections: CloudProjectSelection[],
  policy: CloudOngoingStatus | null | undefined,
  repositoryGroupId?: string,
) {
  if (!policy) return []
  const saved = repositoryGroupId
    ? savedRepositoryProjects(policy, repositoryGroupId)
    : savedExactProjects(policy)
  return selections.filter((project) =>
    saved.some(
      (binding) =>
        binding.sourceKind === project.sourceKind &&
        binding.sourcePath === project.sourcePath &&
        (repositoryGroupId || binding.projectPath === project.projectPath),
    ),
  )
}

/** Match repository grants by ID and exact folder grants by directory boundary. */
export function hasSavedProjectPermission(
  group: DesktopCloudProjectGroup | undefined,
  policy: SavedProjectBindings | null | undefined,
  exact: boolean,
) {
  if (group?.environment !== "native" || !policy) return false
  if (group.repositoryWide) return policy.repositoryGroupIds?.includes(group.id) === true
  return savedExactProjects(policy).some((project) =>
    [group.path, ...group.folderPaths].some(
      (path) =>
        path === project.projectPath ||
        (!exact &&
          (path.startsWith(`${project.projectPath.replace(/\/+$/, "")}/`) ||
            project.projectPath.startsWith(`${path.replace(/\/+$/, "")}/`))),
    ),
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

/** Use indexed counts for display and current provider evidence for confirmation. */
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
      groupingComplete: group.groupingComplete,
      ...(group.repositoryWide && { repositoryWide: true }),
      ...(group.legacyOptOutId && { legacyOptOutId: group.legacyOptOutId }),
      discoveryStale: group.discoveryStale ?? false,
      sessionCount: group.discoveryStale ? null : group.sessionCount,
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
    if (group.discoveryStale) delete choicesForStart[group.id]
    else if (hasSavedProject(group.id) || (explicitOnly && !targets[group.id]))
      choicesForStart[group.id] = false
  }
  return { choicesForStart, explicitOnly }
}
