import type { CloudOngoingStatus, CloudProjectGroup } from "../../lib/cloud"
import { uploadProgressMessage } from "./CloudSyncActivity"

/** Activity text when Desktop cannot distinguish transfer from recovery. */
export const syncingSessionsMessage = "Syncing sessions"

/** Use the current scan during activity. Never substitute an earlier full scan. */
export function currentFolderFound(policy: CloudOngoingStatus) {
  if (policy.fullSyncInProgress || policy.backfillInProgress)
    return policy.uploadProgress?.folderFound
  if (policy.backgroundSyncInProgress) return policy.backgroundProgress?.folderFound
  return (policy.backgroundProgress ?? policy.uploadProgress)?.folderFound
}

/** Sum rules within one verified workspace. A scan counts each source in at most one rule. */
export function folderSyncFoundCount(group: CloudProjectGroup, policy: CloudOngoingStatus) {
  const counts = currentFolderFound(policy)
  if (!counts) return undefined
  const ruleIds = new Set(
    policy.savedFolders
      ?.filter(({ groupId, path }) =>
        group.repositoryWide || policy.repositoryGroupIds?.includes(groupId)
          ? groupId === group.id
          : path === group.path || group.memberPaths.includes(path),
      )
      .map(({ groupId }) => groupId),
  )
  return [...ruleIds].reduce((sum, id) => sum + (counts[id] ?? 0), 0) || undefined
}

const unavailableSyncStatus: Partial<Record<CloudOngoingStatus["status"], string>> = {
  offline: "Cloud offline",
  route_denied: "Workspace denied uploads",
  reauth_required: "Sign-in or workspace access required",
  upload_unavailable: "Uploads unavailable",
  storage_unavailable: "Status unavailable",
  fenced: "Folder sync needs attention",
  interrupted: "Folder sync needs attention",
}

/** Report when every affected upload has automatic recovery and needs no user action. */
export function onlyAutomaticRecovery(policy: CloudOngoingStatus) {
  return (
    (policy.automaticRecoveryCount ?? 0) > 0 && !policy.fencedCount && !policy.interruptedCount
  )
}

/** Hide transient failure labels while Desktop still plans to recover all affected uploads. */
export function savedUploadDisplayState(policy: CloudOngoingStatus) {
  return onlyAutomaticRecovery(policy) &&
    (policy.status === "fenced" || policy.status === "interrupted")
    ? "syncing"
    : policy.status
}

/** A runtime report overrides the older coarse status after the final upload. */
export function syncActivityReported(policy: CloudOngoingStatus) {
  if (policy.backfillInProgress || policy.fullSyncInProgress || policy.backgroundSyncInProgress)
    return true
  if (policy.fullSyncInProgress !== undefined || policy.backgroundSyncInProgress !== undefined)
    return false
  return (
    (policy.status === "ready" && onlyAutomaticRecovery(policy)) ||
    savedUploadDisplayState(policy) === "syncing"
  )
}

/** Keep measured progress visible without inferring activity from missing counters. */
export function idleSyncMessage({
  policies,
  deviceEnabled,
  workspaceUnavailable,
  hasError,
}: {
  policies: readonly CloudOngoingStatus[]
  deviceEnabled: boolean | undefined
  workspaceUnavailable: boolean
  hasError: boolean
}) {
  if (workspaceUnavailable) return "Workspace unavailable"
  if (hasError) return "Folder sync needs attention"
  if (!policies.length) return "Checking folder sync…"
  if (
    policies.some(
      ({ folderConfirmationRequired, sourceConfirmationRequired }) =>
        folderConfirmationRequired || sourceConfirmationRequired,
    )
  )
    return "Review saved folders"

  for (const policy of policies) {
    const unavailable = unavailableSyncStatus[savedUploadDisplayState(policy)]
    if (unavailable) return unavailable
  }
  const enabled = policies.filter(
    ({ consentVersion, projects }) =>
      consentVersion === "desktop-project-sync-v1" && projects.length > 0,
  )
  if (!enabled.length) return "Folder sync not started"
  if (deviceEnabled === false || enabled.every(({ paused }) => paused))
    return "Folder sync paused"
  if (deviceEnabled !== true) return "Checking folder sync…"
  const current = enabled.filter(({ paused }) => !paused)
  if (current.some(({ storageKnown }) => !storageKnown)) return "Status unavailable"
  const phase = current.some((policy) => policy.uploadProgress?.scanIncomplete)
    ? "Folder scan incomplete"
    : undefined
  const progress = uploadProgressMessage(current, phase)
  if (progress) return phase ? progress : `Last full scan • ${progress}`
  return "Folder sync enabled"
}
