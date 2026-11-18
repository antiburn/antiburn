import { ToggleSwitch } from "../components/ui/ToggleSwitch"
import { useState, useSyncExternalStore, type CSSProperties } from "react"
import { ArrowUp, ArrowUpRight, Check, Cloud, Folder, Plus, UserRound } from "lucide-react"
import appIcon from "../assets/app-icon.png"
import {
  cloudSourceKey,
  cloudSourceKind,
  desktopCloudPort,
  type CloudPort,
  type CloudProjectGroup,
  type CloudProjectChoices,
} from "../lib/cloud"
import { noteCloudAction } from "../lib/onboardingAnalytics"
import {
  CloudSession,
  cloudStatusMessage,
  cloudOngoingStatusMessage,
  cloudUploadStatusUnknown,
} from "./CloudSession"
import { CloudCaptureConsent } from "./CloudCaptureConsent"
import { CloudProjectPaths } from "./CloudProjectPaths"
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
/** Show one selection control per desktop project group using the existing rows.
 * The caller owns local choices and Start. Rendering new selected defaults does
 * not save preferences or approve new members. Counts never determine consent.
 */
export function CloudProjectFolders({
  groups,
  choices,
  disabled = false,
  onSelect,
}: {
  groups: readonly CloudProjectGroup[]
  choices: CloudProjectChoices
  disabled?: boolean
  onSelect: (projectId: string, selected: boolean) => void
}) {
  return (
    <div className="cloud-folder-list shrink-0" aria-label="Cloud project folders">
      {groups.length ? (
        groups.map((group) => (
          <div key={group.id} className="cloud-folder-row">
            <div className="cloud-folder-label">
              <Folder size={18} aria-hidden />
              <div className="min-w-0 flex-1">
                <strong className="type-title-3 font-medium!">{group.name}</strong>
                <small
                  className="type-callout text-onboarding-muted"
                  title={group.memberPaths.join("\n")}
                >
                  {group.path}
                </small>
                <div className="text-onboarding-muted">
                  <CloudProjectPaths name={group.name} paths={group.memberPaths} />
                </div>
                {group.captureSupported === false && (
                  <small className="type-callout text-onboarding-muted">
                    Cloud upload unavailable
                  </small>
                )}
              </div>
            </div>
            <span className="cloud-folder-count type-callout text-onboarding-muted">
              {group.sessionCount == null
                ? "Count unavailable"
                : `${group.sessionCount} ${group.sessionCount === 1 ? "session" : "sessions"}`}
              {group.worktreeCount > 0 && (
                <small className="type-callout text-onboarding-muted">
                  {group.worktreeCount} {group.worktreeCount === 1 ? "worktree" : "worktrees"}
                </small>
              )}
            </span>
            <ToggleSwitch
              aria-label={`Sync ${group.name}: ${group.path}`}
              checked={choices[group.id] ?? true}
              disabled={disabled}
              onCheckedChange={(selected) => onSelect(group.id, selected)}
            />
          </div>
        ))
      ) : (
        <p className="cloud-empty type-body text-onboarding-muted">
          No project folders found in your available sessions.
        </p>
      )}
    </div>
  )
}
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
  const syncReady = grouped
    ? value.ongoing?.storageKnown === true &&
      ["ready", "syncing"].includes(value.ongoing.status)
    : ["accepted", "syncing"].includes(value.sync.status)
  const syncing = grouped
    ? syncReady &&
      (value.ongoing?.status === "syncing" || (value.ongoing?.queuedCount ?? 0) > 0)
    : syncReady
  const syncUncertain = ["ambiguous", "failed", "cancelled"].includes(value.sync.status)
  const selectedFolders = grouped
    ? (projects?.groups.filter((group) => projects.choices[group.id] ?? true).length ?? 0)
    : value.cloud.draft.folders.filter((folder) => folder.selected).length
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
      className={`onboarding-shell cloud-shell cloud-screen-${value.screen}`}
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
                ? "Approve this desktop connection in your browser."
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
        <section className="cloud-folder-content overflow-y-auto">
          <h1 className="type-cloud-folder-title" tabIndex={-1} ref={focusHeading}>
            {captureEnabled ? "Choose sources for " : "Choose what to sync to "}
            <span className="text-brand">antiburn cloud</span>
          </h1>
          <p className="type-onboarding-body text-onboarding-muted">
            {grouped
              ? "Each project includes its current worktrees. Your choices stay local until you start syncing."
              : captureEnabled
                ? "Choose the folders where Claude saves sessions, or choose an OpenCode V2 database."
                : "Choose folders to sync their existing and future coding sessions across your devices."}
          </p>
          {grouped ? (
            projects?.loaded ? (
              <CloudProjectFolders
                groups={projects.groups}
                choices={projects.choices}
                disabled={disabled || projects.loading}
                onSelect={session.selectProject}
              />
            ) : (
              <div className="cloud-folder-list">
                <p className="cloud-empty type-body text-onboarding-muted">
                  {projects?.error ?? "Loading project folders…"}
                </p>
              </div>
            )
          ) : (
            <div className="cloud-folder-list" aria-label="Cloud folders">
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
                          {captureEnabled &&
                            `${cloudSourceKind(folder) === "opencode_sqlite_v2" ? "OpenCode V2" : "Claude"}: `}
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
          <p className="type-callout text-onboarding-muted" role="status">
            {grouped ? (
              (projects?.error ??
              (!value.cloud.captureEnabled
                ? "Uploads are unavailable on this device. Continue locally and review Cloud in Settings."
                : projects?.loaded && !projects.groups.length
                  ? "Continue locally, then choose your Claude or OpenCode session files in Cloud Settings."
                  : projects?.needsChoiceReview
                    ? "Review which folders you want to upload, then confirm below. Existing uploads keep their saved selection."
                    : projects?.hasMore
                      ? "More project folders are available. Start approves only the folders currently shown."
                      : projects?.loaded &&
                          !projects.groups.some((group) => group.captureSupported)
                        ? "These sessions do not support Cloud uploads yet. Continue locally or review session file access in Cloud Settings."
                        : projects?.partial
                          ? "Some session folders could not be checked. You can review file access in Cloud Settings."
                          : "Existing and new sessions can sync while Upload sessions is on. Manage uploads in Settings."))
            ) : captureEnabled ? (
              "Upload unavailable. You can save a capture request on this device."
            ) : value.cloud.syncEnabled ? (
              <>
                New sessions in selected folders will sync too.
                <br />
                You can change your selection in Settings.
              </>
            ) : (
              "Cloud sync is coming soon. Your folder selection is saved, but nothing will upload."
            )}
          </p>
          {(!grouped || !!projects?.groups.length) && (
            <CloudCaptureConsent
              session={session}
              value={value}
              {...(grouped
                ? {
                    projects: {
                      groups: projects?.groups ?? [],
                      choices: projects?.choices ?? {},
                      selectionKey: projects?.selectionKey ?? "",
                      loading: !projects?.loaded || projects.loading,
                      error: projects?.error ?? null,
                    },
                  }
                : {})}
            />
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
        {value.screen === "complete" ? (
          <div className="cloud-outcomes" role="group" aria-label="Setup status">
            <div>
              <span
                className={`cloud-sync-dot${syncing ? " is-syncing" : ""}${(grouped ? !syncReady : !accepted && !syncing) ? " is-inactive" : ""}`}
                aria-hidden
              />
              <strong className="type-headline">
                {grouped
                  ? value.ongoing && cloudUploadStatusUnknown(value.ongoing)
                    ? cloudOngoingStatusMessage(value.ongoing)
                    : value.sync.status === "starting"
                      ? "Starting background sync"
                      : value.sync.status === "ambiguous"
                        ? "Cloud sync not confirmed"
                        : value.ongoing
                          ? cloudOngoingStatusMessage(value.ongoing)
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
              {value.agent === "ready" ? (
                <Check className="cloud-agent-tick" size={12} aria-hidden />
              ) : (
                <span aria-hidden />
              )}
              <strong className="type-headline">
                {value.agent === "ready"
                  ? "Local agent ready"
                  : value.agent === "pending"
                    ? "Local agent setup pending"
                    : value.agent === "failed"
                      ? "Local agent status unavailable"
                      : "Local agent setup unavailable"}
              </strong>
              <span className="type-callout text-onboarding-muted">
                {value.agent === "ready"
                  ? "Local integration is active"
                  : "Manage integrations in Settings"}
              </span>
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
        <div className="onboarding-actions">
          {(error || (value.screen !== "complete" && value.error)) && (
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
              className="onboarding-secondary"
              disabled={disabled}
              onClick={() => void finish()}
            >
              Continue locally
            </button>
          )}
          {value.screen === "folders" && (
            <>
              <span className="type-footnote text-onboarding-muted">
                {selectedFolders} {selectedFolders === 1 ? "folder" : "folders"} selected
              </span>

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
                  !value.fullContentConsent ||
                  (grouped
                    ? !projects?.loaded ||
                      cloudUploadStatusUnknown(value.ongoing) ||
                      projects.loading ||
                      !!projects.error ||
                      value.fullContentConsentKey !== projects.selectionKey ||
                      !projects.groups.some(
                        (group) =>
                          group.captureSupported && (projects.choices[group.id] ?? true),
                      )
                    : !value.cloud.draft.folders.some((folder) => folder.selected))
                }
                onClick={() => void session.startSync()}
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
      </footer>
    </main>
  )
}
function subscribeVisibility(listener: () => void) {
  document.addEventListener("visibilitychange", listener)
  return () => document.removeEventListener("visibilitychange", listener)
}
