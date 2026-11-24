import { ArrowUpRight, CircleAlert, Folder, LoaderCircle } from "lucide-react"
import { useId, useState, type ReactNode } from "react"
import {
  type CloudSession,
  canBackfillUploads,
  canRetryPersonalUploads,
  cloudGithubSignInLabel,
  cloudStatusMessage,
  cloudUploadStatusUnknown,
  teamFoldersToConfirm,
  type CloudViewSnapshot,
} from "../../onboarding/CloudSession"
import { canRetryTeamUploads } from "../../onboarding/cloudTeamRetry"
import { changeFolderWithFocus } from "../../onboarding/cloudFolderFocus"
import {
  cloudCaptureUnsupported,
  cloudDiscoveryRetryMessage,
} from "../../onboarding/cloudProjectScope"
import { CloudFolderList } from "./CloudFolderList"
import {
  CloudFolderSyncControl,
  CloudProjectFolderSummary,
} from "../../onboarding/CloudOnboardingFolders"
import { CloudInvitations } from "../../onboarding/CloudInvitations"
import {
  cloudAccountCleanupPending,
  cloudInvitationResultWarning,
  cloudSignOutBeforeSignIn,
} from "../../onboarding/cloudSessionState"
import { Skeleton } from "../../components/ui/Skeleton"
import { ToggleSwitch } from "../../components/ui/ToggleSwitch"
import { SettingsRow } from "./SettingsSearchRows"
import { personalCloudDestination } from "../../lib/cloudDestination"
import {
  CLOUD_BUSY_MESSAGE,
  cloudDisplayPath,
  cloudSourceKey,
  cloudSourceKind,
  cloudSourceLabel,
  type CloudOngoingStatus,
  type CloudProjectGroup,
  type CloudWorkspaceRecovery,
} from "../../lib/cloud"

const uploadErrors: Partial<Record<CloudOngoingStatus["status"], string>> = {
  offline: "Cloud is offline. Desktop will try again when it can connect.",
  reauth_required: "Sign in again to resume uploads.",
  upload_unavailable: "Cloud uploads are unavailable.",
  route_denied: "Cloud denied uploads to this workspace. Check your workspace access.",
  fenced: "Some saved uploads are blocked.",
  interrupted: "Some sessions could not be prepared.",
  storage_unavailable: "Could not read saved uploads.",
}
const localRecoveryError = "Some sessions need local recovery. Saved data is kept."
const automaticRecoveryMessage = "Checking saved uploads automatically. Saved files are kept."
const sessionCountFormat = new Intl.NumberFormat()
const uploadPercentFormat = new Intl.NumberFormat(undefined, { style: "percent" })
const unavailableSyncStatus: Partial<Record<CloudOngoingStatus["status"], string>> = {
  offline: "Cloud offline",
  route_denied: "Workspace denied uploads",
  reauth_required: "Sign-in or workspace access required",
  upload_unavailable: "Uploads unavailable",
  storage_unavailable: "Status unavailable",
  fenced: "Folder sync needs attention",
  interrupted: "Folder sync needs attention",
}

/** Manual retry must not interrupt scheduled recovery or substitute for a connection. */
function showSavedUploadRetry(policy: CloudOngoingStatus) {
  return policy.status !== "offline" && !onlyAutomaticRecovery(policy)
}

/** Ignore a transient failure label when Desktop still plans to recover every affected upload. */
function onlyAutomaticRecovery(policy: CloudOngoingStatus) {
  return (
    (policy.automaticRecoveryCount ?? 0) > 0 && !policy.fencedCount && !policy.interruptedCount
  )
}

function savedUploadDisplayState(policy: CloudOngoingStatus) {
  return onlyAutomaticRecovery(policy) &&
    (policy.status === "fenced" || policy.status === "interrupted")
    ? "syncing"
    : policy.status
}

/** Show current work separately from receipt counts for an explicit backfill. */
function DestinationSyncStatus({
  name,
  policy,
  deviceEnabled,
  checkingStatus,
}: {
  name: string
  policy: CloudOngoingStatus | null | undefined
  deviceEnabled: boolean | undefined
  checkingStatus?: boolean | undefined
}) {
  const known = policy?.storageKnown === true
  const enabled =
    policy?.consentVersion === "desktop-project-sync-v1" && policy.projects.length > 0
  let state = "Checking folder sync…"
  if (policy) {
    const paused = deviceEnabled === false || policy.paused
    if (policy.folderConfirmationRequired || policy.sourceConfirmationRequired) {
      state = "Review saved folders"
    } else if (enabled) {
      state = paused ? "Folder sync paused" : "Folder sync enabled"
    } else {
      state =
        known || policy.status === "consent_required"
          ? "Folder sync not set up"
          : "Status unavailable"
    }
    const unavailable = unavailableSyncStatus[savedUploadDisplayState(policy)]
    if (unavailable && state !== "Review saved folders")
      state = enabled && paused ? `${state} · ${unavailable}` : unavailable
    if (
      checkingStatus &&
      policy.status === "storage_unavailable" &&
      state !== "Review saved folders"
    )
      state = "Checking saved uploads…"
  }
  const progress = known && enabled ? policy.uploadProgress : undefined
  const progressLabel = progress
    ? progress.total === null
      ? `Backfill: ${sessionCountFormat.format(progress.uploaded)} accepted`
      : `Backfill: ${sessionCountFormat.format(progress.uploaded)} of ${sessionCountFormat.format(progress.total)} accepted`
    : null
  const jobsNeedingAttention = known ? policy.fencedCount + policy.interruptedCount : 0
  const counts = [
    known && enabled && policy.queuedCount > 0
      ? `${sessionCountFormat.format(policy.queuedCount)} queued`
      : null,
    jobsNeedingAttention > 0
      ? `${sessionCountFormat.format(jobsNeedingAttention)} need attention`
      : null,
    progress?.blocked
      ? `${sessionCountFormat.format(progress.blocked)} blocked in backfill`
      : null,
    progress?.unsupported
      ? `${sessionCountFormat.format(progress.unsupported)} unsupported in backfill`
      : null,
    progress?.scanIncomplete ? "Backfill scan incomplete" : null,
  ].filter(Boolean)
  const automaticRecovery =
    known &&
    enabled &&
    deviceEnabled !== false &&
    !policy.paused &&
    !policy.sourceConfirmationRequired &&
    !policy.folderConfirmationRequired &&
    (policy.automaticRecoveryCount ?? 0) > 0
  const pendingUploads =
    known &&
    enabled &&
    deviceEnabled !== false &&
    policy.status === "syncing" &&
    policy.queuedCount > 0 &&
    !automaticRecovery
  return (
    <div role="group" aria-label={`${name} sync status`} className="min-w-0 py-2">
      <strong className="block type-callout text-label wrap-anywhere">{name}</strong>
      <p className="min-h-8 type-footnote text-label-secondary tabular-nums wrap-anywhere">
        {[state, progressLabel, ...counts].filter(Boolean).join(" · ")}
        {pendingUploads && (
          <>. Desktop checks pending uploads automatically and keeps saved files.</>
        )}
      </p>
      {automaticRecovery && (
        <p className="type-footnote text-label-secondary" role="status">
          {automaticRecoveryMessage}
        </p>
      )}
    </div>
  )
}

