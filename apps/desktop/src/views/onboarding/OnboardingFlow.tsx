import { AlertTriangle, Check, FolderPlus, Lock, X } from "lucide-react"

import appIcon from "../../assets/app-icon.png"
import { useState } from "react"

import { isMacOS } from "../../lib/platform"
import { cn } from "../../lib/cn"

import { FolderPermissionNotice } from "../../components/repositories/FolderPermissionNotice"
import { LocalRepositoryList } from "../../components/repositories/LocalRepositoryList"
import { Card } from "../../components/ui/Card"
import { PushButton } from "../../components/ui/PushButton"
import { ScrollPane } from "../../components/ui/ScrollPane"
import { ToggleRow } from "../../components/ui/ToggleRow"
import { getConsentDiagnostics, openFolderAccessSettings, type ScanStatus } from "../../lib/ipc"
import type { FolderPermissions, LocalRepositoryItem } from "../../lib/types/repository"
import type { FolderPermissionFlow } from "../../lib/useFolderPermissionFlow"

export interface OnboardingFlowProps {
  /** Directories the engine searches without being asked. */
  defaultRoots: readonly string[]
  /**
   * Default roots the operating system is still guarding, so the step can say
   * "needs permission" rather than ticking a folder nothing has read.
   */
  blockedRoots: readonly string[]
  /** Which protected folders need permission, and which already have it. */
  permissions: FolderPermissions
  /** The sequential request flow the notice drives. */
  permissionFlow: FolderPermissionFlow
  /** Look for access granted in System Settings, and refresh what it changed. */
  onRecheckPermissions: () => void
  /** Whether that re-check is in flight. */
  recheckingPermissions: boolean
  /** Extra directories the reader has added so far. */
  scanRoots: readonly string[]
  /** Open a directory picker and add the result. */
  onAddScanRoot: () => void
  onRemoveScanRoot: (path: string) => void
  /** Repositories the first discovery pass found. */
  repositories: readonly LocalRepositoryItem[]
  /** Include or ignore one repository. */
  onToggleRepository: (item: LocalRepositoryItem, enabled: boolean) => void
  /** Run a discovery pass. Called when a step needs fresh results. */
  onDiscover: () => void
  /** The shell's scan status, or null before the first read. */
  scanStatus: ScanStatus | null
  /** Whether the installed app should start after the reader signs in. */
  launchAtLogin: boolean
  onLaunchAtLoginChange: (enabled: boolean) => void
  /** Finish: records the flag and enters the activity view. */
  onFinish: () => void
  finishing: boolean
  finishError: string | null
}

const STEPS = ["welcome", "sourcesAndRepos", "ready"] as const
type Step = (typeof STEPS)[number]

function focusHeading(heading: HTMLHeadingElement | null): void {
  heading?.focus()
}

function StepDots({ step }: { step: Step }) {
  const index = STEPS.indexOf(step)
  return (
    <div className="flex items-center justify-center gap-1.5" aria-hidden="true">
      {STEPS.map((name, position) => (
        <span
          key={name}
          className={cn(
            "h-1.5 w-1.5 rounded-full transition-colors duration-[var(--duration-fast)] ease-out",
            position === index ? "bg-label-secondary" : "bg-label/20",
          )}
        />
      ))}
    </div>
  )
}

const CENTRED_COLUMN = "mx-auto flex max-w-[440px] flex-col items-center"

function Welcome() {
  return (
    <div className="flex flex-1 flex-col items-center justify-center px-8 text-center overflow-y-auto">
      <div className={CENTRED_COLUMN}>
        <img
          src={appIcon}
          alt=""
          aria-hidden="true"
          width={96}
          height={96}
          className="mb-5 h-24 w-24 select-none drop-shadow-md"
          draggable={false}
        />
        <h2 ref={focusHeading} tabIndex={-1} className="type-title-3 text-label outline-none">
          Stop hitting your token limits.
        </h2>
        <p className="mt-2 text-balance type-callout text-label-secondary">
          antiburn reads your coding agent session logs and analyses them locally.
        </p>
        <p className="mt-2 text-balance type-callout text-label-secondary">
          No account needed, and nothing from your sessions is ever uploaded.
        </p>
      </div>
    </div>
  )
}

