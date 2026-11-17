import { ArrowUpRight } from "lucide-react"
import { useState } from "react"
import {
  type CloudSession,
  cloudStatusMessage,
  type CloudViewSnapshot,
} from "../../onboarding/CloudSession"
import { CloudCaptureConsent } from "../../onboarding/CloudCaptureConsent"
import { ToggleSwitch } from "../../components/ui/ToggleSwitch"
import { SettingsRow } from "./SettingsSearchRows"
import { personalCloudDestination } from "../../lib/cloudDestination"

export function CloudAccount({
  session,
  value,
}: {
  session: CloudSession
  value: CloudViewSnapshot
}) {
  const [confirmReset, setConfirmReset] = useState(false)
  return (
    <>
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
        {value.cloud.authEnabled &&
          !value.cloud.identity &&
          !value.cloud.revocationPending &&
          !value.cloud.localCleanupPending && (
            <button
              className="ui-push-button"
              disabled={
                !value.loaded ||
                value.busy ||
                ["reauth_required", "credential_failed"].includes(value.cloud.status)
              }
              onClick={() => void session.begin()}
            >
              Sign in
            </button>
          )}
        {(value.cloud.identity ||
          value.cloud.revocationPending ||
          value.cloud.localCleanupPending ||
          value.cloud.status === "reauth_required" ||
          value.cloud.status === "credential_failed") && (
          <button
            className="ui-push-button"
            disabled={value.busy}
            onClick={() => void session.signOut()}
          >
            {value.cloud.localCleanupPending
              ? "Retry secure cleanup"
              : value.cloud.revocationPending
                ? "Retry revocation"
                : "Sign out"}
          </button>
        )}
      </div>
      {value.cloud.remoteRevocationUnconfirmed && (
        <p
          className="border-t border-separator px-4 py-3 type-footnote text-label-secondary"
          role="status"
        >
          Cloud has not confirmed revocation of an earlier connection. Revoke that connection in
          browser Settings.
        </p>
      )}
      {value.cloud.localResetAvailable && (
        <div className="flex flex-col gap-3 border-t border-separator px-4 py-3 type-callout text-label">
          {confirmReset ? (
            <>
              <p>
                Reset local connection state? You can reconnect, but this does not revoke the
                old connection. The unconfirmed-revocation warning will remain.
              </p>
              <div className="flex gap-2">
                <button
                  className="ui-push-button"
                  disabled={value.busy}
                  onClick={() => {
                    setConfirmReset(false)
                    void session.resetLocal()
                  }}
                >
                  Reset and keep warning
                </button>
                <button
                  className="ui-push-button"
                  disabled={value.busy}
                  onClick={() => setConfirmReset(false)}
                >
                  Cancel
                </button>
              </div>
            </>
          ) : (
            <button
              className="ui-push-button"
              disabled={value.busy}
              onClick={() => setConfirmReset(true)}
            >
              Reset local connection
            </button>
          )}
        </div>
      )}
      <SettingsRow
        searchId="cloudFolders"
        className="border-t border-separator"
        description={
          value.cloud.setupError ??
          (value.cloud.identity
            ? value.cloud.captureEnabled
              ? "Choose native Claude projects/<project> source directories directly, not repositories. Antiburn does not infer sources from working directories or suggest repositories. Source choices alone do not authorize capture."
              : "Choose folders for this account. Folder choices alone do not authorize capture."
            : "Connect to Cloud before choosing folders.")
        }
        trailing={
          <button
            className="ui-push-button"
            disabled={value.busy || !value.cloud.identity || !!value.cloud.setupError}
            onClick={() => void session.addFolders()}
          >
            Choose folders…
          </button>
        }
      >
        {value.cloud.identity && (
          <div className="mt-3 flex flex-col gap-3">
            {value.cloud.draft.folders.map((folder) => (
              <div key={folder.path} className="flex items-center justify-between gap-3">
                <span className="min-w-0 break-all type-callout text-label">{folder.path}</span>
                <ToggleSwitch
                  aria-label={`Select ${folder.path} for Cloud capture`}
                  checked={folder.selected}
                  disabled={value.busy || !!value.cloud.setupError}
                  onCheckedChange={(selected) =>
                    void session.selectFolder(folder.path, selected)
                  }
                />
              </div>
            ))}
          </div>
        )}
      </SettingsRow>
      {value.cloud.identity && (
        <>
          <CloudCaptureConsent session={session} value={value} />
          {value.cloud.captureEnabled && (
            <div className="flex flex-col items-start gap-2 px-4 pb-3">
              <button
                className="ui-push-button"
                disabled={
                  value.busy ||
                  !value.loaded ||
                  value.cloud.status !== "signed_in" ||
                  !personalCloudDestination(value.cloud.destination ?? null) ||
                  !!value.cloud.setupError ||
                  value.sync.status === "cancelled" ||
                  !value.fullContentConsent ||
                  !value.cloud.draft.folders.some((folder) => folder.selected)
                }
                onClick={() => void session.startSync()}
              >
                {value.sync.status === "upload_unavailable" && value.error
                  ? "Retry source listing"
                  : "Save capture request"}
              </button>
              <p className="type-footnote text-label-secondary" role="status">
                {value.sync.status === "upload_unavailable"
                  ? "Capture request saved on this device. Upload unavailable."
                  : value.sync.status === "starting"
                    ? "Saving capture request. Upload unavailable."
                    : value.sync.status === "ambiguous"
                      ? "Could not confirm the capture request. Try Save capture request again. Upload unavailable."
                      : value.sync.status === "cancelled"
                        ? "Capture request cancelled. Upload unavailable."
                        : "Upload unavailable."}
              </p>
            </div>
          )}
          <p className="px-4 pb-3 type-footnote text-label-secondary">
            Sign out revokes this desktop connection only. It does not sign out your browser or
            delete saved captures.
          </p>
        </>
      )}
    </>
  )
}
