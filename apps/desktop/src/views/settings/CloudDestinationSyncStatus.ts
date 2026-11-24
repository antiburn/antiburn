import type { CloudOngoingStatus } from "../../lib/cloud"

const sessionCountFormat = new Intl.NumberFormat()
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

/** Report one idle result. Never add counts from separate workspace scans. */
export function idleSyncMessage({
  policies,
  deviceEnabled,
  checkingStatus,
  workspaceUnavailable,
  hasError,
}: {
  policies: readonly CloudOngoingStatus[]
  deviceEnabled: boolean | undefined
  checkingStatus: boolean | undefined
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
    if (unavailable)
      return checkingStatus && policy.status === "storage_unavailable"
        ? "Checking saved uploads…"
        : unavailable
  }
  const enabled = policies.filter(
    ({ consentVersion, projects }) =>
      consentVersion === "desktop-project-sync-v1" && projects.length > 0,
  )
  if (!enabled.length) return "Folder sync not started"
  if (deviceEnabled === false || enabled.every(({ paused }) => paused))
    return "Folder sync paused"
  if (deviceEnabled !== true) return "Checking folder sync…"
  const current = enabled.filter(({ storageKnown, paused }) => storageKnown && !paused)
  if (!current.length) return "Status unavailable"
  if (current.some(onlyAutomaticRecovery)) return "Checking saved uploads…"
  if (enabled.length === 1 && current.length === 1) {
    const progress = current[0]?.uploadProgress
    if (progress?.scanIncomplete) return "Folder scan incomplete"
    if (progress?.total === 0) return "No sessions found to sync."
    if (progress && progress.uploaded > 0) {
      const { uploaded, total } = progress
      const count = sessionCountFormat.format(uploaded)
      if (total != null && total > uploaded)
        return `${count} of ${sessionCountFormat.format(total)} sessions uploaded`
      return `${count} ${uploaded === 1 ? "session" : "sessions"} uploaded`
    }
  }
  return "Folder sync enabled"
}
