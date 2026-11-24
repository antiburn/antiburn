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

/** Desktop reports one upload's confirmed bytes, not a total for all folders. */
function currentUploadTransfer(policy: CloudOngoingStatus | undefined) {
  const transfer = policy?.uploadTransfer
  return transfer &&
    transfer.totalBytes > 0 &&
    transfer.sentBytes >= 0 &&
    transfer.sentBytes <= transfer.totalBytes
    ? transfer
    : undefined
}

/** Describe current activity without adding or comparing totals across workspaces. */
export function activeSyncMessage(scanning: boolean, policies: CloudOngoingStatus[]) {
  const transfers = policies.flatMap((policy) => currentUploadTransfer(policy) ?? [])
  const transfer = transfers.length === 1 ? transfers[0] : undefined
  if (transfer)
    return `${uploadSize(transfer.sentBytes)} of ${uploadSize(transfer.totalBytes)} uploaded`
  const progress = policies.length === 1 ? policies[0]?.uploadProgress : undefined
  const uploaded = progress?.uploaded ?? 0
  const count = sessionCountFormat.format(uploaded)
  if (scanning) {
    const found = progress?.found ?? 0
    if (found > 0) {
      const discovered = sessionCountFormat.format(found)
      return uploaded > 0
        ? `${discovered} found · ${count} uploaded`
        : `${discovered} ${found === 1 ? "session" : "sessions"} found`
    }
    return uploaded > 0 ? `Scanning folders · ${count} uploaded` : "Scanning folders…"
  }
  if (!uploaded) return "Uploading sessions…"
  const total = progress?.total
  if (total != null && total >= uploaded && !progress?.scanIncomplete)
    return `${count} of ${sessionCountFormat.format(total)} ${total === 1 ? "session" : "sessions"} uploaded`
  return `${count} ${uploaded === 1 ? "session" : "sessions"} uploaded`
}

/** Keep the live region mounted so it can announce a change once. */
export function CloudSyncActivity({ message, active }: { message: string; active: boolean }) {
  return (
    <p role="status" className="flex min-w-0 items-center gap-1">
      {active && (
        <LoaderCircle size={13} className="shrink-0 motion-safe:animate-spin" aria-hidden />
      )}
      <span className="truncate" title={message}>
        {message}
      </span>
    </p>
  )
}