function SourcesAndRepos({
  blockedRoots,
  defaultRoots,
  permissionFlow,
  permissions,
  recheckingPermissions,
  repositories,
  scanError,
  scanning,
  scanRoots,
  onAddScanRoot,
  onRecheckPermissions,
  onRemoveScanRoot,
  onRetryScan,
  onToggleRepository,
}: Pick<
  OnboardingFlowProps,
  "defaultRoots" | "scanRoots" | "onAddScanRoot" | "onRemoveScanRoot"
> & {
  blockedRoots: readonly string[]
  permissionFlow: FolderPermissionFlow
  permissions: FolderPermissions
  recheckingPermissions: boolean
  repositories: readonly LocalRepositoryItem[]
  scanError: string | null
  scanning: boolean
  onRecheckPermissions: () => void
  onRetryScan: () => void
  onToggleRepository: (item: LocalRepositoryItem, enabled: boolean) => void
}) {
  const scanFailed = !scanning && scanError !== null

  return (
    <div className="grid grid-cols-2 h-full">
      <div className="flex min-h-0 flex-col px-8">
        <h2 ref={focusHeading} tabIndex={-1} className="type-title-3 text-label outline-none">
          Repo search locations
        </h2>

        <ScrollPane className="mt-3" viewportClassName="pr-1">
          {defaultRoots.length > 0 && (
            <>
              <p className="pb-1 type-footnote font-semibold! text-label-tertiary">Defaults</p>
              <ul className="space-y-0.5 pb-3">
                {defaultRoots.map((root) => {
                  const blocked = blockedRoots.includes(root)
                  return (
                    <li key={root} className="flex items-center gap-1.5">
                      {blocked ? (
                        <Lock
                          size={11}
                          strokeWidth={2.5}
                          aria-hidden="true"
                          className="shrink-0 text-label-tertiary"
                        />
                      ) : (
                        <Check
                          size={11}
                          strokeWidth={2.5}
                          aria-hidden="true"
                          className="shrink-0 text-label-tertiary"
                        />
                      )}
                      <span
                        dir="rtl"
                        className="truncate text-left type-footnote text-label-secondary"
                      >
                        <bdi>{root}</bdi>
                      </span>
                      {blocked ? (
                        <span className="shrink-0 type-caption text-label-tertiary">
                          needs permission
                        </span>
                      ) : null}
                    </li>
                  )
                })}
              </ul>
            </>
          )}

          <div className="flex gap-x-3">
            <p className="pb-1 type-footnote font-semibold! text-label-tertiary">
              Added by you
            </p>

            <PushButton className="gap-1.5" onClick={onAddScanRoot}>
              <FolderPlus size={12} aria-hidden="true" />
              Add a folder…
            </PushButton>
          </div>
          {scanRoots.length === 0 ? (
            <p className="pb-2 type-footnote italic text-label-tertiary">
              Locations you add here will also be searched.
            </p>
          ) : (
            <ul className="space-y-0.5 pb-2">
              {scanRoots.map((root) => (
                <li key={root} className="flex items-center gap-1.5">
                  <span
                    dir="rtl"
                    className="min-w-0 flex-1 truncate text-left type-footnote text-label-secondary"
                  >
                    <bdi>{root}</bdi>
                  </span>
                  <button
                    type="button"
                    onClick={() => onRemoveScanRoot(root)}
                    aria-label={`Stop scanning ${root}`}
                    className="shrink-0 rounded-control p-0.5 text-label-tertiary hover:bg-surface-hover hover:text-label-secondary"
                  >
                    <X size={11} strokeWidth={2.5} aria-hidden="true" />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </ScrollPane>
      </div>

      <div className="flex min-h-0 flex-col px-8">
        <h2 className="type-title-3 text-label">Repos found</h2>
        {permissions.supported && permissions.deferred.length > 0 ? (
          <div className="mt-2">
            <FolderPermissionNotice
              deferred={permissions.deferred}
              phase={permissionFlow.phase}
              current={permissionFlow.current}
              position={permissionFlow.position}
              total={permissionFlow.total}
              recordedDenials={permissionFlow.recordedDenials}
              onRequest={permissionFlow.start}
              onOpenSettings={() => void openFolderAccessSettings()}
              onRecheck={onRecheckPermissions}
              rechecking={recheckingPermissions}
              onCopyDiagnostics={() => {
                void getConsentDiagnostics().then((probes) =>
                  navigator.clipboard.writeText(
                    probes
                      .map((probe) => `${probe.outcome}\t${probe.elapsedMs}ms\t${probe.target}`)
                      .join("\n") || "No folder-access probes this run.",
                  ),
                )
              }}
            />
          </div>
        ) : null}
        {scanFailed ? (
          <Card className="mt-2 shrink-0">
            <div className="flex items-start gap-2 px-3 py-2" role="alert">
              <AlertTriangle
                size={14}
                strokeWidth={2}
                aria-hidden="true"
                className="mt-0.5 shrink-0 text-system-orange"
              />
              <div className="min-w-0 flex-1">
                <p className="type-callout font-medium! text-label">Scan did not finish</p>
                <p className="mt-0.5 break-words type-footnote text-label-secondary">
                  {scanError}
                </p>
              </div>
              <PushButton className="shrink-0" onClick={onRetryScan} disabled={scanning}>
                Try again
              </PushButton>
            </div>
          </Card>
        ) : null}
        <div className="mt-2 min-h-0 flex-1">
          {scanFailed && repositories.length === 0 ? (
            <div className="flex h-full items-center justify-center px-6 text-center">
              <p className="type-footnote text-label-tertiary">
                Check the scan folders and folder access, then try again.
              </p>
            </div>
          ) : (
            <LocalRepositoryList
              repositories={[...repositories]}
              loading={scanning}
              onToggleRepository={onToggleRepository}
              emptyTitle="Nothing found yet"
              emptyDescription="Repositories appear once a coding session has run in one. You can change this later in Settings."
            />
          )}
        </div>
      </div>
    </div>
  )
}

function Ready({
  sessions,
  launchAtLogin,
  onLaunchAtLoginChange,
  finishError,
}: {
  sessions: number
  launchAtLogin: boolean
  onLaunchAtLoginChange: (enabled: boolean) => void
  finishError: string | null
}) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center px-8 text-center">
      <div className={CENTRED_COLUMN}>
        <div className="mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-surface-secondary text-label-secondary">
          <Check size={22} strokeWidth={2} aria-hidden="true" />
        </div>
        <h2
          ref={focusHeading}
          tabIndex={-1}
          className="type-title-3 text-label outline-none mt-1"
        >
          Ready
        </h2>
        <p className="mt-2 text-balance type-callout text-label-secondary">
          {sessions > 0
            ? `${sessions} ${sessions === 1 ? "session is" : "sessions are"} indexed and waiting in the menu bar.`
            : "Nothing is indexed yet — antiburn keeps looking in the background as you work."}
        </p>
        <p className="mt-2 text-balance type-footnote text-label-tertiary">
          Session files are read only; your repositories are never modified.
        </p>
        <Card className="mt-12 w-full text-left">
          <ToggleRow
            label="Launch antiburn on startup"
            description="Starts automatically in the menu bar. Change anytime in Settings."
            checked={launchAtLogin}
            onChange={onLaunchAtLoginChange}
          />
        </Card>
        {finishError ? (
          <p className="mt-3 type-footnote text-system-red" role="alert">
            {finishError}
          </p>
        ) : null}
      </div>
    </div>
  )
}

export function OnboardingFlow({
  defaultRoots,
  blockedRoots,
  permissions,
  permissionFlow,
  onRecheckPermissions,
  recheckingPermissions,
  scanRoots,
  onAddScanRoot,
  onRemoveScanRoot,
  repositories,
  onToggleRepository,
  onDiscover,
  scanStatus,
  launchAtLogin,
  onLaunchAtLoginChange,
  onFinish,
  finishing,
  finishError,
}: OnboardingFlowProps) {
  const [step, setStep] = useState<Step>("welcome")
  const [discoveryRequested, setDiscoveryRequested] = useState(false)
  const index = STEPS.indexOf(step)
  const last = index === STEPS.length - 1
  const scanSucceeded =
    scanStatus?.finishedAt != null && scanStatus.error === null && !scanStatus.cancelled
  const running = scanStatus?.running ?? false

  const advance = () => {
    const next = STEPS[index + 1] ?? "ready"
    if (next === "sourcesAndRepos" && !scanSucceeded && !running && !discoveryRequested) {
      setDiscoveryRequested(true)
      onDiscover()
    }
    setStep(next)
  }

  return (
    <div
      className="grid h-full grid-rows-[auto_minmax(0,1fr)_auto]"
      aria-label="Set up antiburn"
      role="region"
    >
      {isMacOS() && (
        // The native title bar is an overlay here (`src-tauri/src/onboarding.rs`),
        // so this transparent strip is the window's drag handle. Same treatment
        // as the settings window: h-10 is a more forgiving grab target than the
        // 28pt bar itself, and any future child must be pointer-events-none,
        // since `data-tauri-drag-region` only starts a drag when the mousedown
        // lands on this element. Double-click no-ops — the window is neither
        // resizable nor maximizable.
        <div
          data-tauri-drag-region
          className="absolute inset-x-0 top-0 z-10 h-10"
          aria-hidden="true"
        />
      )}
      {/* On macOS the header IS the drag strip, and the traffic lights live in
          it. The wordmark is hidden there rather than inset past them: an
          inset is a number that has to keep matching AppKit's button
          placement, and it buys nothing — the window is already named by its
          title bar, by the app icon on the Welcome step, and by every step's
          own heading. Windows and Linux keep the native bar and show it. */}
      <header className="flex h-11 items-center px-4">
        <h1 className={cn("type-headline text-label", isMacOS() && "sr-only")}>antiburn</h1>
        <p className="sr-only" aria-live="polite">
          Step {index + 1} of {STEPS.length}
        </p>
      </header>

      <div className={cn("min-h-0", step != "sourcesAndRepos" && "self-center")}>
        {step === "welcome" && <Welcome />}
        {step === "sourcesAndRepos" && (
          <SourcesAndRepos
            blockedRoots={blockedRoots}
            defaultRoots={defaultRoots}
            permissions={permissions}
            permissionFlow={permissionFlow}
            recheckingPermissions={recheckingPermissions}
            repositories={repositories}
            scanError={scanStatus?.error ?? null}
            scanning={running}
            scanRoots={scanRoots}
            onAddScanRoot={onAddScanRoot}
            onRecheckPermissions={onRecheckPermissions}
            onRemoveScanRoot={onRemoveScanRoot}
            onRetryScan={onDiscover}
            onToggleRepository={onToggleRepository}
          />
        )}
        {step === "ready" && (
          <Ready
            finishError={finishError}
            launchAtLogin={launchAtLogin}
            sessions={scanStatus?.sessions ?? 0}
            onLaunchAtLoginChange={onLaunchAtLoginChange}
          />
        )}
      </div>

      <footer className="grid grid-cols-[1fr_max-content_1fr] items-center gap-2 border-t border-separator px-4 py-3">
        <div>
          {index > 0 && (
            <PushButton onClick={() => setStep(STEPS[index - 1] ?? "welcome")}>Back</PushButton>
          )}
        </div>
        <StepDots step={step} />
        <PushButton
          className="justify-self-end"
          variant="primary"
          onClick={() => (last ? onFinish() : advance())}
          disabled={last && finishing}
        >
          {last ? (finishing ? "Finishing…" : "Start using antiburn") : "Continue"}
        </PushButton>
      </footer>
    </div>
  )
}
