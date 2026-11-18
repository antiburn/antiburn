import { ArrowUpRight, CircleAlert, Folder } from "lucide-react"
import { useState, type ReactNode } from "react"
import {
  type CloudSession,
  cloudStatusMessage,
  cloudUploadStatusUnknown,
  type CloudViewSnapshot,
} from "../../onboarding/CloudSession"
import { CloudProjectFolderSummary } from "../../onboarding/CloudOnboardingFolders"
import { ToggleSwitch } from "../../components/ui/ToggleSwitch"
import { SettingsRow } from "./SettingsSearchRows"
import { ConnectionDialog } from "./ConnectionDialog"
import { personalCloudDestination } from "../../lib/cloudDestination"
import {
  CLOUD_BUSY_MESSAGE,
  cloudSourceKey,
  cloudSourceKind,
  type CloudOngoingStatus,
} from "../../lib/cloud"

const uploadErrors: Partial<Record<CloudOngoingStatus["status"], string>> = {
  offline: "Cloud is offline.",
  reauth_required: "Sign in again to resume uploads.",
  upload_unavailable: "Cloud uploads are unavailable.",
  fenced: "Could not resume saved uploads.",
  interrupted: "Some sessions could not be prepared.",
  storage_unavailable: "Could not read saved uploads.",
}

/** Render one observed account. Setup errors block source actions, not revocation.
 * Local reset needs a separate confirmation and cannot claim remote deletion.
 */
export function CloudAccount({
  session,
  value,
  children,
}: {
  session: CloudSession
  value: CloudViewSnapshot
  /** The caller owns the upload switch and its saved-policy gate. */
  children?: ReactNode
}) {
  const [confirmReset, setConfirmReset] = useState(false)
  const readFailed = value.loaded && !!value.error && !value.cloud.identity
  const accountMessage = value.error
    ? value.error === CLOUD_BUSY_MESSAGE
      ? "Cloud is busy."
      : value.error
    : !["idle", "signed_in", "signed_out"].includes(value.cloud.status)
      ? cloudStatusMessage(value.cloud.status)
      : null
  return (
    <>
      <div className="flex items-center justify-between gap-3 px-4 py-3">
        <div className="min-w-0">
          <p className="type-body text-label">
            {readFailed
              ? "Cloud account"
              : value.cloud.identity
                ? `Signed in as ${value.cloud.identity.login}`
                : value.cloud.authEnabled
                  ? "Not signed in"
                  : "Antiburn Cloud is coming soon"}
          </p>
          {value.cloud.identity && value.cloud.destination?.kind === "personal" && (
            <p className="type-footnote text-label-secondary">{value.cloud.destination.name}</p>
          )}
          {accountMessage && (
            <p
              className={`type-footnote ${value.error ? "text-system-red-text" : "text-label-secondary"}`}
              role={value.error ? "alert" : "status"}
            >
              {accountMessage}
            </p>
          )}
        </div>
        {readFailed && (
          <button
            className="ui-push-button"
            disabled={value.busy}
            onClick={() => void session.load()}
          >
            Try again
          </button>
        )}
        {!readFailed && !value.cloud.authEnabled && !value.cloud.identity && (
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
          !readFailed &&
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
              ? "Finish signing out"
              : value.cloud.revocationPending
                ? "Retry sign-out"
                : "Sign out"}
          </button>
        )}
      </div>
      {value.cloud.remoteRevocationUnconfirmed && (
        <p className="px-4 py-3 type-footnote text-label-secondary" role="status">
          Cloud has not confirmed an earlier sign-out. Open Cloud settings in your browser to
          close that old connection.
        </p>
      )}
      {value.cloud.localResetAvailable && (
        <div className="flex flex-col gap-3 px-4 py-3 type-callout text-label">
          {confirmReset ? (
            <>
              <p>
                Reset sign-in on this device? You can sign in again, but the old connection may
                still have access to Cloud. The warning will stay until you close that
                connection in Cloud settings.
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
                  Reset sign-in
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
              Reset sign-in on this device
            </button>
          )}
        </div>
      )}
      {children}
      {session.projectSyncSupported ? (
        <CloudProjectsAccount session={session} value={value} />
      ) : (
        <>
          <SettingsRow
            searchId="cloudFolders"
            description={value.cloud.setupError ?? undefined}
          >
            {value.cloud.identity && (
              <div className="mt-3 flex flex-col gap-3">
                {value.cloud.draft.folders.map((folder) => (
                  <div
                    key={cloudSourceKey(folder)}
                    className="flex items-center justify-between gap-3"
                  >
                    <span className="min-w-0 break-all type-callout text-label">
                      {cloudSourceKind(folder) === "opencode_sqlite_v2"
                        ? "OpenCode V2"
                        : "Claude"}
                      : {folder.path}
                    </span>
                    <ToggleSwitch
                      aria-label={`Select ${folder.path} for Cloud capture`}
                      checked={folder.selected}
                      disabled={value.busy || !!value.cloud.setupError}
                      onCheckedChange={(selected) =>
                        void session.selectFolder(
                          folder.path,
                          selected,
                          cloudSourceKind(folder),
                        )
                      }
                    />
                  </div>
                ))}
              </div>
            )}
          </SettingsRow>
          {value.cloud.identity && (
            <>
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
                      !value.cloud.draft.folders.some((folder) => folder.selected)
                    }
                    aria-description={`Save a full-session capture request for ${value.cloud.identity.login} in ${value.cloud.destination?.name ?? "the selected destination"}. This does not upload content.`}
                    onClick={() => void session.startSync(value)}
                  >
                    {value.sourceListingNeedsRetry
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
            </>
          )}
        </>
      )}
    </>
  )
}