/** Announce scan results and completion without reading each upload. */
function backfillAnnouncement(
  name: string,
  progress: NonNullable<CloudOngoingStatus["uploadProgress"]>,
) {
  const { uploaded, total } = progress
  if (total === null) return `${name}: Session total unavailable.`
  if (total === 0) return `${name}: No sessions found for backfill.`
  const count = `${sessionCountFormat.format(total)} ${total === 1 ? "session" : "sessions"}`
  return uploaded === total
    ? `${name}: Backfill complete. ${count} accepted.`
    : `${name}: Scan complete. ${count} found.`
}

/** Show counts without claiming a percentage until discovery has a nonzero total. */
function BackfillUploadProgress({
  name,
  progress: { uploaded, total },
  scanning,
}: {
  name: string
  progress: NonNullable<CloudOngoingStatus["uploadProgress"]>
  scanning: boolean
}) {
  if (total === 0) return <p>No sessions found for backfill.</p>

  const count = sessionCountFormat.format(uploaded)
  if (total === null) {
    return (
      <p className="tabular-nums">
        {count} {uploaded === 1 ? "session" : "sessions"} accepted
        {!scanning && ". Session total unavailable."}
      </p>
    )
  }

  const fraction = uploaded / total
  const rounded = Math.round(fraction * 100)
  const percent =
    uploaded > 0 && rounded === 0
      ? `<${uploadPercentFormat.format(0.01)}`
      : uploaded < total && rounded === 100
        ? `>${uploadPercentFormat.format(0.99)}`
        : uploadPercentFormat.format(fraction)
  const summary = `${count} of ${sessionCountFormat.format(total)} ${total === 1 ? "session" : "sessions"} accepted`
  return (
    <div className="flex flex-col gap-1">
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 tabular-nums">
        <span>{summary}</span>
        <span className="font-medium text-label">{percent}</span>
      </div>
      <div
        role="progressbar"
        aria-label={`Session upload progress for ${name}`}
        aria-valuemin={0}
        aria-valuemax={total}
        aria-valuenow={uploaded}
        aria-valuetext={`${summary} (${percent})`}
        className="ui-progress"
      >
        <div
          className="ui-progress-indicator motion-reduce:transition-none"
          style={{ width: `${fraction * 100}%` }}
        />
      </div>
    </div>
  )
}

/** Give a short warning and its optional action room to wrap independently. */
function CloudFolderWarning({
  id,
  children,
  action,
}: {
  id?: string
  children: ReactNode
  action?: ReactNode
}) {
  return (
    <div className="flex min-h-[var(--control-height-regular)] flex-wrap items-center gap-x-3 gap-y-1.5">
      <p
        id={id}
        role="alert"
        className="flex min-w-0 items-start gap-2 type-footnote text-system-red-text"
      >
        <CircleAlert size={15} className="mt-0.5 shrink-0" aria-hidden="true" />
        <span className="min-w-0 wrap-anywhere">{children}</span>
      </p>
      {action}
    </div>
  )
}

/** Show loading in the same columns as the folder list, without implying a count. */
function CloudFolderLoadingRow({ message }: { message: string }) {
  return (
    <div className="grid grid-cols-[auto_minmax(0,1fr)] items-start gap-x-3 gap-y-2 py-2 @min-[30rem]:grid-cols-[auto_minmax(0,1fr)_minmax(0,12rem)]">
      <Folder size={16} className="mt-0.5 shrink-0 text-label-secondary" aria-hidden />
      <div className="min-w-0">
        <p className="type-callout text-label-secondary" role="status">
          {message}
        </p>
        <Skeleton className="mt-2 h-3 w-32 max-w-full motion-reduce:animate-none" />
      </div>
      <Skeleton className="col-start-2 h-5 w-8 motion-reduce:animate-none @min-[30rem]:col-start-3 @min-[30rem]:justify-self-end" />
    </div>
  )
}

function RetryUploadButton({
  label,
  busy,
  foldersId,
  retry,
}: {
  label: string
  busy: boolean
  foldersId: string
  retry: () => Promise<void>
}) {
  return (
    <button
      type="button"
      className="ui-push-button min-h-8 aria-disabled:opacity-50"
      aria-label={label}
      aria-disabled={busy || undefined}
      onClick={(event) => {
        if (!busy) void changeFolderWithFocus(event.currentTarget, foldersId, undefined, retry)
      }}
    >
      Retry uploads
    </button>
  )
}

/** Render one observed account. Setup errors block source actions, not revocation.
 * Local reset needs a separate confirmation and cannot claim remote deletion.
 */
