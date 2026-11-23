import type { CloudViewSnapshot } from "./CloudSession"

/** Team Retry requires a verified destination and a displayed, retryable policy revision. */
export function canRetryTeamUploads(value: CloudViewSnapshot, destinationId: string): boolean {
  const team = value.teamStatuses?.[destinationId]
  return (
    !!team &&
    team.accountId === value.cloud.identity?.id &&
    team.destinationId === destinationId &&
    !!team.policyRevision &&
    team.retryAvailable === true &&
    !team.folderConfirmationRequired &&
    !team.paused &&
    value.deviceUploadEnabled === true &&
    !!value.projects?.workspaces?.some(
      ({ id, kind }) => id === destinationId && kind === "team",
    )
  )
}
