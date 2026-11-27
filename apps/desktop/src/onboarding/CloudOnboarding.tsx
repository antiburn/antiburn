import {
  useId,
  useState,
  useSyncExternalStore,
  type CSSProperties,
  type ReactNode,
} from "react"
import { invoke } from "@tauri-apps/api/core"
import { ArrowUp, ArrowUpRight, Check, Cloud, Info, Plus, UserRound } from "lucide-react"
import appIcon from "../assets/app-icon.png"
import { DeviceAccessUpdate } from "./DeviceAccessUpdate"
import type { CloudPort } from "../lib/cloud"
import { noteCloudAction, noteOnboardingAction } from "../lib/onboardingAnalytics"
import {
  CloudSession,
  cloudGithubSignInLabel,
  cloudStatusMessage,
  cloudOngoingStatusMessage,
  cloudUploadStatusUnknown,
} from "./CloudSession"
import { CloudProjectFolders, cloudWorkspaceLabel } from "./CloudOnboardingFolders"
import { changeFolderWithFocus } from "./cloudFolderFocus"
import { cloudCaptureUnsupported, cloudDiscoveryRetryMessage } from "./cloudProjectScope"
import { cloudAccountCleanupPending, cloudSignOutBeforeSignIn } from "./cloudSessionState"
import "./onboarding.css"
import "./cloud.css"

/** Existing optional setup flow. The desktop build capability gates its caller;
 * runtime service or platform failures must still allow local completion.
 */
export interface CloudOnboardingProps {
  onBack: () => void
  onComplete: () => void
  busy?: boolean
  error?: string | null | undefined
  visible?: boolean
  port?: CloudPort
  analytics?: boolean
  holdIdentity?: boolean
}
const focusHeading = (node: HTMLHeadingElement | null) => node?.focus()

function CloudBadge() {
  return (
    <>
      <div className="onboarding-drag-region" data-tauri-drag-region />
      <span className="cloud-badge type-caption">
        <Cloud size={14} aria-hidden />
        antiburn cloud
      </span>
    </>
  )
}

function CloudHandoff({
  authEnabled,
  children,
}: {
  authEnabled: boolean
  children: ReactNode
}) {
  return (
    <section className="cloud-setup" aria-label="Cloud connection">
      <div className="cloud-connection">
        <img className="cloud-app-icon" src={appIcon} alt="" />
        <h1 className="type-onboarding-title" tabIndex={-1} ref={focusHeading}>
          {authEnabled ? "Sign in to Antiburn Cloud" : "Antiburn Cloud is coming soon"}
        </h1>
        <p className="type-onboarding-body text-onboarding-muted">
          {authEnabled
            ? "Use GitHub in your browser to approve this device. Signing in doesn’t start uploads."
            : "Learn about bringing your session insights together across devices."}
        </p>
        {children}
      </div>
    </section>
  )
}

