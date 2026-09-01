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
import { ToggleSwitch } from "../../components/ui/ToggleSwitch"
import { renderAgentIcon } from "../../lib/agentIcon"
import { AGENT_SLUGS, agentDisplayName } from "../../lib/presentation/agents"
import { sessionHygieneCheckName } from "../../lib/presentation/sessionHygiene"
import { getConsentDiagnostics, openFolderAccessSettings, type ScanStatus } from "../../lib/ipc"
import type { HygieneSummary } from "../../lib/insightsIpc"
import type { FolderPermissions, LocalRepositoryItem } from "../../lib/types/repository"
import type { FolderPermissionFlow } from "../../lib/useFolderPermissionFlow"
import type { OnboardingStep } from "./OnboardingSession"

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
  /** Draft of the disabled-agent display filter. Persisted on finish. */
  disabledAgents: readonly string[]
  /** Show or hide one agent's sessions. Sessions stay indexed either way. */
  onAgentEnabledChange: (slug: string, enabled: boolean) => void
  /** Whether the installed app should start after the reader signs in. */
  launchAtLogin: boolean
  onLaunchAtLoginChange: (enabled: boolean) => void
  /** Draft of the Do Not Disturb opt-in. Persisted on finish. */
  nudgesRespectDnd: boolean
  onNudgesRespectDndChange: (enabled: boolean) => void
  /** The activity window, in days, that scopes the session numbers. */
  activityWindowDays: number
  /** Aggregate analysis numbers for the Ready step, or null before the
   *  first read. */
  hygieneSummary: HygieneSummary | null
  /** The Ready step is now visible. The session starts summary polling. */
  onReadyEntered: () => void
  /** Count one step. The session deduplicates repeated navigation. */
  onStepViewed: (step: OnboardingStep) => void
  /** Finish: records the flag and enters the activity view. */
  onFinish: () => void
  finishing: boolean
  finishError: string | null
}

const STEPS = ["welcome", "agentsDetected", "sourcesAndRepos", "ready"] as const
type Step = (typeof STEPS)[number]

const ANALYTICS_STEP: Record<Step, OnboardingStep> = {
  welcome: "welcome",
  agentsDetected: "agents_detected",
  sourcesAndRepos: "sources_and_repos",
  ready: "ready",
}

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
        <h2 ref={focusHeading} tabIndex={-1} className="type-title-1 text-label outline-none">
          Stop hitting your token limits.
        </h2>
        <p className="mt-2.5 text-balance type-body text-label-secondary">
          antiburn reads your coding agent session logs and analyses them locally.
        </p>
        <p className="mt-2 text-balance type-body text-label-secondary">
          No account needed, and nothing from your sessions is ever uploaded.
        </p>
      </div>
    </div>
  )
}

