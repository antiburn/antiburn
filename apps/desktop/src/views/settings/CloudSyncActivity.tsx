import { LoaderCircle } from "lucide-react"
import type { CloudOngoingStatus } from "../../lib/cloud"

const sessionCountFormat = new Intl.NumberFormat()
const uploadSizeFormat = new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 })

function uploadSize(bytes: number) {
  if (bytes >= 1_000_000_000) return `${uploadSizeFormat.format(bytes / 1_000_000_000)} GB`
  if (bytes >= 1_000_000) return `${uploadSizeFormat.format(bytes / 1_000_000)} MB`
  if (bytes >= 1_000) return `${uploadSizeFormat.format(bytes / 1_000)} KB`
  return `${sessionCountFormat.format(bytes)} B`
}

function sessionCountLabel(count: number, combined: boolean) {
  const unit = combined ? "session upload" : "session"
  return `${sessionCountFormat.format(count)} ${unit}${count === 1 ? "" : "s"}`
}

/**
 * Keep measured session acceptance and byte transfer visible through routine phases.
 * Omit unavailable measurements and use found counts only as lower bounds.
 * Separate workspace deliveries count separately, even for the same local session.
 */
export function uploadProgressMessage(
  policies: readonly Pick<CloudOngoingStatus, "uploadProgress">[],
  phase?: string,
  compact = false,
): string | null {
  const progress = policies.flatMap(({ uploadProgress }) =>
    uploadProgress ? [uploadProgress] : [],
  )
  if (!progress.length || progress.length !== policies.length) return phase || null

  const clauses: string[] = []
  const combined = policies.length > 1
  const uploaded = progress.reduce((sum, value) => sum + value.uploaded, 0)
  const totalKnown = progress.every((value) => value.total != null && !value.scanIncomplete)
  const total = totalKnown ? progress.reduce((sum, value) => sum + (value.total ?? 0), 0) : null
  const accepted = combined ? "complete" : "uploaded"
  const found = totalKnown
    ? null
    : progress.reduce(
        (sum, value) =>
          sum +
          (value.scanIncomplete
            ? (value.found ?? value.uploaded)
            : (value.total ?? value.found ?? value.uploaded)),
        0,
      )
  if (compact) {
    const count = sessionCountFormat.format(uploaded)
    if (total !== null && total > 0) {
      clauses.push(`Syncing ${count} of ${sessionCountLabel(total, combined)}`)
    } else if (
      found !== null &&
      (found > uploaded || (found > 0 && progress.some((value) => value.found != null)))
    ) {
      clauses.push(`Syncing ${count} of ${sessionCountLabel(found, combined)} found`)
    } else if (uploaded > 0) {
      clauses.push(`${sessionCountLabel(uploaded, combined)} ${accepted}`)
    }
  } else if (total === 0) {
    clauses.push("No sessions to upload")
  } else if (total !== null) {
    clauses.push(
      `${sessionCountFormat.format(uploaded)} of ${sessionCountLabel(total, combined)} ${accepted}`,
    )
  } else {
    clauses.push(`${sessionCountLabel(uploaded, combined)} ${accepted}`)
    if (found !== null && found > uploaded)
      clauses.push(`${sessionCountLabel(found, combined)} found`)
  }

  const transfers = progress.flatMap(({ transfer }) => (transfer ? [transfer] : []))
  if (transfers.length === progress.length) {
    const sentBytes = transfers.reduce((sum, value) => sum + value.sentBytes, 0)
    if (compact) {
      const label = clauses[0]
      return sentBytes > 0
        ? `${label ?? "Syncing"} (${uploadSize(sentBytes)} transferred)`
        : (label ?? null)
    }
    const totalBytes = transfers.every((value) => value.totalBytes !== null)
      ? transfers.reduce((sum, value) => sum + (value.totalBytes ?? 0), 0)
      : null
    if (sentBytes !== 0 || totalBytes !== 0) {
      const transfer =
        totalBytes === null
          ? `${uploadSize(sentBytes)} transferred`
          : `${uploadSize(sentBytes)} of ${uploadSize(totalBytes)} transferred`
      clauses.push(transfer)
    }
  }
  if (phase) clauses.push(phase)
  return clauses.join(" • ") || null
}

/** Prefer the explicit scan when it is active. Never show its old counts as background work. */
export function currentUploadProgressMessage(activePolicies: readonly CloudOngoingStatus[]) {
  const fullSync = activePolicies.filter(
    ({ backfillInProgress, fullSyncInProgress }) => backfillInProgress || fullSyncInProgress,
  )
  const sources = fullSync.length ? fullSync : activePolicies
  const progress = sources.map((policy) => {
    const measurement = fullSync.length
      ? policy.uploadProgress
      : policy.backgroundSyncInProgress
        ? policy.backgroundProgress
        : undefined
    return measurement ? { uploadProgress: measurement } : {}
  })
  if (
    sources.some(({ queuedCount }) => queuedCount > 0) &&
    progress.every(({ uploadProgress }) => uploadProgress?.total === 0)
  )
    return null
  return uploadProgressMessage(progress, undefined, true)
}

/** Describe saved work without treating the queue as a session scan. */
export function savedUploadProgressMessage(policies: readonly CloudOngoingStatus[]) {
  const pending = policies.reduce((sum, { queuedCount }) => sum + queuedCount, 0)
  const clauses: string[] = []
  if (pending > 0) {
    const unit = pending === 1 ? "upload" : "uploads"
    clauses.push(`${sessionCountFormat.format(pending)} saved ${unit} pending`)
  }
  const transfer =
    pending === 1 && policies.length === 1 ? policies[0]?.uploadTransfer : undefined
  if (transfer) {
    clauses.push(
      `Current upload: ${uploadSize(transfer.sentBytes)} of ${uploadSize(transfer.totalBytes)} transferred`,
    )
  }
  return clauses
}

/** Keep the live region mounted so it can announce a change once. */
export function CloudSyncActivity({ message, active }: { message: string; active: boolean }) {
  return (
    <p role="status" className="flex min-h-8 min-w-0 items-start gap-1 py-1.5">
      {active && (
        <span className="flex h-[1lh] shrink-0 items-center" aria-hidden>
          <LoaderCircle size={13} className="motion-safe:animate-spin" />
        </span>
      )}
      <span className="min-w-0 tabular-nums wrap-anywhere" title={message}>
        {message}
      </span>
    </p>
  )
}