export function CloudAccount({
  session,
  value,
  children,
  folderRowId,
}: {
  session: CloudSession
  value: CloudViewSnapshot
  /** The caller owns the upload switch and its saved-policy gate. */
  children?: ReactNode
  /** Lets the upload control guide the user to folder setup. */
  folderRowId?: string
}) {
  const [confirmReset, setConfirmReset] = useState(false)
  const [retryingSignInRead, setRetryingSignInRead] = useState(false)
  const waitingForBrowser = value.cloud.status === "opening" || value.cloud.status === "waiting"
  const checkingSignIn = value.cloud.status === "returned"
  const readFailed =
    value.loaded && !!value.error && !value.cloud.identity && !waitingForBrowser
  const checkingAgain = retryingSignInRead && value.busy && !value.cloud.identity
  const checkingAccount = !value.loaded || checkingAgain
  const cleanupPending = cloudAccountCleanupPending(value.cloud)
  const requiresAccountCleanup = cleanupPending && value.cloud.status === "reauth_required"
  const needsSignOut =
    !!value.cloud.identity ||
    cleanupPending ||
    value.cloud.status === "reauth_required" ||
    value.cloud.status === "credential_failed"
  const workspaceCount =
    value.cloud.status === "signed_in" && !value.projects?.error
      ? value.projects?.workspaces?.length
      : undefined
  const accountMessage = readFailed
    ? "Could not check Cloud sign-in."
    : value.error
      ? value.error === CLOUD_BUSY_MESSAGE
        ? "Cloud is busy."
        : value.error
      : waitingForBrowser
        ? cloudStatusMessage("waiting")
        : requiresAccountCleanup
          ? cloudSignOutBeforeSignIn
          : !["idle", "signed_in", "signed_out"].includes(value.cloud.status)
            ? cloudStatusMessage(value.cloud.status)
            : null
  return (
    <>
      <div className="flex items-start justify-between gap-3 px-4 py-3">
        <div className="min-w-0">
          <p
            className="type-headline text-label wrap-anywhere"
            role={checkingAccount ? "status" : undefined}
          >
            {checkingAccount
              ? "Checking Cloud sign-in…"
              : readFailed
                ? "Cloud account"
                : value.cloud.identity
                  ? `Signed in as ${value.cloud.identity.login}`
                  : value.cloud.authEnabled
                    ? "Not signed in"
                    : "Antiburn Cloud is coming soon"}
          </p>
          {value.cloud.identity && workspaceCount !== undefined && (
            <p className="mt-1 type-footnote text-label-secondary">
              {workspaceCount} {workspaceCount === 1 ? "workspace" : "workspaces"}
            </p>
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
        {(readFailed || checkingAgain) && (
          <button
            className="ui-push-button shrink-0"
            disabled={value.busy || retryingSignInRead}
            onClick={() => {
              setRetryingSignInRead(true)
              void session.load().finally(() => setRetryingSignInRead(false))
            }}
          >
            Check again
          </button>
        )}
        {!checkingAccount && !readFailed && !value.cloud.authEnabled && !needsSignOut && (
          <button
            className="inline-flex min-h-10 shrink-0 items-center gap-1 rounded-control type-callout text-label-secondary hover:text-label disabled:opacity-50"
            disabled={value.busy}
            onClick={() => void session.begin()}
          >
            Learn more <ArrowUpRight size={13} aria-hidden="true" />
          </button>
        )}
        {!checkingAccount &&
          value.cloud.authEnabled &&
          !readFailed &&
          !value.cloud.identity &&
          !requiresAccountCleanup &&
          !value.cloud.localCleanupPending &&
          value.cloud.status !== "credential_failed" &&
          !checkingSignIn && (
            <button
              className="ui-push-button shrink-0 aria-disabled:opacity-50"
              aria-disabled={value.busy || undefined}
              onClick={() => {
                if (value.busy) return
                void (waitingForBrowser ? session.cancel() : session.begin())
              }}
            >
              {waitingForBrowser ? "Cancel sign-in" : cloudGithubSignInLabel}
            </button>
          )}
        {needsSignOut && (
          <button
            className="ui-push-button shrink-0"
            disabled={value.busy}
            onClick={() => void session.signOut()}
          >
            {value.cloud.localCleanupPending
              ? "Finish signing out"
              : value.cloud.revocationPending && value.cloud.status !== "reauth_required"
                ? "Retry sign-out"
                : "Sign out"}
          </button>
        )}
      </div>
      {value.cloud.invitationResultUnconfirmed && (
        <p className="px-4 py-3 type-footnote text-label-secondary" role="status">
          {cloudInvitationResultWarning}
        </p>
      )}
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
      {Object.values(value.workspaceConnections ?? {})
        .filter(
          (connection) =>
            connection.localResetAvailable || connection.remoteRevocationUnconfirmed,
        )
        .map((connection) => (
          <TeamConnectionRecovery
            key={`${connection.accountId}:${connection.workspaceId}`}
            connection={connection}
            name={
              value.projects?.workspaces?.find(
                (workspace) => workspace.id === connection.workspaceId,
              )?.name ??
              connection.workspaceName ??
              `team workspace (${connection.workspaceId})`
            }
            busy={value.busy}
            reset={session.resetWorkspace}
          />
        ))}
      {value.cloud.invitationEnabled && value.cloud.identity && (
        <div className="px-4 py-3">
          <CloudInvitations accountId={value.cloud.identity.id} onJoined={session.load} />
        </div>
      )}
      {children && <div className="border-t border-separator">{children}</div>}
      {session.projectSyncSupported ? (
        <CloudProjectsAccount session={session} value={value} folderRowId={folderRowId} />
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
                      {cloudSourceLabel(folder)}: {folder.path}
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

/** Clear only the named legacy connection. Keep the remote warning after local cleanup. */
function TeamConnectionRecovery({
  connection,
  name,
  busy,
  reset,
}: {
  connection: CloudWorkspaceRecovery
  name: string
  busy: boolean
  reset: (workspaceId: string) => Promise<boolean>
}) {
  const [confirm, setConfirm] = useState(false)
  return (
    <div className="flex flex-col items-start gap-2 px-4 py-3">
      {connection.remoteRevocationUnconfirmed ? (
        <p className="type-footnote text-label-secondary" role="status">
          Cloud has not confirmed that the earlier connection for {name} is closed. Close it in
          Cloud settings in your browser.
        </p>
      ) : (
        <p className="type-footnote text-label-secondary" role="status">
          This device still has an old Cloud connection for {name}.
        </p>
      )}
      {connection.localResetAvailable &&
        (confirm ? (
          <>
            <p className="type-callout text-label">
              Clear the old connection for {name} from this device? This does not close it in
              Cloud or change which folders sync.
            </p>
            <div className="flex gap-2">
              <button
                className="ui-push-button"
                disabled={busy}
                onClick={async () => {
                  if (await reset(connection.workspaceId)) setConfirm(false)
                }}
              >
                Confirm cleanup for {name}
              </button>
              <button
                className="ui-push-button"
                disabled={busy}
                onClick={() => setConfirm(false)}
              >
                Cancel
              </button>
            </div>
          </>
        ) : (
          <button className="ui-push-button" disabled={busy} onClick={() => setConfirm(true)}>
            Clear old connection for {name}
          </button>
        ))}
    </div>
  )
}

/** Keep initial choices local until Start. Saved switches revoke immediately or
 * require confirmation before adding content permission.
 */
function CloudProjectsAccount({
  session,
  value,
  folderRowId,
}: {
  session: CloudSession
  value: CloudViewSnapshot
  folderRowId?: string | undefined
}) {
  const fallbackFolderRowId = useId()
  const uploadActionWarningId = useId()
  const foldersId = folderRowId ?? fallbackFolderRowId
  const [saveTeam, setSaveTeam] = useState<{
    accountId: string
    destinationId: string
    revision: string | null
  }>()
  const [startingBackfill, setStartingBackfill] = useState<string>()
  const projects = value.projects
  const policy = value.ongoing
  const identity = value.cloud.identity
  const hasConsent = policy?.consentVersion === "desktop-project-sync-v1"
  const storageUnknown = cloudUploadStatusUnknown(policy)
  const starting = value.sync.status === "starting"
  if (!identity || value.cloud.status !== "signed_in") return null
  if (!personalCloudDestination(value.cloud.destination ?? null)) {
    const isTeam = value.cloud.destination?.kind === "team"
    if (value.error && isTeam) return null
    return (
      <div className="flex flex-wrap items-center gap-2 px-4 py-3">
        {!value.error && (
          <p className="type-footnote text-label-secondary" role="status">
            {isTeam
              ? "Personal uploads are unavailable for this connection."
              : "Could not verify your Cloud workspaces."}
          </p>
        )}
        {!isTeam && (
          <button
            className="ui-push-button"
            disabled={value.busy}
            onClick={() => void session.load()}
          >
            Check again
          </button>
        )}
      </div>
    )
  }
  const folderError = projects?.loading
    ? null
    : projects?.scopeInvalid
      ? projects.error
      : projects?.error || (projects?.loaded && projects.partial && !projects.hasMore)
        ? projects?.groups.length
          ? "Could not check all session folders. Previously loaded folders are still shown."
          : "Could not check session folders."
        : value.cloud.setupError
          ? value.cloud.setupError === cloudCaptureUnsupported
            ? "Uploads are unavailable on this device."
            : "Could not read Cloud folder settings on this device."
          : null
  const recoveryRequiredCount = policy?.recoveryRequiredCount ?? 0
  const interruptedError =
    recoveryRequiredCount > 0 ? localRecoveryError : uploadErrors.interrupted
  const uploadState = policy && savedUploadDisplayState(policy)
  const uploadError = storageUnknown
    ? projects?.loading
      ? null
      : "Could not read saved uploads. Saved files are kept."
    : policy
      ? ((uploadState === "interrupted"
          ? interruptedError
          : uploadState && uploadErrors[uploadState]) ??
        (policy.fencedCount > 0
          ? uploadErrors.fenced
          : policy.interruptedCount > 0
            ? interruptedError
            : null))
      : null
  const canRetryPersonal =
    hasConsent &&
    !!policy.policyRevision &&
    canRetryPersonalUploads(value) &&
    showSavedUploadRetry(policy)
  const needDeviceSwitch =
    !storageUnknown &&
    value.deviceUploadEnabled === false &&
    ((uploadError &&
      policy &&
      !policy.sourceConfirmationRequired &&
      (policy.fencedCount > 0 || policy.interruptedCount > recoveryRequiredCount)) ||
      Object.values(value.teamStatuses ?? {}).some(
        (team) =>
          !team.sourceConfirmationRequired &&
          (team.fencedCount > 0 || team.interruptedCount > (team.recoveryRequiredCount ?? 0)),
      ))
  const empty =
    projects?.loaded &&
    !projects.loading &&
    !projects.error &&
    !projects.hasMore &&
    !projects.partial &&
    !value.cloud.setupError &&
    !value.error &&
    projects.groups.length === 0
  const teamPolicies = Object.entries(value.teamStatuses ?? {})
  const confirmableTeams = teamPolicies.filter(([id]) => teamFoldersToConfirm(value, id))
  const reviewChoice = saveTeam?.accountId === identity.id ? saveTeam : undefined
  const showTeamChoice = confirmableTeams.length > 1 || !!reviewChoice
  const savedTeamToConfirm = showTeamChoice
    ? confirmableTeams.find(
        ([id, team]) =>
          id === reviewChoice?.destinationId && team.policyRevision === reviewChoice.revision,
      )
    : confirmableTeams[0]
  const hasTeamFolders = teamPolicies.some(([, team]) => team.projects.length > 0)
  const hasSavedPolicy = hasConsent || teamPolicies.length > 0
  const multipleWorkspaces = (projects?.workspaces?.length ?? 0) > 1
  const personalSyncNotSetUp =
    policy?.status === "consent_required" &&
    policy.storageKnown &&
    policy.consentVersion === null &&
    !policy.projects.length &&
    !policy.savedFolders?.length &&
    !policy.folderConfirmationRequired &&
    !policy.sourceConfirmationRequired &&
    !policy.fencedCount &&
    !policy.interruptedCount
  const showPersonalSyncStatus =
    !personalSyncNotSetUp || multipleWorkspaces || teamPolicies.length > 0
  const personalId = value.cloud.destination?.id
  const savedTeams = (id: string) =>
    teamPolicies.filter(([, policy]) => session.projectHasSavedPermission(id, policy, true))
  const showsSavedTeamFolder = (
    group: CloudProjectGroup,
    policy: CloudOngoingStatus,
    path: string,
  ) =>
    (group.path === path || group.memberPaths.includes(path)) &&
    session.projectHasSavedPermission(group.id, policy, true)
  const teamName = (id: string, policy: CloudOngoingStatus) =>
    projects?.workspaces?.find((workspace) => workspace.id === id)?.name ??
    policy.workspaceName ??
    `Team workspace (${id})`
  const savedTeamControl = (
    folderId: string,
    label: string,
    team: string,
    teamPolicy: CloudOngoingStatus,
    descriptionId?: string,
    showTeamName = true,
  ) => {
    const controlLabel = `${label} to ${teamName(team, teamPolicy)}`
    return (
      <div key={team} data-cloud-workspace={team} className="flex min-w-0 items-center gap-2">
        {showTeamName && (
          <span className="min-w-0 type-footnote text-label-secondary wrap-anywhere">
            {teamName(team, teamPolicy)}
          </span>
        )}
        <ToggleSwitch
          aria-label={controlLabel}
          aria-describedby={descriptionId}
          checked
          disabled={value.busy || starting || !teamPolicy.policyRevision}
          onCheckedChange={(next) => {
            if (next) return
            const active = document.activeElement
            void changeFolderWithFocus(
              active?.getAttribute("aria-label") === controlLabel ? active : null,
              foldersId,
              folderId,
              () => session.setSavedProject(value, folderId, false, team),
            )
          }}
        />
      </div>
    )
  }
  const waitingForFolders =
    !projects?.loaded && !projects?.error && !projects?.retrying && !value.cloud.setupError
  const exactTeamFolders = teamPolicies.flatMap(([team, policy]) =>
    (waitingForFolders && policy.status !== "reauth_required"
      ? []
      : (policy.savedFolders ?? [])
    )
      .filter(
        (folder) =>
          policy.status === "reauth_required" ||
          !projects?.groups.some((group) => showsSavedTeamFolder(group, policy, folder.path)),
      )
      .map((folder) => ({ ...folder, team, policy })),
  )
  const exactPersonalFolders = (waitingForFolders ? [] : (policy?.savedFolders ?? [])).filter(
    ({ path }) =>
      !projects?.groups.some(
        (group) =>
          (path === group.path || group.memberPaths.includes(path)) &&
          session.projectHasSavedPermission(group.id, undefined, true),
      ),
  )
  const coldFolderLoad =
    (waitingForFolders || (!projects?.loaded && projects?.loading && !projects.error)) &&
    !projects?.groups.length &&
    !exactPersonalFolders.length &&
    !exactTeamFolders.length
  const retryingFolders = projects?.retrying && projects.loading
  const showFolderRetryStatus = retryingFolders && !coldFolderLoad
  const selected = (id: string) =>
    savedTeams(id).length > 0 ||
    session.projectHasSavedPermission(id, undefined, true) ||
    (hasSavedPolicy ? !!projects?.targets?.[id] : (projects?.choices[id] ?? true))
  const draftFolders =
    projects?.groups.filter(
      (group) =>
        group.captureSupported &&
        selected(group.id) &&
        !session.projectHasSavedPermission(group.id, undefined, true) &&
        !savedTeams(group.id).length,
    ) ?? []
  const draftTargets = new Set(
    draftFolders.map(({ id }) => projects?.targets?.[id] ?? personalId),
  )
  const unavailableTarget = [...draftTargets].some(
    (id) =>
      id !== personalId && !projects?.workspaces?.some((workspace) => workspace.id === id),
  )
  const confirmTeam = draftFolders.length
    ? draftTargets.size === 1
      ? confirmableTeams.find(([id]) => draftTargets.has(id))
      : undefined
    : savedTeamToConfirm
  const startableFolder =
    draftFolders.length > 0 && draftTargets.size === 1 && !unavailableTarget
  const savedSourceToReview =
    !draftFolders.length &&
    !confirmTeam &&
    policy?.sourceConfirmationRequired &&
    projects?.groups.find((group) => session.projectHasReviewableSource(group.id))
  const canSave =
    (!!confirmTeam && !draftFolders.length) ||
    (!storageUnknown &&
      value.cloud.captureEnabled &&
      !value.cloud.setupError &&
      projects?.loaded &&
      !projects.loading &&
      !projects.error &&
      (startableFolder || !!savedSourceToReview))
  const hasError = value.uploadActionError || folderError || uploadError
  const backfillDestinations = [
    ...(personalId && (canBackfillUploads(value) || policy?.backfillInProgress)
      ? [[personalId, "Personal", undefined, policy] as const]
      : []),
    ...teamPolicies
      .filter(([id, team]) => canBackfillUploads(value, id) || team.backfillInProgress)
      .map(([id, team]) => [id, teamName(id, team), id, team] as const),
  ]
  const focusUploadSwitch = () => {
    const folders = document.getElementById(foldersId)
    const switchControl = folders?.parentElement?.querySelector<HTMLElement>(
      '[data-settings-control="cloudUpload"] [role="switch"]',
    )
    switchControl?.focus()
  }
  return (
    <>
      <SettingsRow
        searchId="cloudFolders"
        id={foldersId}
        role="group"
        aria-label="Cloud folders"
        className="@container relative border-t border-separator focus:outline-2 focus:-outline-offset-2 focus:outline-accent-fill"
        description="Uploads include full sessions from approved folders and worktrees. Worktrees outside approved folders need approval."
      >
        {projects?.loaded && projects.loading && !projects.retrying && (
          <span role="status" className="absolute end-4 top-3 text-label-secondary">
            <LoaderCircle size={13} className="motion-safe:animate-spin" aria-hidden />
            <span className="sr-only">Checking folders…</span>
          </span>
        )}
        <div className="grid min-h-32 @min-[30rem]:min-h-24">
          <CloudFolderList groups={projects?.groups}>
            {projects?.groups.map((group) => {
              const teams = savedTeams(group.id)
              const displayedTeams = teams.filter(
                ([, team]) =>
                  !team.savedFolders ||
                  (team.status !== "reauth_required" &&
                    team.savedFolders.some((folder) =>
                      showsSavedTeamFolder(group, team, folder.path),
                    )),
              )
              const savedPersonal = session.projectHasSavedPermission(group.id, undefined, true)
              const unverifiedGroup = group.discoveryStale || group.groupingComplete === false
              const showChoice = teams.length === 0 || savedPersonal
              if (!showChoice && displayedTeams.length === 0) return null
              const showPersonal = savedPersonal && (teams.length > 0 || multipleWorkspaces)
              const draftTarget = projects.targets?.[group.id]
              const pickWorkspace =
                !savedPersonal &&
                (multipleWorkspaces || (!!draftTarget && draftTarget !== personalId))
              const label = `Sync ${group.name}: ${group.path}`
              const controlLabel = showPersonal ? `${label} to Personal` : label
              const descriptionId = `cloud-settings-folder-${encodeURIComponent(group.id)}`
              return (
                <div
                  key={group.id}
                  data-cloud-folder={group.id}
                  className="grid grid-cols-[auto_minmax(0,1fr)] items-start gap-x-3 gap-y-2 py-2 @min-[30rem]:grid-cols-[auto_minmax(0,1fr)_minmax(0,12rem)]"
                >
                  <Folder
                    size={16}
                    className="mt-0.5 shrink-0 text-label-secondary"
                    aria-hidden
                  />
                  <div className="min-w-0 type-callout text-label">
                    <strong className="block wrap-anywhere">{group.name}</strong>
                    <CloudProjectFolderSummary
                      id={descriptionId}
                      group={group}
                      homeDirectory={value.homeDirectory}
                      stackPath
                    />
                  </div>
                  <div className="col-start-2 flex min-w-0 max-w-full flex-col items-start gap-2 @min-[30rem]:col-start-3 @min-[30rem]:items-end">
                    {displayedTeams.map(([team, teamPolicy]) =>
                      savedTeamControl(group.id, label, team, teamPolicy, descriptionId),
                    )}
                    {showChoice && (
                      <div
                        data-cloud-workspace={savedPersonal ? personalId : undefined}
                        className="flex min-w-0 max-w-full items-center gap-2"
                      >
                        {showPersonal && (
                          <span className="type-footnote text-label-secondary">Personal</span>
                        )}
                        <CloudFolderSyncControl
                          label={controlLabel}
                          descriptionId={descriptionId}
                          selected={savedPersonal || (!unverifiedGroup && selected(group.id))}
                          typedWorkspaceLabels
                          wrapWorkspaceLabel
                          workspaces={pickWorkspace ? projects.workspaces : undefined}
                          destinationId={personalId}
                          selectedWorkspace={pickWorkspace ? draftTarget : undefined}
                          disabled={
                            value.busy ||
                            starting ||
                            storageUnknown ||
                            (unverifiedGroup && !savedPersonal) ||
                            (!selected(group.id) &&
                              (!!value.cloud.setupError ||
                                !!projects.error ||
                                !group.captureSupported))
                          }
                          onSelect={(next, workspace) => {
                            if (savedPersonal) {
                              const active = document.activeElement
                              void changeFolderWithFocus(
                                active?.getAttribute("aria-label") === controlLabel
                                  ? active
                                  : null,
                                foldersId,
                                group.id,
                                () => session.setSavedProject(value, group.id, next),
                              )
                            } else
                              session.selectProject(
                                group.id,
                                next,
                                next ? (workspace ?? personalId) : undefined,
                                true,
                              )
                          }}
                        />
                      </div>
                    )}
                  </div>
                </div>
              )
            })}
            {exactPersonalFolders.map(({ groupId, path }) => (
              <div
                key={`personal:${groupId}`}
                data-exact-folder={path}
                data-cloud-workspace={personalId}
                className="flex items-center gap-3 py-2"
              >
                <Folder size={16} className="shrink-0 text-label-secondary" aria-hidden />
                <span
                  className="min-w-0 flex-1 type-callout text-label wrap-anywhere"
                  title={path}
                >
                  {cloudDisplayPath(path, value.homeDirectory)}
                </span>
                {hasTeamFolders && (
                  <span className="type-footnote text-label-secondary">Personal</span>
                )}
                <ToggleSwitch
                  aria-label={`Sync ${path} to Personal`}
                  checked
                  disabled={value.busy || starting || storageUnknown || !policy?.policyRevision}
                  onCheckedChange={(next) => {
                    if (next) return
                    void changeFolderWithFocus(
                      document.activeElement,
                      foldersId,
                      undefined,
                      () => session.setSavedProject(value, groupId, false),
                    )
                  }}
                />
              </div>
            ))}
            {exactTeamFolders.map(({ groupId, path, team, policy }) => (
              <div
                key={`${team}:${groupId}`}
                data-exact-folder={path}
                data-cloud-workspace={team}
                className="grid grid-cols-[auto_minmax(0,1fr)] items-start gap-x-3 gap-y-2 py-2 @min-[30rem]:grid-cols-[auto_minmax(0,1fr)_minmax(0,12rem)]"
              >
                <Folder
                  size={16}
                  className="mt-0.5 shrink-0 text-label-secondary"
                  aria-hidden
                />
                <span className="min-w-0 type-callout text-label wrap-anywhere" title={path}>
                  {cloudDisplayPath(path, value.homeDirectory)}
                </span>
                <div className="col-start-2 min-w-0 @min-[30rem]:col-start-3 @min-[30rem]:justify-self-end">
                  {savedTeamControl(groupId, `Sync ${path}`, team, policy)}
                </div>
              </div>
            ))}
            {coldFolderLoad && (
              <CloudFolderLoadingRow
                message={
                  retryingFolders
                    ? cloudDiscoveryRetryMessage(projects.retryStage)
                    : "Loading folders…"
                }
              />
            )}
            {empty && exactTeamFolders.length === 0 && exactPersonalFolders.length === 0 && (
              <div className="flex flex-wrap items-center gap-2 py-2">
                <p className="type-footnote text-label-secondary">
                  No session folders found. Start a session in a supported coding tool.
                </p>
              </div>
            )}
          </CloudFolderList>
        </div>
        {projects?.hasMore && (
          <button
            className="ui-push-button mt-3"
            aria-disabled={
              value.busy ||
              starting ||
              projects.loading ||
              !!projects.paging ||
              projects.scopeInvalid ||
              undefined
            }
            onClick={(event) => {
              if (
                value.busy ||
                starting ||
                projects.loading ||
                projects.paging ||
                projects.scopeInvalid
              )
                return
              void changeFolderWithFocus(
                event.currentTarget,
                foldersId,
                undefined,
                session.loadMoreProjects,
              )
            }}
          >
            Load more folders
          </button>
        )}
      </SettingsRow>
      <footer className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-x-3 gap-y-2 border-t border-separator px-4 py-2">
        <div className="flex min-h-[var(--control-height-regular)] min-w-0 flex-col gap-1.5 type-footnote text-label-secondary wrap-anywhere">
          {value.uploadActionError && (
            <CloudFolderWarning id={uploadActionWarningId}>
              {value.uploadActionError}
            </CloudFolderWarning>
          )}
          {folderError && <CloudFolderWarning>{folderError}</CloudFolderWarning>}
          {uploadError && (
            <CloudFolderWarning
              action={
                !storageUnknown && canRetryPersonal ? (
                  <RetryUploadButton
                    label={
                      hasTeamFolders ? "Retry saved uploads to Personal" : "Retry saved uploads"
                    }
                    busy={value.busy || starting}
                    foldersId={foldersId}
                    retry={() => session.retrySync(value)}
                  />
                ) : null
              }
            >
              {hasTeamFolders ? `Personal: ${uploadError}` : uploadError}
            </CloudFolderWarning>
          )}
          {teamPolicies.map(([id, team]) => {
            if (!team.projects.length) return null
            if (projects?.loading && team.status === "storage_unavailable") return null
            const unavailable =
              projects?.workspaces &&
              !projects.workspaces.some(({ id: workspaceId }) => workspaceId === id)
            const failure =
              team.status === "reauth_required"
                ? unavailable
                  ? "Uploads are unavailable."
                  : null
                : (team.recoveryRequiredCount ?? 0) > 0
                  ? localRecoveryError
                  : (uploadErrors[savedUploadDisplayState(team)] ??
                    (team.fencedCount > 0
                      ? uploadErrors.fenced
                      : team.interruptedCount > 0
                        ? uploadErrors.interrupted
                        : null))
            const retry = canRetryTeamUploads(value, id) && showSavedUploadRetry(team)
            if (!failure && !retry) return null
            return (
              <CloudFolderWarning
                key={id}
                action={
                  retry ? (
                    <RetryUploadButton
                      label={`Retry saved uploads to ${teamName(id, team)}`}
                      busy={value.busy || starting}
                      foldersId={foldersId}
                      retry={() => session.retryTeamSync(value, id)}
                    />
                  ) : undefined
                }
              >
                {teamName(id, team)}: {failure ?? "Some saved uploads need attention."}
              </CloudFolderWarning>
            )
          })}
          {needDeviceSwitch && (
            <button
              type="button"
              className="self-start underline underline-offset-4"
              onClick={focusUploadSwitch}
            >
              Go to upload switch
            </button>
          )}
          {!draftFolders.length && showTeamChoice && confirmableTeams.length > 0 && (
            <label className="flex flex-wrap items-center gap-1">
              Save folders for
              <select
                className="max-w-full rounded-control bg-surface-card text-label"
                disabled={value.busy || starting}
                value={savedTeamToConfirm?.[0] ?? ""}
                onChange={(event) => {
                  const choice = confirmableTeams.find(([id]) => id === event.target.value)
                  setSaveTeam({
                    accountId: identity.id,
                    destinationId: choice?.[0] ?? "",
                    revision: choice?.[1].policyRevision ?? null,
                  })
                }}
              >
                <option value="">Choose Team</option>
                {confirmableTeams.map(([id, team]) => (
                  <option key={id} value={id}>
                    {teamName(id, team)}
                  </option>
                ))}
              </select>
            </label>
          )}
          {confirmTeam && !value.uploadActionError ? (
            <p>
              {draftFolders.length
                ? "Save listed and new Team folders."
                : "Save the listed Team folders."}
            </p>
          ) : unavailableTarget ? (
            <p>
              A selected workspace is unavailable. Choose another destination or turn off its
              folder.
            </p>
          ) : draftTargets.size > 1 ? (
            <p>Choose one workspace at a time.</p>
          ) : null}
          {!hasError &&
            (policy?.sourceConfirmationRequired ||
              teamPolicies.some(([, team]) => team.sourceConfirmationRequired)) && (
              <p role="status">
                {savedSourceToReview || draftFolders.length ? (
                  "Save to check changed sources."
                ) : (
                  <button
                    type="button"
                    className="underline underline-offset-4"
                    aria-description="Restore access to saved sources, or review the folders that use them."
                    onClick={() => document.getElementById(foldersId)?.focus()}
                  >
                    Review folders with changed sources
                  </button>
                )}
              </p>
            )}
          {backfillDestinations.length > 0 && (
            <div className="flex max-w-sm flex-col gap-3">
              {backfillDestinations.map(([id, name, teamId, destinationPolicy]) => {
                const current =
                  value.deviceUploadEnabled !== false &&
                  destinationPolicy?.storageKnown === true
                const scanning = current && destinationPolicy?.backfillInProgress === true
                const waiting = startingBackfill === id && !scanning && current
                const active = waiting || scanning
                const showButton = !draftFolders.length || active
                const progress = current && !waiting ? destinationPolicy?.uploadProgress : null
                if (!showButton && !progress) return null
                const disabled =
                  value.busy || starting || active || !canBackfillUploads(value, teamId)
                const label =
                  teamId || hasTeamFolders ? `Backfill sessions to ${name}` : undefined
                return (
                  <div key={id} className="flex min-w-0 flex-col gap-1.5">
                    {(label || !showButton) && (
                      <p className="font-medium text-label wrap-anywhere">{name}</p>
                    )}
                    <div
                      className={
                        showButton ? "flex flex-wrap items-center gap-x-2 gap-y-1" : "sr-only"
                      }
                    >
                      {showButton && (
                        <button
                          type="button"
                          className="ui-push-button h-auto! min-h-8 max-w-full shrink-0 text-left wrap-anywhere aria-disabled:opacity-60"
                          aria-label={label}
                          aria-disabled={disabled || undefined}
                          aria-busy={active || undefined}
                          aria-description="Send all available sessions in saved folders again, including sessions previously uploaded."
                          onClick={async () => {
                            if (disabled) return
                            setStartingBackfill(id)
                            try {
                              await session.backfillSync(value, teamId)
                            } finally {
                              setStartingBackfill(undefined)
                            }
                          }}
                        >
                          Backfill sessions
                        </button>
                      )}
                      <span
                        role="status"
                        className={active ? "flex items-center gap-1" : "sr-only"}
                      >
                        {active ? (
                          <>
                            <LoaderCircle
                              size={13}
                              className="shrink-0 motion-safe:animate-spin"
                              aria-hidden="true"
                            />
                            {label && <span className="sr-only">{name}: </span>}
                            {waiting ? "Waiting for backfill to start…" : "Scanning folders…"}
                          </>
                        ) : (
                          progress && backfillAnnouncement(name, progress)
                        )}
                      </span>
                    </div>
                    {progress && (
                      <BackfillUploadProgress
                        name={name}
                        progress={progress}
                        scanning={scanning}
                      />
                    )}
                  </div>
                )
              })}
            </div>
          )}
        </div>
        <button
          className="ui-push-button enabled:bg-accent-fill enabled:text-white enabled:hover:bg-accent-hover disabled:opacity-50 aria-disabled:opacity-50"
          disabled={!value.busy && !starting && !projects?.loading && !canSave}
          aria-disabled={value.busy || starting || (projects?.loading && !canSave) || undefined}
          aria-description={
            confirmTeam
              ? `Save full-session uploads for the exact listed paths in ${teamName(...confirmTeam)}.`
              : `Save full-session uploads for the selected folder and worktree paths for ${identity.login}.`
          }
          onClick={async (event) => {
            if (value.busy || starting || !canSave) return
            const save = event.currentTarget
            const focus = new AbortController()
            const stopFollowingSave = () => focus.abort()
            if (document.activeElement === save) {
              save.addEventListener("blur", stopFollowingSave, { signal: focus.signal })
              window.addEventListener("blur", stopFollowingSave, { signal: focus.signal })
            } else stopFollowingSave()
            try {
              await changeFolderWithFocus(
                save,
                foldersId,
                draftFolders.length === 1 ? draftFolders[0]?.id : undefined,
                () =>
                  session.startPendingProjects(
                    value,
                    hasSavedPolicy,
                    confirmTeam?.[0],
                    savedSourceToReview ? savedSourceToReview.id : undefined,
                  ),
              )
              if (
                !focus.signal.aborted &&
                document.activeElement === save &&
                session.getSnapshot().uploadActionError
              )
                document.getElementById(uploadActionWarningId)?.scrollIntoView?.({
                  block: "nearest",
                })
            } finally {
              stopFollowingSave()
            }
          }}
        >
          Save
        </button>
      </footer>
      {(showPersonalSyncStatus || showFolderRetryStatus || projects?.paging) && (
        <section aria-label="Cloud sync status" className="border-t border-separator px-4">
          {showFolderRetryStatus && (
            <p className="pt-2 type-footnote text-label-secondary" role="status">
              {cloudDiscoveryRetryMessage(projects.retryStage)}
            </p>
          )}
          {projects?.paging && (
            <p className="pt-2 type-footnote text-label-secondary" role="status">
              {projects.paging === "retrying"
                ? "Still checking for more folders."
                : "Loading more folders…"}
            </p>
          )}
          {showPersonalSyncStatus && (
            <DestinationSyncStatus
              name="Personal"
              policy={policy}
              deviceEnabled={value.deviceUploadEnabled}
              checkingStatus={projects?.loading}
            />
          )}
          {teamPolicies.map(([id, team]) => (
            <DestinationSyncStatus
              key={id}
              name={teamName(id, team)}
              policy={team}
              deviceEnabled={value.deviceUploadEnabled}
              checkingStatus={projects?.loading}
            />
          ))}
        </section>
      )}
    </>
  )
}