/** Keep initial choices local until Start. Saved switches revoke immediately or
 * require confirmation before adding content permission.
 */
function CloudProjectsAccount({
  session,
  value,
}: {
  session: CloudSession
  value: CloudViewSnapshot
}) {
  const projects = value.projects
  const [confirm, setConfirm] = useState<{
    id: string
    name: string
    context: CloudViewSnapshot
  } | null>(null)
  const policy = value.ongoing
  const hasConsent = policy?.consentVersion === "desktop-project-sync-v1"
  const storageUnknown = cloudUploadStatusUnknown(policy)
  const starting = value.sync.status === "starting"
  if (!value.cloud.identity || value.cloud.status !== "signed_in") return null
  if (!personalCloudDestination(value.cloud.destination ?? null)) {
    if (value.error) return null
    return (
      <p className="px-4 py-3 type-footnote text-label-secondary" role="status">
        {value.cloud.destination?.kind === "team"
          ? "Personal uploads are unavailable for this connection."
          : "Could not verify the Cloud destination."}
      </p>
    )
  }
  const folderError = projects?.error
    ? projects.error === CLOUD_BUSY_MESSAGE
      ? "Cloud is busy."
      : projects.loaded
        ? "Could not update folders."
        : "Could not load folders."
    : projects?.loaded && projects.partial && !projects.hasMore && projects.groups.length === 0
      ? "Could not finish loading folders."
      : value.cloud.setupError
        ? value.cloud.captureEnabled
          ? "Could not read Cloud folders."
          : "Uploads are unavailable on this device."
        : null
  const recoveryRequiredCount = policy?.recoveryRequiredCount ?? 0
  const interruptedError =
    recoveryRequiredCount > 0
      ? "Some sessions need local recovery. Saved data is kept."
      : uploadErrors.interrupted
  const uploadError = storageUnknown
    ? "Could not read saved uploads."
    : policy
      ? ((policy.status === "interrupted" ? interruptedError : uploadErrors[policy.status]) ??
        (policy.fencedCount > 0
          ? uploadErrors.fenced
          : policy.interruptedCount > 0
            ? interruptedError
            : null))
      : null
  const error = value.error ? null : (folderError ?? uploadError)
  const retryableUploads =
    recoveryRequiredCount === 0 ||
    storageUnknown ||
    (policy &&
      (policy.queuedCount > 0 ||
        policy.fencedCount > 0 ||
        policy.interruptedCount > recoveryRequiredCount ||
        (policy.status !== "interrupted" && !!uploadErrors[policy.status])))
  const retry = folderError
    ? value.cloud.setupError
      ? value.cloud.captureEnabled
        ? () => void session.load()
        : null
      : () => void session.loadProjects()
    : hasConsent && policy?.status !== "reauth_required" && retryableUploads
      ? () => void session.controlSync(false)
      : null
  const empty =
    projects?.loaded &&
    !projects.loading &&
    !projects.error &&
    !projects.hasMore &&
    !projects.partial &&
    !value.cloud.setupError &&
    !value.error &&
    projects.groups.length === 0
  const selected = (id: string) =>
    hasConsent ? session.projectHasSavedPermission(id) : (projects?.choices[id] ?? true)
  const syncState = hasConsent
    ? policy.paused
      ? "Uploads paused"
      : policy.projects.length === 0
        ? "No folders selected"
        : policy.status === "syncing" || policy.queuedCount > 0
          ? "Uploading sessions"
          : policy.status === "ready"
            ? "Uploads on"
            : null
    : null
  return (
    <>
      <SettingsRow
        searchId="cloudFolders"
        trailing={
          projects?.loaded && projects.groups.length > 0 ? (
            <span className="type-footnote text-label-secondary">
              {projects.groups.filter((group) => selected(group.id)).length} selected
            </span>
          ) : null
        }
      >
        <div className="mt-3 flex max-h-64 flex-col gap-3 overflow-y-auto">
          {projects?.groups.map((group) => (
            <div
              key={group.id}
              className="flex items-start justify-between gap-[var(--space-md)]"
            >
              <Folder size={16} className="mt-0.5 shrink-0 text-label-secondary" aria-hidden />
              <div className="min-w-0 flex-1 type-callout text-label">
                <strong className="block truncate">{group.name}</strong>
                <CloudProjectFolderSummary
                  id={`cloud-settings-folder-${encodeURIComponent(group.id)}`}
                  group={group}
                  homeDirectory={value.homeDirectory}
                />
              </div>
              <ToggleSwitch
                aria-label={`Sync ${group.name}: ${group.path}`}
                aria-describedby={`cloud-settings-folder-${encodeURIComponent(group.id)}`}
                checked={selected(group.id)}
                disabled={
                  value.busy ||
                  starting ||
                  storageUnknown ||
                  projects.loading ||
                  (!selected(group.id) && (!!value.cloud.setupError || !group.captureSupported))
                }
                onCheckedChange={(next) => {
                  if (!hasConsent) session.selectProject(group.id, next)
                  else if (next) setConfirm({ id: group.id, name: group.name, context: value })
                  else void session.setSavedProject(value, group.id, false)
                }}
              />
            </div>
          ))}
          {!projects?.loaded && !projects?.error && !value.cloud.setupError && (
            <p className="type-footnote text-label-secondary" role="status">
              Loading folders…
            </p>
          )}
          {empty && (
            <p className="type-footnote text-label-secondary">No session folders found.</p>
          )}
        </div>
        {projects?.hasMore && (
          <button
            className="ui-push-button mt-3"
            disabled={value.busy || starting || projects.loading}
            onClick={() => void session.loadMoreProjects()}
          >
            Load more folders
          </button>
        )}
      </SettingsRow>
      {!hasConsent && !!projects?.groups.length && (
        <div className="px-4 pb-3">
          <button
            className="ui-push-button"
            disabled={
              value.busy ||
              starting ||
              storageUnknown ||
              !value.cloud.captureEnabled ||
              !!value.cloud.setupError ||
              !projects?.loaded ||
              projects.loading ||
              !!projects.error ||
              !projects.groups.some(
                (group) => group.captureSupported && (projects.choices[group.id] ?? true),
              )
            }
            aria-description={`Sync full sessions from the selected folders and their worktrees to ${value.cloud.destination?.name ?? "the selected destination"} for ${value.cloud.identity.login}.`}
            onClick={() => void session.startSync(value)}
          >
            {starting ? "Starting…" : "Start syncing"}
          </button>
        </div>
      )}
      {syncState && !error && !value.error && (
        <p role="status" className="px-4 pb-3 type-footnote text-label-secondary">
          {syncState}
        </p>
      )}
      {error && (
        <div
          className="mx-4 mb-3 flex flex-wrap items-center gap-2 rounded-control bg-system-red/10 px-3 py-2 type-footnote text-system-red-text"
          role="alert"
        >
          <CircleAlert size={15} className="shrink-0" aria-hidden="true" />
          <span>{error}</span>
          {retry && (
            <button
              className="underline underline-offset-4"
              aria-label={folderError ? "Retry folder list" : "Retry saved uploads"}
              disabled={value.busy || starting || projects?.loading}
              onClick={retry}
            >
              {folderError ? "Retry folders" : "Retry uploads"}
            </button>
          )}
        </div>
      )}
      {confirm && (
        <ConnectionDialog
          title={`Upload sessions from ${confirm.name}?`}
          onCancel={() => setConfirm(null)}
        >
          <div className="flex flex-col gap-4 px-4 pb-4">
            <p className="type-callout text-label-secondary">
              Cloud will upload full sessions from this folder and its worktrees to your
              personal workspace.
            </p>
            <div className="flex justify-end gap-2">
              <button
                className="ui-push-button"
                disabled={value.busy}
                onClick={() => setConfirm(null)}
              >
                Cancel
              </button>
              <button
                className="ui-push-button"
                disabled={value.busy}
                onClick={async () => {
                  await session.setSavedProject(confirm.context, confirm.id, true)
                  setConfirm(null)
                }}
              >
                {value.busy ? "Allowing uploads…" : "Allow uploads"}
              </button>
            </div>
          </div>
        </ConnectionDialog>
      )}
    </>
  )
}
