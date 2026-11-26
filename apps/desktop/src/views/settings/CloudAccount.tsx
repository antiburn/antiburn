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
import { DeviceAccessUpdate } from "../../onboarding/DeviceAccessUpdate"
import { changeFolderWithFocus } from "../../onboarding/cloudFolderFocus"
import {
  cloudCaptureUnsupported,
  cloudDiscoveryRetryMessage,
} from "../../onboarding/cloudProjectScope"
import { CloudFolderList } from "./CloudFolderList"
import {
  currentFolderFound,
  folderSyncFoundCount,
  idleSyncMessage,
  onlyAutomaticRecovery,
  savedUploadDisplayState,
  syncActivityReported,
  syncingSessionsMessage,
} from "./CloudDestinationSyncStatus"
import {
  currentUploadProgressMessage,
  savedUploadProgressMessage,
  CloudSyncActivity,
} from "./CloudSyncActivity"
import {
  CloudFolderSyncControl,
  CloudProjectFolderSummary,
  folderSessionsFoundLabel,
} from "../../onboarding/CloudOnboardingFolders"
import {
  cloudAccountCleanupPending,
  cloudSignOutBeforeSignIn,
  confirmationContextKey,
  folderSaveRecovery,
  matchesUploadPolicy,
  uploadPolicy,
} from "../../onboarding/cloudSessionState"
import { Skeleton } from "../../components/ui/Skeleton"
import { ToggleSwitch } from "../../components/ui/ToggleSwitch"
import { SettingsRow } from "./SettingsSearchRows"
import { personalCloudDestination } from "../../lib/cloudDestination"
import {
  CLOUD_BUSY_MESSAGE,
  cloudDisplayPath,
  type CloudOngoingStatus,
  type CloudProjectGroup,
  type CloudWorkspaceRecovery,
} from "../../lib/cloud"

const uploadErrors: Partial<Record<CloudOngoingStatus["status"], string>> = {
  offline: "Cloud is offline. Saved uploads are kept.",
  reauth_required: "Sign in again to resume uploads.",
  upload_unavailable: "Cloud uploads are unavailable.",
  route_denied: "Cloud denied uploads to this workspace. Check your workspace access.",
  fenced: "Some saved uploads are blocked.",
  interrupted: "Some sessions could not be prepared.",
  storage_unavailable: "Could not read saved uploads.",
}
const localRecoveryError = "Some sessions need local recovery. Saved data is kept."

/** Manual retry must not interrupt scheduled recovery or substitute for a connection. */
function showSavedUploadRetry(policy: CloudOngoingStatus) {
  return policy.status !== "offline" && !onlyAutomaticRecovery(policy)
}

/** A manual sync only scans saved folders in eligible workspaces. */
function canSyncSavedFolders(value: CloudViewSnapshot) {
  return [undefined, ...Object.keys(value.teamStatuses ?? {})].some((teamId) =>
    canBackfillUploads(value, teamId),
  )
}

/** Explain why a folder has no editable sync control. */
function FolderUploadState({ children }: { children: ReactNode }) {
  return (
    <span className="min-w-0 type-footnote text-label-secondary wrap-anywhere">{children}</span>
  )
}

/** Keep counts next to the folder they describe, including saved rules outside discovery. */
function SavedFolderSummary({
  path,
  homeDirectory,
  found,
}: {
  path: string
  homeDirectory: string | null
  found: number | undefined
}) {
  return (
    <div className="min-w-0 flex-1 type-callout text-label wrap-anywhere">
      <span title={path}>{cloudDisplayPath(path, homeDirectory)}</span>
      {found !== undefined && (
        <p className="type-footnote text-label-secondary">{folderSessionsFoundLabel(found)}</p>
      )}
    </div>
  )
}

