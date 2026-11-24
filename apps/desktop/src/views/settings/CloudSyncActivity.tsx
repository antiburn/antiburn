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

/**
 * Keep session acceptance and cumulative transfer visible through routine phases.
 * Unknown totals stay unknown. A plus marks sessions found before a scan finishes.
 * Separate workspace deliveries count separately, even for the same local session.
 */
export function uploadProgressMessage(policies: readonly CloudOngoingStatus[], phase?: string) {
  if (!phase && !policies.some((policy) => policy.uploadProgress)) return null
  const progress = policies.map((policy) => policy.uploadProgress)
  const uploadedCount =
    progress.length > 0 && progress.every((value) => value !== undefined)
      ? sessionCountFormat.format(
          progress.reduce((sum, value) => sum + (value?.uploaded ?? 0), 0),
        )
      : "?"
  const totalKnown =
    progress.length > 0 &&
    progress.every((value) => value?.total != null && !value.scanIncomplete)
  const total = totalKnown
    ? sessionCountFormat.format(progress.reduce((sum, value) => sum + (value?.total ?? 0), 0))
    : undefined
  const found = progress.reduce(
    (sum, value) => sum + (value?.total ?? value?.found ?? value?.uploaded ?? 0),
    0,
  )
  const sessionTotal = total ?? (found > 0 ? `${sessionCountFormat.format(found)}+` : "?")
  const transfers = progress.map((value) => value?.transfer)
  const sentBytes = transfers.reduce((sum, value) => sum + (value?.sentBytes ?? 0), 0)
  const sent =
    transfers.length > 0 && transfers.every((value) => value !== undefined)
      ? uploadSize(sentBytes)
      : "?"
  const bytesKnown =
    transfers.length > 0 && transfers.every((value) => value?.totalBytes != null)
  const totalBytes = bytesKnown
    ? uploadSize(transfers.reduce((sum, value) => sum + (value?.totalBytes ?? 0), 0))
    : "?"
  const sessions = policies.length > 1 ? "session uploads" : "sessions"
  const message = `${uploadedCount}/${sessionTotal} ${sessions}, ${sent} of ${totalBytes} uploaded`
  return phase ? `${message} • ${phase}` : message
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
