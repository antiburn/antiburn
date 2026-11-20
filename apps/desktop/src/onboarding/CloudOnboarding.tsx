import { ToggleSwitch } from "../components/ui/ToggleSwitch"
import { useState, useSyncExternalStore, type CSSProperties } from "react"
import {
  ArrowUp,
  ArrowUpRight,
  Check,
  Cloud,
  Folder,
  Info,
  Plus,
  UserRound,
} from "lucide-react"
import appIcon from "../assets/app-icon.png"
import {
  cloudSourceKey,
  cloudSourceKind,
  cloudSourceLabel,
  desktopCloudPort,
  type CloudPort,
} from "../lib/cloud"
import { noteCloudAction } from "../lib/onboardingAnalytics"
import {
  CloudSession,
  cloudStatusMessage,
  cloudOngoingStatusMessage,
  cloudUploadStatusUnknown,
} from "./CloudSession"
import { CloudProjectFolders, cloudWorkspaceLabel } from "./CloudOnboardingFolders"
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
export { CloudProjectFolders } from "./CloudOnboardingFolders"
/** Observe one CloudSession. Completion can stay local without content consent;
 * leaving browser approval cancels only that attempt, not saved source requests.
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
  const value = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const disabled = busy || value.busy
  const grouped = !!(port ?? desktopCloudPort).ongoing
  const projects = value.projects
  const waiting = value.cloud.authEnabled && ["waiting", "opening"].includes(value.cloud.status)
  const accepted = "operationId" in value.sync
  const captureEnabled = !grouped && value.cloud.captureEnabled === true
  const captureSaved = value.sync.status === "upload_unavailable"
  const ongoing = value.lastTeamSync ?? value.ongoing
  const syncReady = grouped
    ? ongoing?.storageKnown === true && ["ready", "syncing"].includes(ongoing.status)
    : ["accepted", "syncing"].includes(value.sync.status)
  const syncing = grouped
    ? syncReady && (ongoing?.status === "syncing" || (ongoing?.queuedCount ?? 0) > 0)
    : syncReady
  const syncUncertain = ["ambiguous", "failed", "cancelled"].includes(value.sync.status)
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
  const selectedFolders = grouped
    ? (selectedProjects?.length ?? 0)
    : value.cloud.draft.folders.filter((folder) => folder.selected).length
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
      : workspaceConflict
        ? "Start syncing uses one workspace at a time. Set folders for any other workspace to Don't sync, then add them in Settings."
        : hasWorkspaceChoice
          ? "Choose a destination for each folder you want to sync. Start syncing uses one workspace at a time."
          : null
  const showWorkspaceNote = grouped && projects?.loaded && !projects.loading && !projects.error
  const folderMessage =
    error ??
    value.error ??
    (grouped
      ? (projects?.error ??
        value.cloud.setupError ??
        (!value.cloud.captureEnabled
          ? "Uploads are unavailable on this device."
          : projects?.needsChoiceReview
            ? "Review your folders before starting sync."
            : projects?.loaded &&
                !projects.loading &&
                !projects.hasMore &&
                !projects.partial &&
                projects.groups.some((group) => group.captureSupported === false)
              ? "Could not check uploads for some folders."
              : ""))
      : captureEnabled
        ? "Upload unavailable. You can save a capture request on this device."
        : value.cloud.syncEnabled
          ? ""
          : "Cloud sync is coming soon. Nothing will upload.")
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
        ? "Try Repair in Settings → CLI."
        : toolsPhase === "unavailable"
          ? "Unavailable for this setup."
          : "CLI and skill"
  const finish = async () => {
    if (busy || (value.screen !== "complete" && value.busy)) return
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
      <div className="onboarding-drag-region" data-tauri-drag-region />
      <span className="cloud-badge type-caption">
        <Cloud size={14} aria-hidden />
        antiburn cloud
      </span>
      {value.screen === "handoff" ? (
        <section className="cloud-setup" aria-label="Cloud connection">
          <div className="cloud-connection">
            <img className="cloud-app-icon" src={appIcon} alt="" />
            <h1 className="type-onboarding-title" tabIndex={-1} ref={focusHeading}>
              {value.cloud.authEnabled
                ? "Sign in to antiburn cloud"
                : "Antiburn Cloud is coming soon"}
            </h1>
            <p className="type-onboarding-body text-onboarding-muted">
              {value.cloud.authEnabled
                ? "Approve this device in your browser."
                : "Learn about bringing your session insights together across devices."}
            </p>
            {waiting ? (
              <button
                className="onboarding-secondary"
                disabled={disabled}
                onClick={() => void session.cancel()}
              >
                Cancel
              </button>
            ) : (
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
                {value.cloud.authEnabled ? "Connect in browser" : "Learn more"}
                <ArrowUpRight size={16} aria-hidden />
              </button>
            )}
            <p className="cloud-status type-footnote" role="status">
              {value.loaded
                ? value.cloud.authEnabled || value.cloud.status === "browser_failed"
                  ? cloudStatusMessage(value.cloud.status)
                  : ""
                : "Checking Cloud availability…"}
            </p>
            {value.cloud.authEnabled && (
              <p className="type-footnote text-onboarding-muted">
                You’ll return here when you’re signed in.
              </p>
            )}
          </div>
        </section>
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
            {value.error && (
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
                <button className="onboarding-secondary" disabled={disabled} onClick={onBack}>
                  Back
                </button>
              </>
            )}
          </div>
        </section>
      ) : value.screen === "folders" ? (
        <section className="cloud-folder-content min-h-0 flex-1 overflow-hidden pb-0!">
          <h1 className="type-cloud-folder-title" tabIndex={-1} ref={focusHeading}>
            {captureEnabled ? "Choose sources for " : "Choose what to sync to "}
            <span className="text-brand">antiburn cloud</span>
          </h1>
          <p id="cloud-sync-description" className="type-onboarding-body text-onboarding-muted">
            {grouped
              ? "Sync full sessions from selected folders and their worktrees."
              : captureEnabled
                ? "Choose the folders where Claude saves sessions, or choose an OpenCode V2 database."
                : "Choose folders to sync their existing and future coding sessions across your devices."}
          </p>
          <div className="flex flex-wrap items-center justify-between gap-2 type-callout text-onboarding-muted">
            <span id="cloud-sync-selection">
              {selectedFolders} {selectedFolders === 1 ? "folder" : "folders"} selected
            </span>
            {(grouped || (value.cloud.identity && destination)) && (
              <span
                id="cloud-sync-destination"
                className="min-w-0 wrap-anywhere rounded-control bg-surface-card px-2 py-1 type-footnote"
                aria-live="polite"
              >
                {grouped ? workspaceSummary : destination?.name}
                {value.cloud.identity && ` · ${value.cloud.identity.login}`}
              </span>
            )}
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
          {grouped ? (
            projects && (projects.groups.length > 0 || projectListComplete) ? (
              <CloudProjectFolders
                groups={projects.groups}
                choices={folderChoices ?? {}}
                disabled={
                  disabled ||
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
              <div className="cloud-folder-list min-h-0! max-h-none! flex-1">
                <p className="cloud-empty type-body text-onboarding-muted">
                  {emptyProjectMessage}
                </p>
              </div>
            )
          ) : (
            <div
              className="cloud-folder-list min-h-0! max-h-none! flex-1"
              aria-label="Cloud folders"
            >
              {value.cloud.draft.folders.length ? (
                value.cloud.draft.folders.map((folder) => (
                  <div key={cloudSourceKey(folder)} className="cloud-folder-row">
                    <div className="cloud-folder-label">
                      <Folder size={18} aria-hidden />
                      <span>
                        <strong className="type-title-3 font-medium!">
                          {folder.path.split(/[\\/]/).filter(Boolean).at(-1) ?? folder.path}
                        </strong>
                        <small className="type-callout text-onboarding-muted">
                          {captureEnabled && `${cloudSourceLabel(folder)}: `}
                          {folder.path}
                        </small>
                      </span>
                    </div>
                    <span className="cloud-folder-count type-callout text-onboarding-muted">
                      {folder.sessionCount == null
                        ? "Count unavailable"
                        : `${folder.sessionCount}${folder.sessionCountIsPartial ? " recent" : ""} sessions`}
                    </span>
                    <ToggleSwitch
                      aria-label={`${captureEnabled ? "Select" : "Sync"} ${folder.path}`}
                      checked={folder.selected}
                      disabled={disabled}
                      onCheckedChange={(selected) =>
                        void session.selectFolder(
                          folder.path,
                          selected,
                          cloudSourceKind(folder),
                        )
                      }
                    />
                  </div>
                ))
              ) : (
                <p className="cloud-empty type-body text-onboarding-muted">
                  {captureEnabled
                    ? "No sources added. Add a Claude source directory or OpenCode V2 database to save a capture request."
                    : "No folders added. Add a folder to choose what to sync."}
                </p>
              )}
            </div>
          )}
          {grouped && projects?.hasMore && (
            <button
              className="onboarding-secondary cloud-add"
              disabled={disabled || projects.loading}
              onClick={() => void session.loadMoreProjects()}
            >
              Load more folders
            </button>
          )}
          {!grouped && (
            <>
              <button
                className="onboarding-secondary cloud-add"
                disabled={disabled}
                onClick={() => void session.addFolders()}
              >
                <Plus size={16} aria-hidden />
                {captureEnabled ? "Add Claude sources…" : "Add a folder…"}
              </button>
              {captureEnabled && (
                <button
                  className="onboarding-secondary cloud-add"
                  disabled={disabled}
                  onClick={() => void session.addFolders("opencode_sqlite_v2")}
                >
                  <Plus size={16} aria-hidden />
                  Add OpenCode databases…
                </button>
              )}
            </>
          )}
          {folderMessage && (
            <div
              className="flex items-center gap-2 type-callout text-onboarding-muted"
              role="status"
            >
              <Info size={12} className="shrink-0" aria-hidden />
              <span>{folderMessage}</span>
              {grouped && value.cloud.captureEnabled && (
                <button
                  className="min-h-0! border-0! bg-transparent p-0! underline underline-offset-4"
                  disabled={disabled || projects?.loading}
                  onClick={() =>
                    void (value.cloud.setupError ? session.load() : session.loadProjects())
                  }
                >
                  Refresh folders
                </button>
              )}
            </div>
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
              {grouped
                ? syncReady
                  ? "Sessions sync in the background"
                  : "Continue using Antiburn locally"
                : captureEnabled
                  ? captureSaved
                    ? "Capture request saved"
                    : value.sync.status === "starting"
                      ? "Saving capture request"
                      : "Continue using Antiburn locally"
                  : value.cloud.syncEnabled
                    ? "Sessions sync in the background"
                    : "Continue using Antiburn locally"}
            </h1>
            <p className="type-onboarding-body">
              {grouped ? (
                "Manage Cloud uploads in Settings. You can pause uploads and review saved work there."
              ) : captureEnabled ? (
                "Upload is unavailable. Saving a request does not confirm that any content was read or captured."
              ) : !value.cloud.syncEnabled ? (
                (value.cloud.setupError ??
                "Cloud uploads are unavailable. Continue using Antiburn locally.")
              ) : (
                <>
                  Continue using Antiburn while your sessions sync. Manage Cloud uploads in
                  Settings.
                </>
              )}
            </p>
          </section>
        </>
      )}
      <footer className="onboarding-footer cloud-footer">
        <div className="flex w-full flex-wrap items-center justify-between gap-3">
          {value.screen === "complete" ? (
            <div className="cloud-outcomes" role="group" aria-label="Setup status">
              <div>
                <span
                  className={`cloud-sync-dot${syncing ? " is-syncing" : ""}${(grouped ? !syncReady : !accepted && !syncing) ? " is-inactive" : ""}`}
                  aria-hidden
                />
                <strong className="type-headline">
                  {grouped
                    ? ongoing && cloudUploadStatusUnknown(ongoing)
                      ? cloudOngoingStatusMessage(ongoing)
                      : value.sync.status === "starting"
                        ? "Starting background sync"
                        : value.sync.status === "ambiguous"
                          ? "Cloud sync not confirmed"
                          : ongoing
                            ? cloudOngoingStatusMessage(ongoing)
                            : "Cloud sync not confirmed"
                    : captureEnabled
                      ? "Upload unavailable"
                      : accepted || syncing
                        ? value.sync.status === "partial"
                          ? "Sync needs attention"
                          : value.sync.status === "complete"
                            ? `Synced ${selectedFolders} ${selectedFolders === 1 ? "folder" : "folders"}`
                            : `Syncing ${selectedFolders} ${selectedFolders === 1 ? "folder" : "folders"}`
                        : syncUncertain
                          ? "Sync status unavailable"
                          : value.sync.status === "starting"
                            ? "Starting background sync"
                            : "Cloud sync not started"}
                </strong>
                <span className="type-callout text-onboarding-muted">
                  {grouped
                    ? "Manage uploads in Settings"
                    : captureEnabled
                      ? captureSaved
                        ? "Capture request saved on this device"
                        : value.sync.status === "starting"
                          ? "Saving capture request"
                          : value.sync.status === "cancelled"
                            ? "Capture request cancelled"
                            : "Capture request not confirmed"
                      : accepted || syncing
                        ? value.sync.status === "partial"
                          ? "Some items could not sync"
                          : value.sync.status === "complete"
                            ? "Selected items are up to date"
                            : "Sessions upload in the background"
                        : syncUncertain
                          ? "Your folder selection is kept"
                          : selectedFolders
                            ? "Cloud sync has not started"
                            : "No folders selected for Cloud sync"}
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
            {value.screen === "folders" && (
              <>
                <button
                  className="onboarding-primary"
                  disabled={
                    disabled ||
                    !(grouped
                      ? value.cloud.captureEnabled
                      : captureEnabled || value.cloud.syncEnabled) ||
                    value.cloud.status !== "signed_in" ||
                    !!value.cloud.setupError ||
                    !value.cloud.identity ||
                    value.cloud.destination?.kind !== "personal" ||
                    !value.cloud.destination.id ||
                    (grouped
                      ? !projects?.loaded ||
                        cloudUploadStatusUnknown(value.ongoing) ||
                        projects.loading ||
                        !!projects.error ||
                        needsWorkspaceChoice ||
                        workspaceUnavailable ||
                        workspaceConflict ||
                        selectedFolders === 0
                      : !value.cloud.draft.folders.some((folder) => folder.selected))
                  }
                  aria-describedby={[
                    "cloud-sync-description",
                    "cloud-sync-selection",
                    (grouped || (value.cloud.identity && destination)) &&
                      "cloud-sync-destination",
                    showWorkspaceNote && workspaceNote && "cloud-workspace-note",
                  ]
                    .filter(Boolean)
                    .join(" ")}
                  onClick={() => void session.startSync(value)}
                >
                  {captureEnabled ? "Save capture request" : "Start syncing"}
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
