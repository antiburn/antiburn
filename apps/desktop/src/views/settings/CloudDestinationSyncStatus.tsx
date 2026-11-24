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

/** Report one workspace's queue and receipts. Do not show cached counts as current. */
export function CloudDestinationSyncStatus({
  name,
  policy,
  deviceEnabled,
  checkingStatus,
  workspaceUnavailable,
}: {
  name: string | undefined
  policy: CloudOngoingStatus | null | undefined
  deviceEnabled: boolean | undefined
  checkingStatus?: boolean | undefined
  workspaceUnavailable: boolean
}) {
  const known = policy?.storageKnown === true && !workspaceUnavailable
  const enabled =
    policy?.consentVersion === "desktop-project-sync-v1" && policy.projects.length > 0
  let state = "Checking folder sync…"
  if (policy) {
    const paused = deviceEnabled === false || policy.paused
    if (policy.folderConfirmationRequired || policy.sourceConfirmationRequired) {
      state = "Review saved folders"
    } else if (enabled) {
      state = paused
        ? "Folder sync paused"
        : !known
          ? "Sync status unavailable"
          : policy.status === "syncing"
            ? "Syncing sessions…"
            : "Folder sync enabled"
    } else {
      state =
        known || policy.status === "consent_required"
          ? "Folder sync not started"
          : "Status unavailable"
    }
    const unavailable = unavailableSyncStatus[savedUploadDisplayState(policy)]
    if (unavailable && state !== "Review saved folders")
      state = enabled && paused ? `${state} · ${unavailable}` : unavailable
    if (
      checkingStatus &&
      policy.status === "storage_unavailable" &&
      state !== "Review saved folders"
    )
      state = "Checking saved uploads…"
  }
  if (workspaceUnavailable) state = "Workspace unavailable"
  const progress = known && enabled ? policy.uploadProgress : undefined
  const acceptedCount =
    !progress && known && enabled && policy.acceptedCount > 0
      ? `${sessionCountFormat.format(policy.acceptedCount)} accepted`
      : null
  const jobsNeedingAttention = known ? policy.fencedCount + policy.interruptedCount : 0
  const counts = [
    known && enabled && policy.queuedCount > 0
      ? `${sessionCountFormat.format(policy.queuedCount)} queued`
      : null,
    jobsNeedingAttention > 0
      ? `${sessionCountFormat.format(jobsNeedingAttention)} need attention`
      : null,
    progress?.blocked ? `${sessionCountFormat.format(progress.blocked)} blocked` : null,
    progress?.unsupported
      ? `${sessionCountFormat.format(progress.unsupported)} unsupported`
      : null,
    progress?.scanIncomplete ? "Folder scan incomplete" : null,
  ]
  const automaticRecovery =
    known &&
    enabled &&
    deviceEnabled !== false &&
    !policy.paused &&
    !policy.sourceConfirmationRequired &&
    !policy.folderConfirmationRequired &&
    (policy.automaticRecoveryCount ?? 0) > 0
  return (
    <div
      role="group"
      aria-label={name ? `${name} sync status` : "Folder sync status"}
      className="min-w-0"
    >
      <p className="type-footnote text-label-secondary tabular-nums wrap-anywhere">
        {[name, state, acceptedCount, ...counts].filter(Boolean).join(" · ")}
      </p>
      {automaticRecovery && (
        <p className="type-footnote text-label-secondary" role="status">
          Checking saved uploads automatically. Saved files are kept.
        </p>
      )}
    </div>
  )
}