function AgentsDetected({
  scanStatus,
  disabledAgents,
  onAgentEnabledChange,
}: {
  scanStatus: ScanStatus | null
  disabledAgents: readonly string[]
  onAgentEnabledChange: (slug: string, enabled: boolean) => void
}) {
  const detected = (scanStatus?.agents ?? [])
    .filter((entry) => entry.sessionsSeen > 0)
    .sort((a, b) => b.sessionsSeen - a.sessionsSeen)
  const detectedSlugs = new Set(detected.map((entry) => entry.agent))
  const quiet = AGENT_SLUGS.filter((slug) => !detectedSlugs.has(slug))
  const isEnabled = (slug: string) => !disabledAgents.includes(slug)

  return (
    <div className="flex h-full min-h-0 flex-col px-8">
      <h2 ref={focusHeading} tabIndex={-1} className="type-title-3 text-label outline-none">
        Scan Locations: Agents
      </h2>
      <p className="mt-1.5 type-callout text-label-secondary">
        antiburn does constant background session scans from agents you enable.
      </p>

      <ScrollPane className="mt-3" viewportClassName="pr-1">
        {detected.length > 0 ? (
          <Card>
            {detected.map((entry) => (
              <div key={entry.agent} className="flex items-center gap-2.5 px-3 py-1.5">
                <span className="flex w-5 shrink-0 justify-center">
                  {renderAgentIcon(entry.agent, 16)}
                </span>
                <span className="type-callout font-semibold! text-label">
                  {agentDisplayName(entry.agent)}
                </span>
                <span className="flex-1 type-footnote text-label-tertiary">
                  {entry.sessionsSeen} {entry.sessionsSeen === 1 ? "session" : "sessions"}
                </span>
                <ToggleSwitch
                  checked={isEnabled(entry.agent)}
                  onCheckedChange={(next) => onAgentEnabledChange(entry.agent, next)}
                  aria-label={`Show ${agentDisplayName(entry.agent)} sessions`}
                />
              </div>
            ))}
          </Card>
        ) : null}

        {quiet.length > 0 ? (
          <>
            <p
              className={cn(
                "pb-1.5 type-footnote font-semibold! text-label-tertiary",
                detected.length > 0 ? "mt-3.5" : "mt-1",
              )}
            >
              No sessions found
            </p>
            <Card className="grid grid-cols-2 divide-y-0">
              {quiet.map((slug, position) => (
                <div
                  key={slug}
                  className={cn(
                    "flex items-center gap-2.5 border-separator px-3 py-1",
                    position >= 2 && "border-t",
                    position % 2 === 0 && "border-r",
                  )}
                >
                  <span className="flex w-5 shrink-0 justify-center">
                    {renderAgentIcon(slug, 14)}
                  </span>
                  <span className="flex-1 truncate type-footnote text-label-secondary">
                    {agentDisplayName(slug)}
                  </span>
                  <ToggleSwitch
                    checked={isEnabled(slug)}
                    onCheckedChange={(next) => onAgentEnabledChange(slug, next)}
                    aria-label={`Show ${agentDisplayName(slug)} sessions`}
                  />
                </div>
              ))}
            </Card>
          </>
        ) : null}
      </ScrollPane>
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

  // A repository that is not on this machine carries no switch, so "all" means
  // the repositories the reader can actually turn on.
  const toggleable = repositories.filter((item) => item.status !== "not_cloned")
  const allEnabled = toggleable.length > 0 && toggleable.every((item) => item.enabled)
  const [choosingRepos, setChoosingRepos] = useState(false)
  // Every repository is on and the reader has not asked to see them, so the
  // list stays closed.
  const scanningAll = allEnabled && !choosingRepos

  function handleScanAllRepos(next: boolean): void {
    if (next) {
      // Turn each repository back on, then close the list again.
      for (const item of toggleable) {
        if (!item.enabled) onToggleRepository(item, true)
      }
      setChoosingRepos(false)
      return
    }
    // Turning this off opens the list. It disables no repository.
    setChoosingRepos(true)
  }

  return (
    <div className="grid h-full grid-cols-2 gap-x-8 px-8">
      <div className="col-span-full mb-4 flex flex-col gap-1.5">
        <h2 ref={focusHeading} tabIndex={-1} className="type-title-3 text-label outline-none">
          Scan Locations: Repos
        </h2>

        <p className="type-callout text-label-secondary">
          antiburn will only scan sessions from repos enabled here.
        </p>
      </div>

      <div className="flex min-h-0 flex-col">
        <h3 className="border-b border-separator pb-1 type-body-large" tabIndex={-1}>
          Folders to scan
        </h3>

        <ScrollPane className="mt-2.5" viewportClassName="pr-1">
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
              Add Locations…
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

      <div className="flex min-h-0 flex-col">
        <h3 className="border-b border-separator pb-1 type-body-large" tabIndex={-1}>
          Repos found
        </h3>

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
        {toggleable.length > 0 ? (
          <div className="mt-2 flex items-center gap-3 border-b border-separator pb-2">
            <div className="min-w-0 flex-1">
              <p className="type-callout text-label">All repos</p>
              <p className="type-caption text-label-tertiary">
                {scanningAll
                  ? `antiburn scans each of the ${toggleable.length} repos it found.`
                  : "Choose the repos antiburn scans."}
              </p>
            </div>
            <ToggleSwitch
              checked={scanningAll}
              onCheckedChange={handleScanAllRepos}
              aria-label="Scan all repos"
            />
          </div>
        ) : null}
        <div className="mt-2 min-h-0 flex-1">
          {scanningAll ? null : scanFailed && repositories.length === 0 ? (
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

/** "Claude Code, Codex and Cursor" from a display-name list. */
function joinNames(names: readonly string[]): string {
  if (names.length <= 1) return names[0] ?? ""
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`
}

/**
 * The fixed-height slot between the Ready sentence and the toggle rows.
 *
 * The results card and the analyzing bar render at the same height, so the
 * heading and the toggles never move when analysis finishes.
 */
function ReadyStatSlot({ summary }: { summary: HygieneSummary | null }) {
  const content = () => {
    if (summary === null || summary.totalSessions === 0) return null
    if (summary.settledSessions < summary.totalSessions) {
      const progress = Math.round((summary.settledSessions / summary.totalSessions) * 100)
      return (
        <div className="mx-auto w-full max-w-[300px]">
          <div className="flex items-baseline gap-2">
            <p className="type-footnote text-label-secondary">Analyzing sessions</p>
            <span className="flex-1" />
            <p className="font-mono type-footnote text-label-tertiary">
              {summary.settledSessions} of {summary.totalSessions}
            </p>
          </div>
          <div className="mt-2 h-1 w-full overflow-hidden rounded-full bg-surface-secondary">
            <div
              className="h-full rounded-full bg-accent-fill"
              style={{ width: `${progress}%` }}
            />
          </div>
        </div>
      )
    }
    const passing =
      summary.analyzedSessions > 0
        ? Math.round(
            ((summary.analyzedSessions - summary.failingSessions) / summary.analyzedSessions) *
              100,
          )
        : null
    return (
      <Card className="grid w-full grid-cols-[1fr_1fr_1.5fr] divide-x divide-y-0 divide-separator">
        <div className="flex flex-col items-center justify-center px-3 py-2.5">
          <p className="font-mono type-title-2 text-label">{summary.analyzedSessions}</p>
          <p className="mt-0.5 type-footnote text-label-tertiary">sessions analyzed</p>
        </div>
        <div className="flex flex-col items-center justify-center px-3 py-2.5">
          <p className="font-mono type-title-2 text-accent">
            {passing === null ? "–" : `${passing}%`}
          </p>
          <p className="mt-0.5 type-footnote text-label-tertiary">pass the session checks</p>
        </div>
        <div className="flex flex-col items-center justify-center px-3 py-2.5">
          <p className="text-balance type-headline text-label">
            {summary.mostCommonFinding === null
              ? "None"
              : sessionHygieneCheckName(summary.mostCommonFinding)}
          </p>
          <p className="mt-0.5 type-footnote text-label-tertiary">most common failure</p>
        </div>
      </Card>
    )
  }
  return <div className="mt-4 flex h-[88px] w-full items-center">{content()}</div>
}

function Ready({
  sessions,
  windowDays,
  agentNames,
  hygieneSummary,
  launchAtLogin,
  onLaunchAtLoginChange,
  nudgesRespectDnd,
  onNudgesRespectDndChange,
  finishError,
}: {
  sessions: number
  windowDays: number
  agentNames: readonly string[]
  hygieneSummary: HygieneSummary | null
  launchAtLogin: boolean
  onLaunchAtLoginChange: (enabled: boolean) => void
  nudgesRespectDnd: boolean
  onNudgesRespectDndChange: (enabled: boolean) => void
  finishError: string | null
}) {
  const dayClause = windowDays === 1 ? "the last day" : `the last ${windowDays} days`
  const agentClause = agentNames.length > 0 ? ` across ${joinNames(agentNames)}` : ""
  const sentence =
    sessions > 0
      ? `${sessions} ${sessions === 1 ? "session" : "sessions"} from ${dayClause}${agentClause} ${
          sessions === 1 ? "is" : "are"
        } indexed and waiting in the menu bar.`
      : "Looking for coding sessions…"

  return (
    <div className="flex flex-1 flex-col items-center justify-center px-8 text-center">
      <div className={cn(CENTRED_COLUMN, "w-full")}>
        <div className="mb-3 flex h-12 w-12 items-center justify-center rounded-full bg-surface-secondary text-label-secondary">
          <Check size={22} strokeWidth={2} aria-hidden="true" />
        </div>
        <h2 ref={focusHeading} tabIndex={-1} className="type-title-1 text-label outline-none">
          Ready
        </h2>
        <p className="mt-2.5 text-balance type-body text-label-secondary">{sentence}</p>
        <ReadyStatSlot summary={hygieneSummary} />
        <div className="mt-4 flex w-full flex-col gap-2 text-left">
          <div className="flex items-center gap-3">
            <span className="type-callout text-label">Launch antiburn on startup</span>
            <span className="flex-1" />
            <ToggleSwitch
              checked={launchAtLogin}
              onCheckedChange={onLaunchAtLoginChange}
              aria-label="Launch antiburn on startup"
            />
          </div>
          <div className="flex items-center gap-3">
            <span className="type-callout text-label">Nudges respect Do Not Disturb</span>
            <span className="flex-1" />
            <ToggleSwitch
              checked={nudgesRespectDnd}
              onCheckedChange={onNudgesRespectDndChange}
              aria-label="Nudges respect Do Not Disturb"
            />
          </div>
        </div>
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
  disabledAgents,
  onAgentEnabledChange,
  launchAtLogin,
  onLaunchAtLoginChange,
  nudgesRespectDnd,
  onNudgesRespectDndChange,
  activityWindowDays,
  hygieneSummary,
  onReadyEntered,
  onStepViewed,
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
    if (next === "agentsDetected" && !scanSucceeded && !running && !discoveryRequested) {
      setDiscoveryRequested(true)
      onDiscover()
    }
    if (next === "ready") onReadyEntered()
    onStepViewed(ANALYTICS_STEP[next])
    setStep(next)
  }

  const goBack = () => {
    const previous = STEPS[index - 1] ?? "welcome"
    onStepViewed(ANALYTICS_STEP[previous])
    setStep(previous)
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

      <div className={cn("min-h-0", (step === "welcome" || step === "ready") && "self-center")}>
        {step === "welcome" && <Welcome />}
        {step === "agentsDetected" && (
          <AgentsDetected
            scanStatus={scanStatus}
            disabledAgents={disabledAgents}
            onAgentEnabledChange={onAgentEnabledChange}
          />
        )}
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
            windowDays={activityWindowDays}
            agentNames={(scanStatus?.agents ?? [])
              .filter((entry) => entry.sessionsSeen > 0)
              .sort((a, b) => b.sessionsSeen - a.sessionsSeen)
              .map((entry) => agentDisplayName(entry.agent))}
            hygieneSummary={hygieneSummary}
            onLaunchAtLoginChange={onLaunchAtLoginChange}
            nudgesRespectDnd={nudgesRespectDnd}
            onNudgesRespectDndChange={onNudgesRespectDndChange}
          />
        )}
      </div>

      <footer className="grid grid-cols-[1fr_max-content_1fr] items-center gap-2 border-t border-separator px-4 py-3">
        <div>{index > 0 && <PushButton onClick={goBack}>Back</PushButton>}</div>
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
