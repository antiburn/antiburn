import type {
  CloudDestinationBinding,
  CloudOngoingStatus,
  CloudPort,
  CloudSnapshot,
  CloudWorkspace,
  CloudWorkspaceRecovery,
} from "../lib/cloud"
import { retryCloudRead } from "./cloudReadRetry"
import { cloudProjectSourceKey, type CloudViewSnapshot } from "./cloudSessionState"

export const cloudCaptureUnsupported = "Desktop capture is not supported on this platform."

/** Name the read that has not finished. Do not suggest changing local access for Cloud errors. */
export function cloudDiscoveryRetryMessage(
  stage: NonNullable<CloudViewSnapshot["projects"]>["retryStage"],
) {
  if (stage === "workspaces") return "Still checking Cloud workspaces."
  if (stage === "setup") return "Still checking Cloud setup."
  return "Still checking session folders."
}

/** Recheck incomplete metadata when the user returns after restoring a source. */
export function shouldRecheckCloudFolders({
  busy,
  cloud,
  projects,
  ongoing,
}: CloudViewSnapshot) {
  return (
    !busy &&
    !projects?.loading &&
    !projects?.paging &&
    !projects?.scopeInvalid &&
    cloud.setupError !== cloudCaptureUnsupported &&
    (!projects ||
      ongoing?.status === "storage_unavailable" ||
      !!cloud.setupError ||
      !!projects?.error ||
      !!projects?.retrying ||
      !!(
        projects?.loaded &&
        !projects.hasMore &&
        (!projects.groups.length || projects.partial)
      ))
  )
}

/** Account events can change the source scope without a Settings action. */
export function cloudFolderReadChanged(previous: CloudSnapshot, current: CloudViewSnapshot) {
  return (
    !current.busy &&
    (!current.projects ||
      !!previous.setupError !== !!current.cloud.setupError ||
      cloudProjectSourceKey(previous) !== cloudProjectSourceKey(current.cloud))
  )
}

/** One automatic recheck on a transition to unreadable storage, not on every event. */
export function savedUploadStatusBecameUnavailable(
  value: CloudViewSnapshot,
  status: CloudOngoingStatus,
) {
  if (value.busy || status.status !== "storage_unavailable" || !status.destinationId)
    return false
  const previous =
    status.destinationId === value.cloud.destination?.id
      ? value.ongoing
      : value.teamStatuses?.[status.destinationId]
  return previous?.status !== "storage_unavailable"
}

/** Keep the last checked list in place until the replacement read settles. */
export function cloudFoldersLoading(
  existing: CloudViewSnapshot["projects"],
  needsChoiceReview: boolean,
): NonNullable<CloudViewSnapshot["projects"]> {
  return {
    groups: [],
    choices: {},
    loaded: false,
    partial: false,
    hasMore: false,
    selectionKey: "",
    ...existing,
    loading: true,
    retrying: false,
    retryStage: undefined,
    scopeInvalid: false,
    needsChoiceReview,
    error: null,
  }
}

/** Apply read results only if a newer event has not changed the same state. */
export interface ProjectScopeObserver {
  getSnapshot: () => CloudViewSnapshot
  accountRevision: () => number
  policyRevision: () => number
  receiveAccount: (value: CloudSnapshot) => void
  receivePolicy: (value: CloudOngoingStatus) => void
  receiveWorkspace: (value: CloudWorkspaceRecovery) => void
}

/** Read workspaces for the current account and keep newer desktop events.
 * A null result means a setup error other than unsupported capture blocks the read.
 */
export async function readCloudProjectScope(
  port: CloudPort,
  ongoing: NonNullable<CloudPort["ongoing"]>,
  binding: CloudDestinationBinding,
  signal: AbortSignal,
  observer: ProjectScopeObserver,
  request: <T>(read: () => Promise<T>) => Promise<T>,
  onExtendedRetry?: (stage: "setup" | "workspaces") => void,
): Promise<readonly CloudWorkspace[] | null | undefined> {
  const setupError = observer.getSnapshot().cloud.setupError
  if (setupError && setupError !== cloudCaptureUnsupported) {
    const revision = observer.accountRevision()
    const cloud = await retryCloudRead(
      () => request(port.read),
      signal,
      (value) => !!value.setupError && value.setupError !== cloudCaptureUnsupported,
      onExtendedRetry && (() => onExtendedRetry("setup")),
    )
    signal.throwIfAborted()
    if (revision === observer.accountRevision()) observer.receiveAccount(cloud)
    signal.throwIfAborted()
  }

  const revision = observer.policyRevision()
  const previous = observer.getSnapshot()
  let unavailableReads = 0
  const status = await retryCloudRead(
    () => request(ongoing.status),
    signal,
    (value) =>
      (value?.status === "storage_unavailable" ||
        !!value?.teamStatuses?.some(({ status }) => status === "storage_unavailable")) &&
      unavailableReads++ < 2,
    onExtendedRetry && (() => onExtendedRetry("setup")),
  )
  signal.throwIfAborted()
  if (
    status?.accountId === binding.accountId &&
    status.destinationId === binding.destinationId
  ) {
    for (const team of status.teamStatuses ?? []) {
      const id = team.destinationId
      if (!id || id === binding.destinationId || team.accountId !== binding.accountId) continue
      const current = observer.getSnapshot()
      if (previous.teamStatuses?.[id] === current.teamStatuses?.[id])
        observer.receivePolicy(team)
      if (previous.workspaceConnections?.[id] === current.workspaceConnections?.[id])
        observer.receiveWorkspace({
          accountId: team.accountId,
          workspaceId: id,
          workspaceName: team.workspaceName,
          localResetAvailable: team.localResetAvailable,
          remoteRevocationUnconfirmed: team.remoteRevocationUnconfirmed,
        })
    }
  }
  if (status && revision === observer.policyRevision()) observer.receivePolicy(status)
  const currentSetupError = observer.getSnapshot().cloud.setupError
  if (currentSetupError && currentSetupError !== cloudCaptureUnsupported) return null

  const verifyWorkspaces = port.workspaces
  if (!verifyWorkspaces) return previous.projects?.workspaces
  const missingPersonal = (items: readonly CloudWorkspace[] | null | undefined) =>
    !!items &&
    !items.some(({ id, kind }) => id === binding.destinationId && kind === "personal")
  const workspaces = await retryCloudRead(
    () => request(() => verifyWorkspaces(binding)),
    signal,
    undefined,
    onExtendedRetry && (() => onExtendedRetry("workspaces")),
  )
  signal.throwIfAborted()
  if (missingPersonal(workspaces))
    throw new Error("The signed-in workspace is not available for selection.")
  return workspaces ?? undefined
}
