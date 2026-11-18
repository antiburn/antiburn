import { ArrowUpRight } from "lucide-react"
import { useState, type ReactNode } from "react"
import {
  type CloudSession,
  cloudStatusMessage,
  cloudOngoingStatusMessage,
  cloudUploadStatusUnknown,
  type CloudViewSnapshot,
} from "../../onboarding/CloudSession"
import { CloudCaptureConsent } from "../../onboarding/CloudCaptureConsent"
import { CloudProjectPaths } from "../../onboarding/CloudProjectPaths"
import { ToggleSwitch } from "../../components/ui/ToggleSwitch"
import { SettingsRow } from "./SettingsSearchRows"
import { personalCloudDestination } from "../../lib/cloudDestination"
import { cloudSourceKey, cloudSourceKind } from "../../lib/cloud"
import { byteLabel } from "../../lib/presentation/scanStatusCopy"

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
  /** Caller-owned upload switch stays above folder changes and consent text. */
  children?: ReactNode
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
              ? "Finish signing out"
              : value.cloud.revocationPending
                ? "Retry sign-out"
                : "Sign out"}
          </button>
        )}
      </div>
      {value.cloud.remoteRevocationUnconfirmed && (
        <p
          className="border-t border-separator px-4 py-3 type-footnote text-label-secondary"
          role="status"
        >
          Cloud has not confirmed an earlier sign-out. Open Cloud settings in your browser to
          close that old connection.
        </p>
      )}
      {value.cloud.localResetAvailable && (
        <div className="flex flex-col gap-3 border-t border-separator px-4 py-3 type-callout text-label">
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
            className="border-t border-separator"
            description={
              value.cloud.setupError ??
              (value.cloud.identity
                ? value.cloud.captureEnabled
                  ? "Choose the folders where Claude saves sessions, or choose an OpenCode V2 database. This does not give permission to upload their contents."
                  : "Choose folders for this account. Folder choices alone do not authorize capture."
                : "Connect to Cloud before choosing folders.")
            }
            trailing={
              <div className="flex flex-wrap gap-2">
                <button
                  className="ui-push-button"
                  disabled={value.busy || !value.cloud.identity || !!value.cloud.setupError}
                  onClick={() => void session.addFolders()}
                >
                  {value.cloud.captureEnabled ? "Choose Claude sources…" : "Choose folders…"}
                </button>
                {value.cloud.captureEnabled && (
                  <button
                    className="ui-push-button"
                    disabled={value.busy || !value.cloud.identity || !!value.cloud.setupError}
                    onClick={() => void session.addFolders("opencode_sqlite_v2")}
                  >
                    Choose OpenCode databases…
                  </button>
                )}
              </div>
            }
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
              <p className="px-4 pb-3 type-footnote text-label-secondary">
                Signing out disconnects this device only. It does not sign out your browser or
                delete saved uploads.
              </p>
            </>
          )}
        </>
      )}
    </>
  )
}

/** Keep project defaults local. Only the explicit Start action saves new content
 * consent. Provider access, saved upload recovery, and acceptance stay separate.
 */
