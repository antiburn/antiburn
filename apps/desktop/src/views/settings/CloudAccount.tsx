import { ArrowUpRight } from "lucide-react"
import {
  type CloudSession,
  cloudStatusMessage,
  type CloudViewSnapshot,
} from "../../onboarding/CloudSession"

export function CloudAccount({
  session,
  value,
}: {
  session: CloudSession
  value: CloudViewSnapshot
}) {
  return (
    <div className="flex items-center justify-between gap-3 border-t border-separator px-4 py-3">
      <div className="min-w-0">
        <p className="type-body text-label">
          {value.cloud.identity
            ? `Signed in as ${value.cloud.identity.login}`
            : value.cloud.authEnabled
              ? "Not signed in"
              : "Antiburn Cloud is coming soon"}
        </p>
        <p className="type-footnote text-label-secondary" role="status">
          {value.error ??
            (!value.cloud.authEnabled && value.cloud.status === "idle"
              ? "Bring your session insights together across devices."
              : value.cloud.status === "idle"
                ? "Cloud sync is unavailable. Your sessions stay on this device."
                : cloudStatusMessage(value.cloud.status))}
        </p>
      </div>
      {!value.cloud.authEnabled && !value.cloud.identity && (
        <button
          className="inline-flex min-h-10 shrink-0 items-center gap-1 rounded-control type-callout text-label-secondary hover:text-label disabled:opacity-50"
          disabled={!value.loaded || value.busy}
          onClick={() => void session.begin()}
        >
          Learn more <ArrowUpRight size={13} aria-hidden="true" />
        </button>
      )}
      {(value.cloud.identity || value.cloud.authEnabled) && (
        <button
          className="ui-push-button"
          disabled={value.busy}
          onClick={() => void session.signOut()}
        >
          Sign out
        </button>
      )}
    </div>
  )
}