/** Explain why a discovered folder cannot add upload permission. */
function folderUploadUnavailable(group: CloudProjectGroup) {
  if (group.discoveryStale) return "Folder details unavailable"
  if (group.groupingComplete === false) return "Could not check this folder"
  if (!group.captureSupported) return "No sessions ready to upload"
  return null
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
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
      <p
        id={id}
        role="alert"
        className="flex min-h-8 min-w-0 items-center type-footnote text-system-red-text"
      >
        <span className="flex min-w-0 items-start gap-2">
          <span className="flex h-[1lh] shrink-0 items-center" aria-hidden="true">
            <CircleAlert size={15} />
          </span>
          <span className="min-w-0 wrap-anywhere">{children}</span>
        </span>
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

/** Reserve Settings rows until their first read settles. Do not show guessed controls. */
function CloudSettingsLoading({
  account,
  uploadControl,
  folderRowId,
}: {
  account: boolean
  uploadControl: boolean
  folderRowId: string | undefined
}) {
  return (
    <>
      <div aria-busy="true">
        {account && (
          <div className="flex items-start justify-between gap-3 px-4 py-3">
            <div className="min-w-0 space-y-2">
              <Skeleton className="h-4 w-40 max-w-full motion-reduce:animate-none" />
              <Skeleton className="h-3 w-20 motion-reduce:animate-none" />
            </div>
            <Skeleton className="h-6 w-16 shrink-0 motion-reduce:animate-none" />
          </div>
        )}
        {uploadControl && (
          <SettingsRow
            searchId="cloudUpload"
            className="border-t border-separator"
            trailing={<Skeleton className="h-5 w-8 motion-reduce:animate-none" />}
          />
        )}
        <SettingsRow
          searchId="cloudFolders"
          id={folderRowId}
          role="group"
          aria-label="Cloud folders"
          className="@container border-t border-separator"
        >
          <Skeleton className="mt-1 h-3 w-64 max-w-full motion-reduce:animate-none" />
          <div className="mt-3 grid min-h-32 @min-[30rem]:min-h-24">
            {["w-32", "w-24"].map((width) => (
              <div
                key={width}
                className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-start gap-3 py-2"
              >
                <Skeleton className="h-4 w-4 motion-reduce:animate-none" />
                <div className="min-w-0 space-y-2">
                  <Skeleton className={`h-3 max-w-full motion-reduce:animate-none ${width}`} />
                  <Skeleton className="h-3 w-40 max-w-full motion-reduce:animate-none" />
                </div>
                <Skeleton className="h-5 w-8 motion-reduce:animate-none" />
              </div>
            ))}
          </div>
        </SettingsRow>
      </div>
      <footer className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-x-4 border-t border-separator px-4 py-2 type-footnote text-label-secondary">
        <p role="status" className="flex min-h-8 items-center">
          Loading Cloud settings…
        </p>
        <button
          type="button"
          className="ui-push-button min-h-8 min-w-28 disabled:bg-surface-tertiary disabled:text-label-secondary"
          disabled
        >
          Sync now
        </button>
      </footer>
    </>
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
  uploadSwitchRowId,
}: {
  session: CloudSession
  value: CloudViewSnapshot
  /** The caller owns the upload switch and its saved-policy gate. */
  children?: ReactNode
  /** Lets the upload control guide the user to folder setup. */
  folderRowId?: string
  /** The row ID when the caller renders an upload switch. */
  uploadSwitchRowId?: string | undefined
}) {
  const [confirmReset, setConfirmReset] = useState(false)
  const [retryingSignInRead, setRetryingSignInRead] = useState(false)
  const [refreshingWorkspaces, setRefreshingWorkspaces] = useState(false)
  const [openingBrowser, setOpeningBrowser] = useState(false)
  const [browserOpenError, setBrowserOpenError] = useState<string | null>(null)
  const accessUpdateWaiting = value.cloud.accessUpdate === "waiting"
  const waitingForBrowser = value.cloud.status === "opening" || value.cloud.status === "waiting"
  const checkingSignIn = value.cloud.status === "returned"
  const readFailed =
    value.loaded && value.accountReadFailed && !value.cloud.identity && !waitingForBrowser
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
  const canRefreshWorkspaces =
    value.cloud.status === "signed_in" &&
    !!value.cloud.identity &&
    personalCloudDestination(value.cloud.destination ?? null)
  const browserActions = [
    {
      label: "Finish setup in browser",
      available: session.canOpenSetup(),
      open: session.openSetup,
      error: "Could not open Cloud setup in your browser. Try again.",
    },
    {
      label: "Open Cloud dashboard",
      available: value.cloud.dashboardAvailable,
      open: session.openDashboard,
      error: "Could not open the Cloud dashboard. Try again.",
    },
  ]
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
  if (!value.loaded)
    return <CloudSettingsLoading account uploadControl={!!children} folderRowId={folderRowId} />
  const loadingSyncStatus =
    value.cloud.status === "signed_in" &&
    personalCloudDestination(value.cloud.destination ?? null) &&
    !value.ongoing &&
    !value.projects?.loaded &&
    !value.projects?.error &&
    !value.projects?.refreshError &&
    !value.cloud.setupError &&
    !value.error
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
          {value.cloud.identity && (workspaceCount !== undefined || canRefreshWorkspaces) && (
            <div className="mt-1 flex flex-wrap items-center gap-x-3 type-footnote text-label-secondary">
              {workspaceCount !== undefined && (
                <p aria-live="polite">
                  {workspaceCount} {workspaceCount === 1 ? "workspace" : "workspaces"}
                </p>
              )}
              {canRefreshWorkspaces && (
                <button
                  type="button"
                  className="rounded-control border-0 bg-transparent p-0 underline underline-offset-2 hover:text-label aria-disabled:opacity-50"
                  aria-disabled={value.busy || refreshingWorkspaces || undefined}
                  aria-busy={refreshingWorkspaces || undefined}
                  onClick={() => {
                    if (value.busy || refreshingWorkspaces) return
                    setRefreshingWorkspaces(true)
                    void session
                      .retryFolderScope()
                      .finally(() => setRefreshingWorkspaces(false))
                  }}
                >
                  Refresh workspaces
                </button>
              )}
              {canRefreshWorkspaces &&
                browserActions
                  .filter(({ available }) => available)
                  .map(({ label, open, error }) => (
                    <button
                      key={label}
                      type="button"
                      className="inline-flex items-center gap-1 rounded-control border-0 bg-transparent p-0 underline underline-offset-2 hover:text-label aria-disabled:opacity-50"
                      aria-disabled={openingBrowser || undefined}
                      onClick={() => {
                        if (openingBrowser) return
                        setOpeningBrowser(true)
                        setBrowserOpenError(null)
                        void open()
                          .catch(() => setBrowserOpenError(error))
                          .finally(() => setOpeningBrowser(false))
                      }}
                    >
                      {label} <ArrowUpRight size={13} aria-hidden="true" />
                    </button>
                  ))}
            </div>
          )}
          {canRefreshWorkspaces && browserOpenError && (
            <p className="type-footnote text-system-red-text" role="alert">
              {browserOpenError}
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
          !accessUpdateWaiting &&
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
      {(value.cloud.status === "signed_in" || accessUpdateWaiting) && (
        <DeviceAccessUpdate
          value={value.cloud}
          busy={value.busy}
          update={session.begin}
          cancel={session.cancel}
        />
      )}
      {value.cloud.localResetAvailable && (
        <div className="flex flex-col gap-3 px-4 py-3 type-callout text-label">
          {confirmReset ? (
            <>
              <p>Reset sign-in on this device? This clears local sign-in only.</p>
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
        .filter((connection) => connection.localResetAvailable)
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
      {!loadingSyncStatus && children && (
        <div className="border-t border-separator">{children}</div>
      )}
      {loadingSyncStatus ? (
        <CloudSettingsLoading
          account={false}
          uploadControl={!!children}
          folderRowId={folderRowId}
        />
      ) : (
        <CloudProjectsAccount
          session={session}
          value={value}
          folderRowId={folderRowId}
          uploadSwitchRowId={uploadSwitchRowId}
        />
      )}
    </>
  )
}

/** Clear the named legacy connection when it still needs local cleanup. */
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
      {confirm ? (
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
      )}
    </div>
  )
}

/** Keep new folders and workspace changes local until the primary sync action.
 * A saved switch can turn off uploads immediately.
 */
function CloudProjectsAccount({
  session,
  value,
  folderRowId,
  uploadSwitchRowId,
}: {
  session: CloudSession
  value: CloudViewSnapshot
  folderRowId?: string | undefined
  uploadSwitchRowId?: string | undefined
}) {
  const fallbackFolderRowId = useId()
  const uploadActionWarningId = useId()
  const folderWarningId = useId()
  const foldersId = folderRowId ?? fallbackFolderRowId
  const [saveTeam, setSaveTeam] = useState<{
    accountId: string
    destinationId: string
    revision: string | null
  }>()
  const [syncPhase, setSyncPhase] = useState<"saving" | "starting">()
  const [folderMove, setFolderMove] = useState<{
    accountId: string
    groupId: string
    confirmationKey: CloudProjectGroup["confirmationKey"]
    from: string
    to: string | undefined
    sourceRevision: string
  }>()
  const projects = value.projects
  const policy = value.ongoing
  const identity = value.cloud.identity
  const hasConsent = policy?.consentVersion === "desktop-project-sync-v1"
  const storageUnknown = cloudUploadStatusUnknown(policy)
  const starting = value.sync.status === "starting"
  const syncPending = starting || !!syncPhase
  const syncActionBusy = value.busy || syncPending
  if (!identity || value.cloud.status !== "signed_in") return null
  const accountId = identity.id
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
  const recoveryRequiredCount = policy?.recoveryRequiredCount ?? 0
  const interruptedError =
    recoveryRequiredCount > 0 ? localRecoveryError : uploadErrors.interrupted
  const uploadState = policy && savedUploadDisplayState(policy)
  const checkingUploadStatus =
    projects?.loading ||
    (projects?.retrying && (!projects.retryStage || projects.retryStage === "setup"))
  const uploadError = storageUnknown
    ? checkingUploadStatus
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
    !!uploadSwitchRowId &&
    !syncActionBusy &&
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
  const visibleGroups = projects?.groups.filter((group) => group.memberPaths.length > 0)
  const empty =
    projects?.loaded &&
    !projects.loading &&
    !projects.error &&
    !projects.refreshError &&
    !projects.scopeInvalid &&
    !projects.hasMore &&
    !projects.partial &&
    !value.cloud.setupError &&
    !value.error &&
    !visibleGroups?.length
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
  const savedTeamPolicies = teamPolicies.filter(
    ([, team]) => team.projects.length > 0 || !!team.savedFolders?.length,
  )
  const hasTeamFolders = savedTeamPolicies.length > 0
  const hasSavedFolders =
    hasTeamFolders || !!policy?.projects.length || !!policy?.savedFolders?.length
  const hasSavedPolicy = hasConsent || teamPolicies.length > 0
  const multipleWorkspaces = (projects?.workspaces?.length ?? 0) > 1
  const personalId = value.cloud.destination?.id
  const savedTeams = (id: string) =>
    teamPolicies.filter(([, policy]) => session.projectHasSavedPermission(id, policy, true))
  const moveGroup = projects?.groups.find(
    (group) =>
      folderMove?.accountId === identity.id &&
      folderMove.groupId === group.id &&
      folderMove.confirmationKey === group.confirmationKey &&
      !group.discoveryStale &&
      group.groupingComplete !== false &&
      projects.workspaces?.some(({ id }) => id === folderMove.from),
  )
  const move =
    moveGroup &&
    folderMove &&
    (folderMove.from === personalId
      ? session.projectHasSavedPermission(moveGroup.id, undefined, true)
      : savedTeams(moveGroup.id).some(([id]) => id === folderMove.from))
      ? folderMove
      : undefined
  const moveTargetUnavailable =
    !!move?.to && !projects?.workspaces?.some(({ id }) => id === move.to)
  const moveSourcePolicy = (snapshot: CloudViewSnapshot, from: string) =>
    uploadPolicy(snapshot, from === personalId ? undefined : from)
  const moveSourceChanged =
    !!move && moveSourcePolicy(value, move.from)?.policyRevision !== move.sourceRevision
  const showsSavedFolder = (
    group: CloudProjectGroup,
    policy: CloudOngoingStatus,
    folder: NonNullable<CloudOngoingStatus["savedFolders"]>[number],
  ) => {
    const repository = policy.repositoryGroupIds?.includes(folder.groupId)
    return (
      (repository || group.repositoryWide
        ? group.id === folder.groupId
        : group.path === folder.path || group.memberPaths.includes(folder.path)) &&
      session.projectHasSavedPermission(group.id, policy, true)
    )
  }
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
        {!teamPolicy.policyRevision ? (
          <FolderUploadState>Saved</FolderUploadState>
        ) : (
          <ToggleSwitch
            aria-label={controlLabel}
            aria-describedby={descriptionId}
            checked
            aria-disabled={folderControlsBusy}
            onCheckedChange={(next) => {
              if (next) return
              const active = document.activeElement
              void changeFolderWithFocus(
                active?.getAttribute("aria-label") === controlLabel ? active : null,
                foldersId,
                folderId,
                async () => {
                  await session.setSavedProject(value, folderId, false, team)
                },
              )
            }}
          />
        )}
      </div>
    )
  }
  const waitingForFolders =
    !projects?.loaded && !projects?.error && !projects?.retrying && !value.cloud.setupError
  const exactTeamFolders = teamPolicies.flatMap(([team, policy]) =>
    (policy.savedFolders ?? [])
      .filter(
        (folder) =>
          policy.status === "reauth_required" ||
          !visibleGroups?.some((group) => showsSavedFolder(group, policy, folder)),
      )
      .map((folder) => ({ ...folder, team, policy })),
  )
  const exactPersonalFolders =
    policy?.savedFolders?.filter(
      (folder) => !visibleGroups?.some((group) => showsSavedFolder(group, policy, folder)),
    ) ?? []
  const coldFolderLoad =
    (waitingForFolders ||
      (projects?.loading && !projects.error) ||
      (projects?.retrying && projects.partial && !projects.hasMore && !projects.error)) &&
    !visibleGroups?.length &&
    !exactPersonalFolders.length &&
    !exactTeamFolders.length
  const retryingFolders = projects?.retrying && projects.retryStage !== undefined
  const showFolderRetryStatus = retryingFolders && !coldFolderLoad && !visibleGroups?.length
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
    !draftFolders.length && !confirmTeam && policy?.sourceConfirmationRequired
      ? projects?.groups.find((group) => session.projectHasReviewableSource(group.id))
      : undefined
  const canSave = move
    ? !storageUnknown &&
      value.cloud.captureEnabled &&
      !value.cloud.setupError &&
      projects?.loaded &&
      !projects.loading &&
      !projects.scopeInvalid &&
      !projects.error &&
      !moveTargetUnavailable &&
      !moveSourceChanged
    : (!!confirmTeam && !draftFolders.length) ||
      (!storageUnknown &&
        value.cloud.captureEnabled &&
        !value.cloud.setupError &&
        projects?.loaded &&
        !projects.loading &&
        !projects.scopeInvalid &&
        !projects.error &&
        (startableFolder || !!savedSourceToReview))
  const syncPolicies = [
    ...(policy && (!hasTeamFolders || policy.projects.length || policy.savedFolders?.length)
      ? [policy]
      : []),
    ...savedTeamPolicies
      .filter(([id]) => projects?.workspaces?.some((workspace) => workspace.id === id))
      .map(([, team]) => team),
  ]
  const progressPolicies = syncPolicies.filter(
    (destinationPolicy) =>
      destinationPolicy.consentVersion === "desktop-project-sync-v1" &&
      destinationPolicy.projects.length > 0 &&
      destinationPolicy.storageKnown &&
      !destinationPolicy.paused,
  )
  const activePolicies = progressPolicies.filter(
    (policy) => value.deviceUploadEnabled === true && syncActivityReported(policy),
  )
  const foundForSavedRule = (destinationId: string | undefined, groupId: string) => {
    if (!destinationId || projects?.scopeInvalid) return undefined
    const snapshot = progressPolicies.find((policy) => policy.destinationId === destinationId)
    return snapshot && currentFolderFound(snapshot)?.[groupId]
  }
  const unverifiedTeamWorking =
    value.deviceUploadEnabled === true &&
    !projects?.workspaces &&
    !projects?.error &&
    !projects?.refreshError &&
    savedTeamPolicies.some(
      ([, policy]) =>
        policy.storageKnown &&
        !policy.paused &&
        (policy.fullSyncInProgress ||
          policy.backgroundSyncInProgress ||
          policy.backfillInProgress),
    )
  const scanning = activePolicies.some((policy) => policy.backfillInProgress)
  const hasStagedChoices = !!move || draftFolders.length > 0 || confirmableTeams.length > 0
  const syncWorking = syncPending || unverifiedTeamWorking || activePolicies.length > 0
  const folderControlsBusy = value.busy || syncWorking
  const syncActivity =
    syncPhase === "saving"
      ? "Saving folder choices"
      : syncPending && !scanning
        ? "Starting sync"
        : scanning
          ? "Checking folders"
          : syncingSessionsMessage
  const measuredProgress =
    syncWorking && !unverifiedTeamWorking && (scanning || !syncPending)
      ? currentUploadProgressMessage(activePolicies)
      : null
  const syncProgressMessage = syncWorking
    ? (measuredProgress ??
      ((syncActivity === syncingSessionsMessage && !unverifiedTeamWorking
        ? savedUploadProgressMessage(activePolicies).join(" • ")
        : "") ||
        syncActivity))
    : null
  const canSubmit =
    !syncWorking && (canSave || (!hasStagedChoices && canSyncSavedFolders(value)))
  const hasUsableFolder =
    canSubmit ||
    (!storageUnknown &&
      !!policy?.policyRevision &&
      (exactPersonalFolders.length > 0 ||
        visibleGroups?.some((group) =>
          session.projectHasSavedPermission(group.id, undefined, true),
        ))) ||
    exactTeamFolders.some(({ policy }) => !!policy.policyRevision) ||
    visibleGroups?.some((group) =>
      savedTeams(group.id).some(([, policy]) => !!policy.policyRevision),
    ) ||
    (value.cloud.captureEnabled &&
      !storageUnknown &&
      !value.cloud.setupError &&
      projects?.loaded &&
      !projects.loading &&
      !projects.error &&
      visibleGroups?.some((group) => !folderUploadUnavailable(group)))
  const folderError = projects?.scopeInvalid
    ? projects.error
    : projects?.loading
      ? null
      : value.cloud.setupError
        ? value.cloud.setupError === cloudCaptureUnsupported
          ? "Uploads are unavailable on this device."
          : "Could not read Cloud folder settings on this device."
        : !hasUsableFolder && !projects?.retrying
          ? (projects?.error ??
            projects?.refreshError ??
            (projects?.loaded && projects.partial && !projects.hasMore
              ? "Could not check session folders."
              : null))
          : null
  const hasError = value.uploadActionError || folderError || uploadError
  const workspaceUnavailable =
    !!projects?.workspaces &&
    savedTeamPolicies.some(
      ([id]) => !projects.workspaces?.some((workspace) => workspace.id === id),
    )
  const uploadNeedsAttention = syncPolicies.some(
    (policy) =>
      policy.fencedCount > 0 ||
      policy.interruptedCount > 0 ||
      (policy.recoveryRequiredCount ?? 0) > 0,
  )
  const syncError =
    !!hasError ||
    uploadNeedsAttention ||
    workspaceUnavailable ||
    syncPolicies.some(
      (policy) =>
        !policy.folderConfirmationRequired && !!uploadErrors[savedUploadDisplayState(policy)],
    )
  const idleStatus = idleSyncMessage({
    policies: syncPolicies,
    deviceEnabled: value.deviceUploadEnabled,
    workspaceUnavailable,
    hasError:
      !!value.uploadActionError ||
      !!folderError ||
      (!storageUnknown && !!uploadError) ||
      uploadNeedsAttention,
  })
  const footerStatus = syncError
    ? idleStatus
    : (syncProgressMessage ?? (confirmableTeams.length ? "Review saved folders" : idleStatus))
  const paging = projects?.paging
  const canCheckFolders = !!folderError && value.cloud.setupError !== cloudCaptureUnsupported
  const focusUploadSwitch = () => {
    if (!uploadSwitchRowId) return
    document
      .getElementById(uploadSwitchRowId)
      ?.querySelector<HTMLElement>('[role="switch"]')
      ?.focus()
  }
  const runSavedSync = async () => {
    setSyncPhase("starting")
    try {
      await session.syncNow(value)
    } finally {
      setSyncPhase(undefined)
    }
  }
  const runSync = async (trigger: HTMLButtonElement) => {
    const before = session.getSnapshot()
    if (
      syncActionBusy ||
      !canSubmit ||
      before.contextRevision !== value.contextRevision ||
      confirmationContextKey(before) !== confirmationContextKey(value) ||
      (move && moveSourcePolicy(before, move.from)?.policyRevision !== move.sourceRevision)
    )
      return
    if (!canSave) return runSavedSync()
    const focus = new AbortController()
    const stopFollowingSave = () => focus.abort()
    if (document.activeElement === trigger) {
      document.addEventListener(
        "focusin",
        (event) => {
          if (event.target !== trigger && event.target !== document.body) stopFollowingSave()
        },
        { signal: focus.signal },
      )
      window.addEventListener("blur", stopFollowingSave, { signal: focus.signal })
    } else stopFollowingSave()
    setSyncPhase("saving")
    try {
      await changeFolderWithFocus(
        trigger,
        foldersId,
        move?.groupId ?? (draftFolders.length === 1 ? draftFolders[0]?.id : undefined),
        () => saveAndSync().finally(() => setSyncPhase(undefined)),
      )
      if (
        !focus.signal.aborted &&
        document.activeElement === trigger &&
        session.getSnapshot().uploadActionError
      )
        document.getElementById(uploadActionWarningId)?.scrollIntoView?.({ block: "nearest" })
    } finally {
      stopFollowingSave()
    }

    async function saveAndSync() {
      if (move) {
        const outcome = await session.moveSavedProject(value, move.groupId, move.from, move.to)
        if (outcome === "keep_draft") return
        setFolderMove((pending) => (pending === move ? undefined : pending))
      } else if (
        (await session.startPendingProjects(
          value,
          hasSavedPolicy,
          confirmTeam?.[0],
          savedSourceToReview?.id,
        )) !== true
      )
        return
      const current = session.getSnapshot()
      if (
        current.busy ||
        current.uploadActionError ||
        current.cloud.status !== "signed_in" ||
        current.cloud.identity?.id !== accountId ||
        current.cloud.destination?.kind !== "personal" ||
        current.cloud.destination.id !== personalId
      )
        return

      const savedTeamId = confirmTeam?.[0] ?? [...draftTargets].find((id) => id !== personalId)
      if (
        !move &&
        savedTeamId &&
        !current.projects?.workspaces?.some(
          ({ id, kind }) => id === savedTeamId && kind === "team",
        )
      )
        return

      // A finished move can clear the draft without changing the saved policy.
      const savedMoveChange = (groupId: string, destinationId: string, added: boolean) => {
        const teamId = destinationId === personalId ? undefined : destinationId
        const updated = uploadPolicy(current, teamId)
        const previous = uploadPolicy(before, teamId)
        const revision = updated?.policyRevision
        return (
          updated?.consentVersion === "desktop-project-sync-v1" &&
          !!revision &&
          (revision !== previous?.policyRevision ||
            previous.consentVersion !== updated.consentVersion) &&
          matchesUploadPolicy(updated, { accountId, destinationId }, revision) &&
          (!teamId ||
            current.projects?.workspaces?.some(
              ({ id, kind }) => id === teamId && kind === "team",
            )) &&
          session.projectHasSavedPermission(groupId, updated, true) === added
        )
      }
      if (
        move &&
        (current.projects?.scopeInvalid ||
          !current.projects?.groups.some(
            (group) =>
              group.id === move.groupId &&
              group.confirmationKey === move.confirmationKey &&
              !group.discoveryStale,
          ) ||
          !savedMoveChange(move.groupId, move.from, false) ||
          (move.to && !savedMoveChange(move.groupId, move.to, true)))
      )
        return
      if (!canSyncSavedFolders(current)) return
      setSyncPhase("starting")
      await session.syncNow(current)
    }
  }
  const saveRecovery = folderSaveRecovery(value.uploadActionError)
  const scanRetry = value.uploadActionError === "Could not start sync."
  const canRetryAction = scanRetry ? canSyncSavedFolders(value) : canSave && !scanning
  const saveRecoveryLabel =
    saveRecovery === "retry" && canRetryAction
      ? "Try again"
      : saveRecovery === "retry" || saveRecovery === "check"
        ? "Check Cloud"
        : "Review folders"
  const checkFoldersInPrimary = canCheckFolders && !saveRecovery && !canSubmit
  const primaryBlocked = !saveRecovery && !canSubmit && !checkFoldersInPrimary
  const primaryLabel = syncWorking
    ? "Syncing"
    : saveRecovery
      ? saveRecoveryLabel
      : checkFoldersInPrimary
        ? "Check again"
        : hasSavedFolders
          ? "Sync now"
          : "Start syncing"
  const keepPrimaryFocusable =
    syncActionBusy || syncWorking || (primaryBlocked && !!projects?.loading)
  const recoverFolderSave = async () => {
    if (saveRecoveryLabel === "Check Cloud") {
      await session.retryFolderScope()
    } else {
      document.getElementById(foldersId)?.focus()
      if (saveRecovery === "refresh") await session.retryFolderScope()
    }
    session.clearUploadActionError(value.uploadActionError)
  }
  return (
    <>
      <SettingsRow
        searchId="cloudFolders"
        id={foldersId}
        role="group"
        aria-label="Cloud folders"
        className="@container border-t border-separator"
        description={`Uploads include full sessions from approved folders.${draftFolders.some((group) => group.repositoryWide) || (move?.to && moveGroup?.repositoryWide) ? " Matching Git checkouts are included." : ""}`}
      >
        {!!visibleGroups?.length && projects?.loading && !projects.scopeInvalid && (
          <p className="mt-2 type-footnote text-label-secondary" role="status">
            {retryingFolders
              ? cloudDiscoveryRetryMessage(projects.retryStage)
              : "Checking Cloud workspace access…"}
          </p>
        )}
        <div className="grid min-h-32 @min-[30rem]:min-h-24">
          <CloudFolderList groups={visibleGroups}>
            {projects &&
              visibleGroups?.map((group) => {
                const teams = savedTeams(group.id)
                const displayedTeams = teams.filter(
                  ([, team]) =>
                    !team.savedFolders ||
                    (team.status !== "reauth_required" &&
                      team.savedFolders.some((folder) =>
                        showsSavedFolder(group, team, folder),
                      )),
                )
                const savedPersonal = session.projectHasSavedPermission(
                  group.id,
                  undefined,
                  true,
                )
                const unverifiedGroup = group.discoveryStale || group.groupingComplete === false
                const savedDestination =
                  teams.length === 0 && savedPersonal
                    ? personalId
                    : !savedPersonal && teams.length === 1
                      ? displayedTeams[0]?.[0]
                      : undefined
                const folderProgress =
                  savedDestination && !unverifiedGroup && !projects.scopeInvalid
                    ? progressPolicies.find(
                        ({ destinationId }) => destinationId === savedDestination,
                      )
                    : undefined
                const syncFound = folderProgress && folderSyncFoundCount(group, folderProgress)
                const pendingMove = move?.groupId === group.id ? move : undefined
                const sourceRevision =
                  savedDestination && moveSourcePolicy(value, savedDestination)?.policyRevision
                const canMove =
                  !value.cloud.setupError &&
                  !projects.scopeInvalid &&
                  (multipleWorkspaces || !!pendingMove) &&
                  !!savedDestination &&
                  !!sourceRevision &&
                  projects.workspaces?.some(({ id }) => id === savedDestination) &&
                  !unverifiedGroup &&
                  group.captureSupported
                const showChoice = canMove || teams.length === 0 || savedPersonal
                if (!showChoice && displayedTeams.length === 0) return null
                const showPersonal =
                  !canMove && savedPersonal && (teams.length > 0 || multipleWorkspaces)
                const draftTarget = projects.targets?.[group.id]
                const pickWorkspace =
                  canMove ||
                  (!savedPersonal &&
                    (multipleWorkspaces || (!!draftTarget && draftTarget !== personalId)))
                const label = `Sync ${group.name}: ${group.path}`
                const controlLabel = showPersonal ? `${label} to Personal` : label
                const descriptionId = `cloud-settings-folder-${encodeURIComponent(group.id)}`
                const hasSavedControl = !!savedDestination || savedPersonal
                const unavailable = hasSavedControl ? null : folderUploadUnavailable(group)
                const controlMessage =
                  unavailable ??
                  (storageUnknown
                    ? "Sync status unavailable"
                    : (savedPersonal && !policy?.policyRevision) ||
                        (canMove && move && !pendingMove)
                      ? "Saved"
                      : !hasSavedControl && projects.loading
                        ? "Not ready"
                        : !hasSavedControl &&
                            (value.cloud.setupError || projects.error || projects.scopeInvalid)
                          ? "Uploads unavailable"
                          : null)
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
                        syncFound={syncFound}
                        stackPath
                      />
                    </div>
                    <div className="col-start-2 flex min-w-0 max-w-full flex-col items-start gap-2 @min-[30rem]:col-start-3 @min-[30rem]:items-end">
                      {!canMove &&
                        displayedTeams.map(([team, teamPolicy]) =>
                          savedTeamControl(group.id, label, team, teamPolicy, descriptionId),
                        )}
                      {showChoice && (
                        <div
                          data-cloud-workspace={
                            canMove ? savedDestination : savedPersonal ? personalId : undefined
                          }
                          className="flex min-w-0 max-w-full items-center gap-2"
                        >
                          {showPersonal && (
                            <span className="type-footnote text-label-secondary">Personal</span>
                          )}
                          {controlMessage && (!pickWorkspace || unavailable) ? (
                            <FolderUploadState>{controlMessage}</FolderUploadState>
                          ) : (
                            <CloudFolderSyncControl
                              label={controlLabel}
                              descriptionId={descriptionId}
                              selected={
                                pendingMove
                                  ? !!pendingMove.to
                                  : savedPersonal || (!unverifiedGroup && selected(group.id))
                              }
                              typedWorkspaceLabels
                              wrapWorkspaceLabel
                              keepWorkspacePicker={!!pendingMove}
                              workspaces={pickWorkspace ? projects.workspaces : undefined}
                              destinationId={personalId}
                              selectedWorkspace={
                                canMove
                                  ? (pendingMove?.to ?? savedDestination)
                                  : pickWorkspace
                                    ? draftTarget
                                    : undefined
                              }
                              disabled={!!controlMessage || !!(canMove && projects.loading)}
                              busy={folderControlsBusy}
                              onSelect={(next, workspace) => {
                                if (folderControlsBusy) return
                                if (canMove && savedDestination && sourceRevision) {
                                  const to = next ? workspace : undefined
                                  setFolderMove(
                                    to === savedDestination
                                      ? undefined
                                      : {
                                          accountId: identity.id,
                                          groupId: group.id,
                                          confirmationKey: group.confirmationKey,
                                          from: savedDestination,
                                          to,
                                          sourceRevision,
                                        },
                                  )
                                } else if (savedPersonal) {
                                  const active = document.activeElement
                                  void changeFolderWithFocus(
                                    active?.getAttribute("aria-label") === controlLabel
                                      ? active
                                      : null,
                                    foldersId,
                                    group.id,
                                    async () => {
                                      await session.setSavedProject(value, group.id, next)
                                    },
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
                          )}
                        </div>
                      )}
                    </div>
                  </div>
                )
              })}
            {(exactPersonalFolders.length > 0 || exactTeamFolders.length > 0) && (
              <details className="py-2">
                <summary className="cursor-pointer type-footnote text-label-secondary">
                  Manage other saved folders
                </summary>
                <div className="mt-2 flex flex-col divide-y divide-separator">
                  {exactPersonalFolders.map(({ groupId, path }) => (
                    <div
                      key={`personal:${groupId}`}
                      data-exact-folder={path}
                      data-cloud-workspace={personalId}
                      className="flex items-center gap-3 py-2"
                    >
                      <Folder size={16} className="shrink-0 text-label-secondary" aria-hidden />
                      <SavedFolderSummary
                        path={path}
                        homeDirectory={value.homeDirectory}
                        found={foundForSavedRule(personalId, groupId)}
                      />
                      {hasTeamFolders && (
                        <span className="type-footnote text-label-secondary">Personal</span>
                      )}
                      {storageUnknown || !policy?.policyRevision ? (
                        <FolderUploadState>Saved</FolderUploadState>
                      ) : (
                        <ToggleSwitch
                          aria-label={`Sync ${path} to Personal`}
                          checked
                          aria-disabled={folderControlsBusy}
                          onCheckedChange={(next) => {
                            if (next) return
                            void changeFolderWithFocus(
                              document.activeElement,
                              foldersId,
                              undefined,
                              async () => {
                                await session.setSavedProject(value, groupId, false)
                              },
                            )
                          }}
                        />
                      )}
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
                      <SavedFolderSummary
                        path={path}
                        homeDirectory={value.homeDirectory}
                        found={foundForSavedRule(team, groupId)}
                      />
                      <div className="col-start-2 min-w-0 @min-[30rem]:col-start-3 @min-[30rem]:justify-self-end">
                        {savedTeamControl(groupId, `Sync ${path}`, team, policy)}
                      </div>
                    </div>
                  ))}
                </div>
              </details>
            )}
            {coldFolderLoad && (
              <CloudFolderLoadingRow
                message={
                  retryingFolders
                    ? cloudDiscoveryRetryMessage(projects.retryStage)
                    : "Loading folders…"
                }
              />
            )}
            {empty && (
              <div className="flex flex-wrap items-center gap-2 py-2">
                <p className="type-footnote text-label-secondary">
                  No session folders found. Start a session in a supported coding tool.
                </p>
              </div>
            )}
          </CloudFolderList>
        </div>
        {!syncProgressMessage && (paging || showFolderRetryStatus) && (
          <p role="status" className="mt-2 type-footnote text-label-secondary">
            {paging
              ? paging === "loading"
                ? "Loading more folders…"
                : "Still checking for more folders."
              : cloudDiscoveryRetryMessage(projects?.retryStage)}
          </p>
        )}
      </SettingsRow>
      <div className="flex min-w-0 flex-col gap-1.5 px-4 pb-2 type-footnote text-label-secondary wrap-anywhere empty:hidden">
        {!syncPending && value.uploadActionError && (
          <CloudFolderWarning id={uploadActionWarningId}>
            {value.uploadActionError}
          </CloudFolderWarning>
        )}
        {!syncPending && folderError && (
          <CloudFolderWarning
            id={folderWarningId}
            action={
              canCheckFolders && !checkFoldersInPrimary ? (
                <button
                  type="button"
                  className="ui-push-button min-h-8 aria-disabled:opacity-50"
                  aria-describedby={folderWarningId}
                  aria-disabled={syncActionBusy || undefined}
                  onClick={(event) => {
                    if (syncActionBusy) return
                    void changeFolderWithFocus(
                      event.currentTarget,
                      foldersId,
                      undefined,
                      session.retryFolderScope,
                    )
                  }}
                >
                  Check again
                </button>
              ) : undefined
            }
          >
            {folderError}
          </CloudFolderWarning>
        )}
        {uploadError && (
          <CloudFolderWarning
            action={
              !storageUnknown && canRetryPersonal ? (
                <RetryUploadButton
                  label={
                    hasTeamFolders ? "Retry saved uploads to Personal" : "Retry saved uploads"
                  }
                  busy={syncActionBusy}
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
          if (checkingUploadStatus && team.status === "storage_unavailable") return null
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
                    busy={syncActionBusy}
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
        {!move && !draftFolders.length && showTeamChoice && confirmableTeams.length > 0 && (
          <label className="flex flex-wrap items-center gap-1">
            Save folders for
            <select
              className="max-w-full rounded-control bg-surface-card text-label"
              disabled={folderControlsBusy}
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
        {!syncPending &&
          (move && !value.uploadActionError ? (
            <p>
              {moveSourceChanged
                ? "The saved folder changed. Choose a workspace again."
                : moveTargetUnavailable
                  ? "That workspace is unavailable. Choose another."
                  : move.to
                    ? "Previous uploads stay in the old workspace."
                    : "Sync now stops uploads for this folder."}
            </p>
          ) : confirmTeam && !value.uploadActionError ? (
            <p>
              {draftFolders.length
                ? "Sync now approves the listed and new Team folders."
                : "Sync now approves the listed Team folders."}
            </p>
          ) : unavailableTarget ? (
            <p>
              A selected workspace is unavailable. Choose another destination or turn off its
              folder.
            </p>
          ) : draftTargets.size > 1 ? (
            <p>Choose one workspace at a time.</p>
          ) : null)}
        {!syncPending &&
          !hasError &&
          (policy?.sourceConfirmationRequired ||
            teamPolicies.some(([, team]) => team.sourceConfirmationRequired)) && (
            <p>
              {savedSourceToReview || draftFolders.length ? (
                "Sync now checks changed sources."
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
      </div>
      <footer className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-x-4 border-t border-separator px-4 py-2 type-footnote text-label-secondary">
        <CloudSyncActivity message={footerStatus} active={syncWorking && !syncError} />
        <button
          type="button"
          className="ui-push-button min-h-8 min-w-28 gap-1 enabled:bg-accent-fill enabled:text-white enabled:hover:bg-accent-hover disabled:bg-surface-tertiary disabled:text-label-secondary aria-disabled:opacity-50"
          disabled={syncWorking || (primaryBlocked && !keepPrimaryFocusable)}
          aria-disabled={(!syncWorking && keepPrimaryFocusable) || undefined}
          aria-busy={syncWorking || undefined}
          aria-describedby={
            syncWorking
              ? undefined
              : saveRecovery
                ? uploadActionWarningId
                : checkFoldersInPrimary
                  ? folderWarningId
                  : undefined
          }
          aria-description={
            syncWorking || saveRecovery || checkFoldersInPrimary
              ? undefined
              : move
                ? "Apply the selected workspace change to this folder."
                : confirmTeam
                  ? `Save full-session uploads for the exact listed paths in ${teamName(...confirmTeam)}.`
                  : canSave
                    ? `Approve full-session uploads for the selected folders and worktrees for ${identity.login}, then check saved folders for updates.`
                    : "Check saved folders for new or changed sessions."
          }
          onClick={(event) => {
            if (syncActionBusy || syncWorking) return
            if (checkFoldersInPrimary) {
              void session.retryFolderScope()
            } else if (scanRetry && canRetryAction) {
              void runSavedSync()
            } else if (!saveRecovery || (saveRecovery === "retry" && canRetryAction)) {
              void runSync(event.currentTarget)
            } else {
              void recoverFolderSave()
            }
          }}
        >
          <span className="flex w-3.5 shrink-0 items-center justify-center" aria-hidden>
            {syncWorking && <LoaderCircle size={13} className="motion-safe:animate-spin" />}
          </span>
          <span>{primaryLabel}</span>
        </button>
      </footer>
    </>
  )
}