function CloudProjectsAccount({
  session,
  value,
}: {
  session: CloudSession
  value: CloudViewSnapshot
}) {
  const projects = value.projects
  const policy = value.ongoing
  const hasConsent = policy?.consentVersion === "desktop-project-sync-v1"
  const storageUnknown = cloudUploadStatusUnknown(policy)
  const starting = value.sync.status === "starting"
  return (
    <>
      {(hasConsent || storageUnknown) && policy && (
        <div className="flex flex-col items-start gap-2 border-t border-separator px-4 py-3">
          <strong className="type-callout text-label" role="status">
            {starting && !storageUnknown
              ? "Updating upload folders…"
              : cloudOngoingStatusMessage(policy)}
          </strong>
          {policy.storageKnown && (
            <p className="type-footnote text-label-secondary">
              {policy.acceptedCount} uploaded · {policy.queuedCount} waiting ·{" "}
              {policy.interruptedCount} interrupted · {policy.fencedCount} need attention
            </p>
          )}
          {policy.storageKnown && policy.progress.negotiated && (
            <p className="type-footnote text-label-secondary">
              Transferred {byteLabel(policy.progress.confirmedUploadBytes)} of{" "}
              {byteLabel(policy.progress.requiredTransferBytes)} needed. Already saved in Cloud:{" "}
              {byteLabel(policy.progress.reusedBytes)}.
            </p>
          )}
          {storageUnknown && (
            <p className="type-footnote text-label-secondary">
              Upload totals are unavailable. Check disk space and file permissions, then retry
              saved uploads.
            </p>
          )}
          <button
            className="ui-push-button"
            disabled={value.busy || starting}
            onClick={() => void session.controlSync(false)}
          >
            Retry saved uploads
          </button>
        </div>
      )}
      <SettingsRow
        searchId="cloudFolders"
        className="border-t border-separator"
        description={
          projects?.needsChoiceReview
            ? "Review your folder choices once before updating uploads. Existing uploads keep their saved selection. Pause Upload sessions above to stop them."
            : "Choose projects and their current worktrees. Changes apply when you update sync selection. Pause Upload sessions above to stop uploads now."
        }
        trailing={
          <button
            className="ui-push-button"
            disabled={value.busy || starting || !value.cloud.identity || projects?.loading}
            onClick={() => void session.load()}
          >
            Refresh folders
          </button>
        }
      >
        <div className="mt-3 flex flex-col gap-3">
          {projects?.groups.map((group) => (
            <div key={group.id} className="flex items-center justify-between gap-3">
              <div
                className="min-w-0 break-all type-callout text-label"
                title={group.memberPaths.join("\n")}
              >
                <strong>{group.name}</strong>
                <span className="block text-label-secondary">{group.path}</span>
                <span className="block text-label-secondary">
                  {group.sessionCount == null
                    ? "Count unavailable"
                    : `${group.sessionCount} sessions`}
                  {group.worktreeCount > 0 &&
                    ` · ${group.worktreeCount} ${group.worktreeCount === 1 ? "worktree" : "worktrees"}`}
                  {group.captureSupported === false && " · Cloud upload unavailable"}
                </span>
                <div className="text-label-secondary">
                  <CloudProjectPaths name={group.name} paths={group.memberPaths} />
                </div>
              </div>
              <ToggleSwitch
                aria-label={`Sync ${group.name}: ${group.path}`}
                checked={projects.choices[group.id] ?? true}
                disabled={
                  value.busy ||
                  starting ||
                  storageUnknown ||
                  projects.loading ||
                  !!value.cloud.setupError
                }
                onCheckedChange={(selected) => session.selectProject(group.id, selected)}
              />
            </div>
          ))}
          <p className="type-footnote text-label-secondary" role="status">
            {projects?.error ??
              (projects?.loading
                ? "Loading project folders…"
                : projects?.partial
                  ? "Some session folders could not be checked. Review session file access below."
                  : projects?.loaded && !projects.groups.length
                    ? "No project folders found yet. Choose your Claude or OpenCode session files below."
                    : "")}
          </p>
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
      {value.cloud.identity && (
        <>
          {!!projects?.groups.length && (
            <>
              <CloudCaptureConsent
                session={session}
                value={value}
                projects={{
                  groups: projects?.groups ?? [],
                  choices: projects?.choices ?? {},
                  selectionKey: projects?.selectionKey ?? "",
                  loading: !projects?.loaded || projects.loading || starting,
                  error: projects?.error ?? null,
                }}
              />
              <div className="flex flex-col items-start gap-2 px-4 pb-3">
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
                    !value.fullContentConsent ||
                    value.fullContentConsentKey !== projects.selectionKey ||
                    !projects.groups.some(
                      (group) => group.captureSupported && (projects.choices[group.id] ?? true),
                    )
                  }
                  onClick={() => void session.startSync()}
                >
                  {starting
                    ? "Starting…"
                    : hasConsent
                      ? "Update sync selection"
                      : "Start syncing"}
                </button>
                {!hasConsent && (
                  <p className="type-footnote text-label-secondary" role="status">
                    {value.sync.status === "starting"
                      ? "Starting background sync"
                      : policy
                        ? cloudOngoingStatusMessage(policy)
                        : "Cloud sync not started"}
                  </p>
                )}
              </div>
            </>
          )}
          <details
            className="border-t border-separator px-4 py-3 type-callout text-label"
            open={projects?.loaded && projects.groups.length === 0}
          >
            <summary>Session file access</summary>
            <p className="mt-2 type-footnote text-label-secondary">
              Choose where Claude or OpenCode saves sessions if a project is missing. Choosing
              these files does not give permission to upload every project in them.
            </p>
            <div className="mt-3 flex flex-wrap gap-2">
              <button
                className="ui-push-button"
                disabled={value.busy || !!value.cloud.setupError}
                onClick={() => void session.addFolders()}
              >
                Choose Claude session folders…
              </button>
              <button
                className="ui-push-button"
                disabled={value.busy || !!value.cloud.setupError}
                onClick={() => void session.addFolders("opencode_sqlite_v2")}
              >
                Choose OpenCode databases…
              </button>
            </div>
          </details>
          <p className="px-4 pb-3 type-footnote text-label-secondary">
            Signing out disconnects this device only. It does not sign out your browser or
            delete saved uploads.
          </p>
        </>
      )}
    </>
  )
}
