import { ToggleSwitch } from "../components/ui/ToggleSwitch"
import { useState, useSyncExternalStore, type CSSProperties } from "react"
import { ArrowUp, ArrowUpRight, Check, Cloud, Folder, Plus, UserRound } from "lucide-react"
import appIcon from "../assets/app-icon.png"
import { cloudSourceKey, cloudSourceKind, type CloudPort } from "../lib/cloud"
import { noteCloudAction } from "../lib/onboardingAnalytics"
import { CloudSession, cloudStatusMessage } from "./CloudSession"
import { CloudCaptureConsent } from "./CloudCaptureConsent"
import "./onboarding.css"
import "./cloud.css"

/** Existing optional setup flow. The native build capability gates its caller;
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
  const waiting = value.cloud.authEnabled && ["waiting", "opening"].includes(value.cloud.status)
  const accepted = "operationId" in value.sync
  const captureEnabled = value.cloud.captureEnabled === true
  const captureSaved = value.sync.status === "upload_unavailable"
  const syncing = ["accepted", "syncing"].includes(value.sync.status)
  const syncUncertain = ["ambiguous", "failed", "cancelled"].includes(value.sync.status)
  const selectedFolders = value.cloud.draft.folders.filter((folder) => folder.selected).length
  const finish = async () => {
    if (disabled) return
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
              <p className="type-callout text-onboarding-muted" role="status">
                {value.cloud.setupError} You can continue locally and manage Cloud in Settings.
              </p>
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
        <section className="cloud-folder-content">
          <h1 className="type-cloud-folder-title" tabIndex={-1} ref={focusHeading}>
            {captureEnabled ? "Choose sources for " : "Choose what to sync to "}
            <span className="text-brand">antiburn cloud</span>
          </h1>
          <p className="type-onboarding-body text-onboarding-muted">
            {captureEnabled
              ? "Choose native Claude projects/<project> source directories directly, not repositories. You can also choose an OpenCode V2 database. Antiburn does not infer sources from working directories."
              : "Choose folders to sync their existing and future coding sessions across your devices."}
          </p>
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
                      void session.selectFolder(folder.path, selected, cloudSourceKind(folder))
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
          <p className="type-callout text-onboarding-muted" role="status">
            {captureEnabled ? (
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
          <CloudCaptureConsent session={session} value={value} />
        </section>
      ) : (
        <>
          <section
            className="onboarding-stage cloud-ready-stage"
            aria-label="Example questions for your coding agent"
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
              {captureEnabled
                ? captureSaved
                  ? "Capture request saved"
                  : value.sync.status === "starting"
                    ? "Saving capture request"
                    : "Continue using Antiburn locally"
                : value.cloud.syncEnabled
                  ? "Sessions syncing! Your agent is ready"
                  : "Continue using Antiburn locally"}
            </h1>
            <p className="type-onboarding-body">
              {captureEnabled ? (
                "Upload is unavailable. Saving a request does not confirm that any content was read or captured."
              ) : !value.cloud.syncEnabled ? (
                (value.cloud.setupError ??
                "Cloud uploads are unavailable. Continue using Antiburn locally.")
              ) : (
                <>
                  {accepted || syncing ? "Ask your agent about your sessions. " : ""}Explore
                  token usage, compare costs, and find ways to waste less.
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
                className={`cloud-sync-dot${syncing ? " is-syncing" : ""}${!accepted && !syncing ? " is-inactive" : ""}`}
                aria-hidden
              />
              <strong className="type-headline">
                {captureEnabled
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
                {captureEnabled
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
                  ? "Agent setup"
                  : value.agent === "pending"
                    ? "Agent setup pending"
                    : value.agent === "failed"
                      ? "Agent status unavailable"
                      : "Agent setup unavailable"}
              </strong>
              <span className="type-callout text-onboarding-muted">
                {value.agent === "ready"
                  ? "Ask from your coding agent"
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
          {(error || value.error) && (
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
            value.screen === "identity" ||
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
                  !(captureEnabled || value.cloud.syncEnabled) ||
                  value.cloud.status !== "signed_in" ||
                  !!value.cloud.setupError ||
                  !value.cloud.identity ||
                  value.cloud.destination?.kind !== "personal" ||
                  !value.cloud.destination.id ||
                  !value.fullContentConsent ||
                  !value.cloud.draft.folders.some((folder) => folder.selected)
                }
                onClick={() => void session.startSync()}
              >
                {captureEnabled ? "Save capture request" : "Start syncing"}
              </button>
            </>
          )}
          {value.screen === "complete" && (
            <>
              {captureEnabled && captureSaved && value.sourceListingNeedsRetry && (
                <button
                  className="onboarding-secondary"
                  disabled={disabled || !value.fullContentConsent}
                  onClick={() => void session.startSync()}
                >
                  Retry source listing
                </button>
              )}
              <button
                className="onboarding-primary"
                disabled={disabled}
                onClick={() => void finish()}
              >
                Done
              </button>
            </>
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