/** Show the public waitlist without creating a Cloud account session. */
export function CloudWaitlist({
  screen,
  onBack,
  onComplete,
  busy = false,
  error,
}: Pick<CloudOnboardingProps, "onBack" | "onComplete" | "busy" | "error"> & {
  screen: "cloud" | "cloudUnavailable"
}) {
  const [opening, setOpening] = useState(false)
  const [action, setAction] = useState<"navigate" | "open" | "openFailed">("navigate")
  const actionError =
    action === "openFailed" ? "Could not open the Cloud page. Try again." : error

  const navigate = (direction: "back" | "done") => {
    setAction("navigate")
    if (screen === "cloudUnavailable") noteOnboardingAction(screen, direction)
    if (direction === "back") onBack()
    else onComplete()
  }

  const openWaitlist = async () => {
    noteOnboardingAction(screen, "learn_more")
    setOpening(true)
    setAction("open")
    try {
      await invoke("plugin:opener|open_url", { url: "https://antiburn.com/cloud" })
    } catch {
      setAction((current) => (current === "open" ? "openFailed" : current))
    } finally {
      setOpening(false)
    }
  }

  return (
    <main
      className="onboarding-shell cloud-shell cloud-screen-handoff"
      data-auth-enabled={false}
      aria-label="Antiburn Cloud setup"
    >
      <CloudBadge />
      <CloudHandoff authEnabled={false}>
        <button
          className="onboarding-secondary"
          disabled={busy || opening}
          onClick={() => void openWaitlist()}
        >
          Learn more
          <ArrowUpRight size={16} aria-hidden />
        </button>
        <p className="cloud-status type-footnote" role="status" />
      </CloudHandoff>
      <footer className="onboarding-footer cloud-footer">
        <div className="flex w-full flex-wrap items-center justify-between gap-3">
          <button
            className="onboarding-secondary"
            disabled={busy}
            onClick={() => navigate("back")}
          >
            Back
          </button>
          <div className="onboarding-actions flex-wrap">
            {actionError && (
              <span className="type-footnote" role="alert">
                {actionError}
              </span>
            )}
            <button
              className="onboarding-primary"
              disabled={busy}
              onClick={() => navigate("done")}
            >
              Done
            </button>
          </div>
        </div>
      </footer>
    </main>
  )
}
/** Observe one CloudSession. Completion can stay local without content consent;
 * leaving browser approval cancels only that attempt, not upload consent.
 */
