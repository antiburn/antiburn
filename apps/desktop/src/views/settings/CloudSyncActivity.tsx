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
  policies: readonly CloudOngoingStatus[],
  phase?: string,
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
  if (total === 0) {
    clauses.push("No sessions to upload")
  } else if (total !== null) {
    clauses.push(
      `${sessionCountFormat.format(uploaded)} of ${sessionCountLabel(total, combined)} ${accepted}`,
    )
  } else {
    clauses.push(`${sessionCountLabel(uploaded, combined)} ${accepted}`)
    const found = progress.reduce(
      (sum, value) =>
        sum +
        (value.scanIncomplete
          ? (value.found ?? value.uploaded)
          : (value.total ?? value.found ?? value.uploaded)),
      0,
    )
    if (found > uploaded) clauses.push(`${sessionCountLabel(found, combined)} found`)
  }

  const transfers = progress.flatMap(({ transfer }) => (transfer ? [transfer] : []))
  if (transfers.length === progress.length) {
    const sentBytes = transfers.reduce((sum, value) => sum + value.sentBytes, 0)
    const totalBytes = transfers.every((value) => value.totalBytes !== null)
      ? transfers.reduce((sum, value) => sum + (value.totalBytes ?? 0), 0)
      : null
    if (sentBytes !== 0 || totalBytes !== 0) {
      clauses.push(
        totalBytes === null
          ? `${uploadSize(sentBytes)} transferred`
          : `${uploadSize(sentBytes)} of ${uploadSize(totalBytes)} transferred`,
      )
    }
  }
  if (phase) clauses.push(phase)
  return clauses.join(" • ")
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