export function CloudOnboarding({
  onBack,
  onComplete,
  busy = false,
  error,
  visible = true,
  port,
  analytics = true,
  holdIdentity = false,
}: CloudOnboardingProps) {
  const documentVisible = useSyncExternalStore(
    subscribeVisibility,
    () => !document.hidden,
    () => true,
  )
  const [session] = useState(() => new CloudSession(port, analytics, holdIdentity))
  const [openingSetup, setOpeningSetup] = useState(false)
  const [setupOpenFailed, setSetupOpenFailed] = useState(false)
  const [checkingWorkspaces, setCheckingWorkspaces] = useState(false)
  const foldersId = useId()
  const value = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const disabled = busy || value.busy || openingSetup
  const browserSetup = session.canOpenSetup() && value.cloud.status === "signed_in"
  const projects = value.projects
  const accessUpdateWaiting = value.cloud.accessUpdate === "waiting"
  const waiting =
    !accessUpdateWaiting &&
    value.cloud.authEnabled &&
    ["waiting", "opening"].includes(value.cloud.status)
  const checkingSignIn = value.cloud.authEnabled && value.cloud.status === "returned"
  const cleanupPending = cloudAccountCleanupPending(value.cloud)
  const ongoing = value.lastTeamSync ?? value.ongoing
  const syncReady =
    ongoing?.storageKnown === true && ["ready", "syncing"].includes(ongoing.status)
  const syncing =
    syncReady && (ongoing?.status === "syncing" || (ongoing?.queuedCount ?? 0) > 0)
  const destination = value.cloud.destination
  const personalDestination = destination?.kind === "personal" ? destination : undefined
  const workspaces = projects?.workspaces ?? (personalDestination ? [personalDestination] : [])
  const hasWorkspaceChoice = workspaces.length > 1
  const savedTeamPolicies = Object.values(value.teamStatuses ?? {})
  const skipImplicitSelection =
    !hasWorkspaceChoice &&
    (value.ongoing?.consentVersion === "desktop-project-sync-v1" ||
      savedTeamPolicies.length > 0)
  const folderChoices = skipImplicitSelection
    ? Object.fromEntries(
        projects?.groups.map(({ id }) => [
          id,
          !!projects.choices[id] && !!projects.targets?.[id],
        ]) ?? [],
      )
    : projects?.choices
  const savedDestinations = new Map<string, string>()
  for (const group of projects?.groups ?? []) {
    if (personalDestination && session.projectHasSavedPermission(group.id)) {
      savedDestinations.set(group.id, cloudWorkspaceLabel(personalDestination))
      continue
    }
    const policy = savedTeamPolicies.find((team) =>
      session.projectHasSavedPermission(group.id, team),
    )
    if (policy) {
      const team = workspaces.find((workspace) => workspace.id === policy.destinationId)
      savedDestinations.set(group.id, team ? cloudWorkspaceLabel(team) : "Team workspace")
    }
  }
  const selectedProjects = projects?.groups.filter(
    (group) =>
      group.captureSupported &&
      !savedDestinations.has(group.id) &&
      (folderChoices?.[group.id] ?? !hasWorkspaceChoice),
  )
  const selectedFolders = selectedProjects?.length ?? 0
  const needsWorkspaceChoice =
    hasWorkspaceChoice && selectedProjects?.some((group) => !projects?.targets?.[group.id])
  const selectedWorkspaceIds = [
    ...new Set(
      selectedProjects?.flatMap((group) => {
        const target =
          projects?.targets?.[group.id] ??
          (hasWorkspaceChoice ? undefined : personalDestination?.id)
        return target ? [target] : []
      }),
    ),
  ]
  const workspaceLabels = selectedWorkspaceIds.map((id) => {
    const workspace = workspaces.find((candidate) => candidate.id === id)
    return workspace ? cloudWorkspaceLabel(workspace) : "Selected workspace unavailable"
  })
  const workspaceUnavailable = selectedWorkspaceIds.some(
    (id) => !workspaces.some((workspace) => workspace.id === id),
  )
  const workspaceConflict = selectedWorkspaceIds.length > 1
  const workspaceSummary =
    projects?.error || !personalDestination
      ? "Workspace availability could not be confirmed"
      : !projects?.loaded
        ? "Checking workspace destinations…"
        : needsWorkspaceChoice
          ? "Choose a sync destination"
          : workspaceLabels.length
            ? `Sync ${workspaceConflict ? "destinations" : "destination"}: ${workspaceLabels.join(" · ")}`
            : hasWorkspaceChoice
              ? "No sync destination selected"
              : `Available destination: ${cloudWorkspaceLabel(personalDestination)}`
  const workspaceNote = workspaceUnavailable
    ? "A selected workspace can’t receive uploads. Choose another destination or Don't sync."
    : needsWorkspaceChoice
      ? "Choose a workspace or Don't sync for each marked folder before starting."
      : hasWorkspaceChoice
        ? "Each folder uploads only to its selected workspace."
        : null
  const showWorkspaceNote = projects?.loaded && !projects.error
  const folderMessage =
    error ??
    value.error ??
    (projects?.loading
      ? ""
      : projects?.scopeInvalid
        ? (projects.error ?? "")
        : projects?.error ||
            (value.cloud.setupError && value.cloud.setupError !== cloudCaptureUnsupported)
          ? "Antiburn could not check your session folders. You can continue locally."
          : !value.cloud.captureEnabled
            ? "Uploads are unavailable on this device."
            : projects?.needsChoiceReview
              ? "Review your folders before starting sync."
              : projects?.loaded &&
                  !projects.loading &&
                  !projects.hasMore &&
                  !projects.partial &&
                  projects.groups.some((group) => group.captureSupported === false)
                ? "Could not check uploads for some folders."
                : "")
  const projectListComplete =
    projects?.loaded &&
    !projects.loading &&
    !projects.partial &&
    !projects.hasMore &&
    !projects.error &&
    !value.cloud.setupError &&
    !value.error &&
    !error
  const emptyProjectMessage = folderMessage
    ? "No folders loaded."
    : projects?.retrying && projects.retryStage === "folders"
      ? "Still checking session folders."
      : !projects?.loaded || projects.loading
        ? "Loading project folders…"
        : "Folder discovery is incomplete."
  const toolsPhase = value.localTools?.phase ?? "pending"
  const toolsReady = !value.localToolsError && toolsPhase === "ready"
  const toolsTitle = value.localToolsError
    ? "Local tools status unavailable"
    : {
        ready: "Local tools ready",
        pending: "Setting up local tools",
        unavailable: "Local tools unavailable",
        failed: "Local tools setup failed",
      }[toolsPhase]
  const toolsDetail = value.localToolsError
    ? "Check Settings → CLI."
    : toolsPhase === "ready"
      ? "CLI tested · Skill installed"
      : toolsPhase === "failed"
        ? value.localTools?.reason === "cli_unavailable"
          ? "CLI unavailable. Try Repair in Settings → CLI."
          : "Try Repair in Settings → CLI."
        : toolsPhase === "unavailable"
          ? "Unavailable for this setup."
          : "CLI and skill"
  const finish = async () => {
    if (openingSetup || busy || (value.screen !== "complete" && value.busy)) return
    if (analytics)
      noteCloudAction(
        value.screen,
        value.screen === "complete" || !value.cloud.authEnabled ? "done" : "use_locally",
      )
    if (waiting && !(await session.cancelSilently())) return
    onComplete()
  }
  const back = async () => {
    if (disabled) return
    if (value.screen === "handoff" || value.screen === "identity") {
      if (analytics) noteCloudAction(value.screen, "back")
      if (waiting && !(await session.cancelSilently())) return
      onBack()
    } else await session.go(value.screen === "folders" ? "identity" : "handoff", "back")
  }
  return (
    <main
      ref={() => session.setVisible(visible)}
      data-paused={!visible || !documentVisible}
      data-auth-enabled={value.cloud.authEnabled}
      className={`onboarding-shell cloud-shell cloud-screen-${value.screen}${value.screen === "folders" ? " flex! flex-col!" : ""}`}
      aria-label="Antiburn Cloud setup"
    >
      <CloudBadge />
      {value.screen === "handoff" ? (
        <CloudHandoff authEnabled={value.cloud.authEnabled}>
          {waiting ? (
            <button
              className="onboarding-secondary"
              disabled={disabled}
              onClick={() => void session.cancel()}
            >
              Cancel sign-in
            </button>
          ) : cleanupPending ? (
            !value.cloud.localResetAvailable && (
              <button
                className="onboarding-secondary"
                disabled={disabled || !value.loaded}
                onClick={() => void session.signOut()}
              >
                {value.cloud.localCleanupPending ? "Finish signing out" : "Sign out"}
              </button>
            )
          ) : !accessUpdateWaiting && !checkingSignIn ? (
            <button
              className={
                value.cloud.authEnabled ? "onboarding-primary" : "onboarding-secondary"
              }
              disabled={disabled || !value.loaded}
              onClick={() =>
                void (!value.cloud.authEnabled ||
                value.cloud.status === "idle" ||
                value.cloud.status === "signed_out"
                  ? session.begin()
                  : session.retry())
              }
            >
              {value.cloud.authEnabled ? cloudGithubSignInLabel : "Learn more"}
              <ArrowUpRight size={16} aria-hidden />
            </button>
          ) : null}
          {accessUpdateWaiting && (
            <DeviceAccessUpdate
              value={value.cloud}
              busy={disabled}
              update={session.begin}
              cancel={session.cancel}
              onboarding
            />
          )}
          <p className="cloud-status type-footnote" role="status">
            {value.loaded
              ? cleanupPending
                ? value.cloud.localResetAvailable
                  ? "Open Cloud in Settings to reset sign-in on this device."
                  : cloudSignOutBeforeSignIn
                : value.cloud.authEnabled || value.cloud.status === "browser_failed"
                  ? cloudStatusMessage(value.cloud.status)
                  : ""
              : "Checking Cloud availability…"}
          </p>
        </CloudHandoff>
      ) : value.screen === "identity" ? (
        <section className="cloud-setup">
          <div className="cloud-connection cloud-identity">
            <div className="cloud-identity-icon">
              {value.cloud.identity?.avatar ? (
                <img src={value.cloud.identity.avatar} alt="" />
              ) : (
                <UserRound size={32} aria-hidden />
              )}
              <span className="cloud-verified">
                <Check size={16} aria-hidden />
              </span>
            </div>
            <h1 className="type-onboarding-title" tabIndex={-1} ref={focusHeading}>
              You’re signed in
            </h1>
            {browserSetup && (
              <>
                <p className="type-onboarding-body text-onboarding-muted">
                  Verify your work email in your browser to check for a workspace invite. Use
                  the same GitHub account.
                  {!value.cloud.setupError && " Then return here and continue."}
                </p>
                <button
                  className="onboarding-secondary"
                  aria-disabled={disabled || undefined}
                  aria-busy={openingSetup || undefined}
                  onClick={() => {
                    if (disabled) return
                    setOpeningSetup(true)
                    setSetupOpenFailed(false)
                    void session
                      .openSetup()
                      .catch(() => setSetupOpenFailed(true))
                      .finally(() => setOpeningSetup(false))
                  }}
                >
                  Finish setup in browser <ArrowUpRight size={16} aria-hidden />
                </button>
                {setupOpenFailed && (
                  <p className="type-callout text-onboarding-muted" role="alert">
                    Could not open Cloud setup in your browser. Try again.
                  </p>
                )}
              </>
            )}
            <DeviceAccessUpdate
              value={value.cloud}
              busy={disabled}
              update={session.begin}
              cancel={session.cancel}
              onboarding
            />
            {value.cloud.setupError && (
              <>
                <p className="type-callout text-onboarding-muted" role="status">
                  {value.cloud.setupError} You can continue locally and manage Cloud in
                  Settings.
                </p>
                <button
                  className="onboarding-secondary"
                  disabled={disabled}
                  onClick={() => void finish()}
                >
                  Continue locally
                </button>
              </>
            )}
            {value.error && !browserSetup && (
              <>
                <p className="type-callout text-onboarding-muted" role="alert">
                  {value.error}
                </p>
                <button
                  className="onboarding-primary"
                  disabled={disabled}
                  onClick={() => void session.go("folders")}
                >
                  Try again
                </button>
                <button
                  className="onboarding-secondary"
                  disabled={disabled}
                  onClick={() => void back()}
                >
                  Back
                </button>
              </>
            )}
          </div>
        </section>
      ) : value.screen === "folders" ? (
        <section className="cloud-folder-content min-h-0 flex-1 overflow-hidden pb-0!">
          <h1 className="type-cloud-folder-title" tabIndex={-1} ref={focusHeading}>
            Choose what to sync to <span className="text-brand">antiburn cloud</span>
          </h1>
          <p id="cloud-sync-description" className="type-onboarding-body text-onboarding-muted">
            {`Sync full sessions from approved folders.${selectedProjects?.some((group) => group.repositoryWide) ? " Matching Git checkouts are included." : ""}`}
          </p>
          <div className="flex flex-wrap items-center justify-between gap-2 type-callout text-onboarding-muted">
            <span id="cloud-sync-selection">
              {selectedFolders} {selectedFolders === 1 ? "folder" : "folders"} selected
            </span>
            <span
              id="cloud-sync-destination"
              className="min-w-0 wrap-anywhere rounded-control bg-surface-card px-2 py-1 type-footnote"
              aria-live="polite"
            >
              {workspaceSummary}
              {value.cloud.identity && ` · ${value.cloud.identity.login}`}
            </span>
          </div>
          {showWorkspaceNote && workspaceNote && (
            <p
              id="cloud-workspace-note"
              className="type-callout text-onboarding-muted"
              role="status"
            >
              {workspaceNote}
            </p>
          )}
          {projects && (projects.groups.length > 0 || projectListComplete) ? (
            <CloudProjectFolders
              id={foldersId}
              groups={projects.groups}
              choices={folderChoices ?? {}}
              disabled={
                disabled ||
                !projects.loaded ||
                projects.loading ||
                !!projects.error ||
                !!value.cloud.setupError ||
                !personalDestination
              }
              homeDirectory={value.homeDirectory}
              workspaces={workspaces}
              destinationId={personalDestination?.id}
              targets={projects.targets}
              savedDestinations={savedDestinations}
              onSelect={(id, selected, workspace) =>
                session.selectProject(
                  id,
                  selected,
                  selected ? (workspace ?? personalDestination?.id) : undefined,
                )
              }
            />
          ) : (
            <div
              id={foldersId}
              tabIndex={-1}
              className="cloud-folder-list min-h-0! max-h-none! flex-1"
            >
              <p className="cloud-empty type-body text-onboarding-muted">
                {emptyProjectMessage}
              </p>
            </div>
          )}
          {projects?.hasMore && (
            <button
              className="onboarding-secondary cloud-add"
              aria-disabled={
                disabled ||
                projects.loading ||
                !!projects.paging ||
                projects.scopeInvalid ||
                undefined
              }
              onClick={(event) => {
                if (disabled || projects.loading || projects.paging || projects.scopeInvalid)
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
          {folderMessage && (
            <div
              className="flex items-center gap-2 type-callout text-onboarding-muted"
              role="status"
            >
              <Info size={12} className="shrink-0" aria-hidden />
              <span>{folderMessage}</span>
            </div>
          )}
          {projects?.retrying &&
            projects.loading &&
            !folderMessage &&
            (projects.groups.length > 0 || projects.retryStage !== "folders") && (
              <p className="type-callout text-onboarding-muted" role="status">
                {cloudDiscoveryRetryMessage(projects.retryStage)}
              </p>
            )}
          {projects?.paging && projects.paging !== "pending" && (
            <p className="type-callout text-onboarding-muted" role="status">
              {projects.paging === "retrying"
                ? "Still checking for more folders."
                : "Loading more folders…"}
            </p>
          )}
          {value.sync.status === "starting" && (
            <p className="type-callout text-onboarding-muted" role="status">
              Confirming folder sync…
            </p>
          )}
        </section>
      ) : (
        <>
          <section
            className="onboarding-stage cloud-ready-stage"
            aria-label="Example session questions"
          >
            <ul className="cloud-ready-questions">
              {[
                "Where did I spend the most tokens this week?",
                "Which sessions cost the most, and why?",
              ].map((question, index) => (
                <li
                  key={question}
                  style={
                    {
                      "--cloud-question-start": `var(--cloud-question-${index + 1}-start)`,
                    } as CSSProperties
                  }
                >
                  <p className="type-onboarding-body" aria-label={question}>
                    <span aria-hidden>
                      {Array.from(question).map((character, position) => (
                        <span
                          key={position}
                          className="cloud-prompt-character"
                          style={{ "--cloud-character-index": position } as CSSProperties}
                        >
                          {character}
                        </span>
                      ))}
                    </span>
                  </p>
                  <div aria-hidden>
                    <Plus size={20} />
                    <span>
                      <ArrowUp size={20} />
                    </span>
                  </div>
                </li>
              ))}
            </ul>
          </section>
          <section className="onboarding-copy cloud-ready-copy">
            <h1 className="type-onboarding-title" tabIndex={-1} ref={focusHeading}>
              {syncReady
                ? "Sessions sync in the background"
                : "Continue using Antiburn locally"}
            </h1>
            <p className="type-onboarding-body">
              Manage Cloud uploads in Settings. You can pause uploads and review saved work
              there.
            </p>
          </section>
        </>
      )}
      <footer
        className={`onboarding-footer cloud-footer${value.screen === "identity" && browserSetup ? " flex!" : ""}`}
      >
        <div className="flex w-full flex-wrap items-center justify-between gap-3">
          {value.screen === "complete" ? (
            <div className="cloud-outcomes" role="group" aria-label="Setup status">
              <div>
                <span
                  className={`cloud-sync-dot${syncing ? " is-syncing" : ""}${!syncReady ? " is-inactive" : ""}`}
                  aria-hidden
                />
                <strong className="type-headline">
                  {ongoing && cloudUploadStatusUnknown(ongoing)
                    ? cloudOngoingStatusMessage(ongoing)
                    : value.sync.status === "starting"
                      ? "Starting background sync"
                      : value.sync.status === "ambiguous"
                        ? "Cloud sync not confirmed"
                        : ongoing
                          ? cloudOngoingStatusMessage(ongoing)
                          : "Cloud sync not confirmed"}
                </strong>
                <span className="type-callout text-onboarding-muted">
                  Manage uploads in Settings
                </span>
              </div>
              <div>
                {toolsReady ? (
                  <Check className="cloud-agent-tick" size={12} aria-hidden />
                ) : (
                  <span aria-hidden />
                )}
                <strong className="type-headline">{toolsTitle}</strong>
                <span className="type-callout text-onboarding-muted">{toolsDetail}</span>
              </div>
            </div>
          ) : value.screen !== "identity" ? (
            <button
              className="onboarding-secondary"
              disabled={disabled}
              onClick={() => void back()}
            >
              Back
            </button>
          ) : null}
          <div className="onboarding-actions flex-wrap">
            {value.screen === "identity" && checkingWorkspaces && (
              <span className="type-footnote" role="status">
                Checking Cloud workspaces…
              </span>
            )}
            {value.screen !== "folders" &&
              (error || (value.screen !== "complete" && value.error)) && (
                <span className="type-footnote" role="alert">
                  {error ?? value.error}
                </span>
              )}
            {value.screen === "handoff" && !value.cloud.authEnabled && (
              <button
                className={
                  value.cloud.authEnabled ? "onboarding-secondary" : "onboarding-primary"
                }
                disabled={disabled}
                onClick={() => void finish()}
              >
                {!value.cloud.authEnabled
                  ? "Done"
                  : value.cloud.status === "returned" || waiting
                    ? "Continue to Sessions"
                    : "Use locally for now"}
              </button>
            )}
            {((value.screen === "handoff" && value.cloud.authEnabled) ||
              (value.screen === "identity" && !value.cloud.setupError) ||
              value.screen === "folders") && (
              <button
                className="border-0! bg-transparent text-onboarding-muted underline underline-offset-4"
                disabled={disabled}
                onClick={() => void finish()}
              >
                Continue locally
              </button>
            )}
            {value.screen === "identity" && browserSetup && !value.cloud.setupError && (
              <button
                className="onboarding-primary"
                disabled={disabled}
                aria-busy={checkingWorkspaces || undefined}
                onClick={() => {
                  setCheckingWorkspaces(true)
                  void session
                    .continueFromDashboard()
                    .finally(() => setCheckingWorkspaces(false))
                }}
              >
                Continue
              </button>
            )}
            {value.screen === "folders" && (
              <>
                <button
                  className="onboarding-primary"
                  disabled={
                    disabled ||
                    !value.cloud.captureEnabled ||
                    value.cloud.status !== "signed_in" ||
                    !!value.cloud.setupError ||
                    !value.cloud.identity ||
                    value.cloud.destination?.kind !== "personal" ||
                    !value.cloud.destination.id ||
                    !projects?.loaded ||
                    cloudUploadStatusUnknown(value.ongoing) ||
                    projects.loading ||
                    !!projects.error ||
                    needsWorkspaceChoice ||
                    workspaceUnavailable ||
                    selectedFolders === 0
                  }
                  aria-describedby={[
                    "cloud-sync-description",
                    "cloud-sync-selection",
                    "cloud-sync-destination",
                    showWorkspaceNote && workspaceNote && "cloud-workspace-note",
                  ]
                    .filter(Boolean)
                    .join(" ")}
                  onClick={() => void session.startSync(value)}
                >
                  Start syncing
                </button>
              </>
            )}
            {value.screen === "complete" && (
              <button
                className="onboarding-primary"
                disabled={busy}
                onClick={() => void finish()}
              >
                Done
              </button>
            )}
          </div>
        </div>
      </footer>
    </main>
  )
}
function subscribeVisibility(listener: () => void) {
  document.addEventListener("visibilitychange", listener)
  return () => document.removeEventListener("visibilitychange", listener)
}
