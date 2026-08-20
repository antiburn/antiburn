// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Check, FolderPlus, Lock, X } from "lucide-react"

import appIcon from "../../assets/app-icon.png"
import { useState } from "react"

import { isMacOS } from "../../lib/platform"
import { cn } from "../../lib/cn"

import { FolderPermissionNotice } from "../../components/repositories/FolderPermissionNotice"
import { LocalRepositoryList } from "../../components/repositories/LocalRepositoryList"
import { Card } from "../../components/ui/Card"
import { PushButton } from "../../components/ui/PushButton"
import { ScrollPane } from "../../components/ui/ScrollPane"
import { SegmentedControl } from "../../components/ui/SegmentedControl"
import { ToggleRow } from "../../components/ui/ToggleRow"
import { getConsentDiagnostics, openFolderAccessSettings, type ScanStatus } from "../../lib/ipc"
import type { FolderPermissions, LocalRepositoryItem } from "../../lib/types/repository";
import type { FolderPermissionFlow } from "../../lib/useFolderPermissionFlow"

/**
 * First run, in five screens: Welcome, Sources, Repositories, Historical scan,
 * Ready.
 *
 * The rhythm is the app-shell contract's — establish trust, choose where to
 * look, choose what to include, do the work with progress and a way out, then
 * confirm a usable result. What each step *does* is local: there is no account
 * to create and nothing is uploaded, so the steps carry local jobs and local
 * copy.
 *
 * ## The window
 *
 * This runs in its own 680×480 window (`src-tauri/src/onboarding.rs`), not in
 * the popover it used to be a surface of — recorded as D-25 in
 * `docs/deviations.md`. Two consequences show up in the markup below. The
 * centred screens cap their copy column rather than running to 680pt, and the
 * header carries a `data-tauri-drag-region` strip on macOS, where the window's
 * title bar is an overlay and the webview covers its area.
 *
 * ## Deliberate copy deviation
 *
 * The ratified onboarding row specifies Welcome copy that mentions "anonymous
 * analytics is sent only after a separate, default-off opt-in". This build
 * ships **no analytics of any kind** — no client, no consent surface, no
 * endpoint — so promising a control that does not exist would be the exact
 * failure the honesty rule is there to prevent. The Welcome step says what is
 * true instead, and the divergence is recorded rather than hidden. If analytics
 * are ever built, the copy and the matrix row can be reconciled then.
 */

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
  /** Stop the pass in flight. */
  onCancelScan: () => void
  /** The shell's scan status, or null before the first read. */
  scanStatus: ScanStatus | null
  /** How many days of history the popover will list. */
  windowDays: number
  onWindowDaysChange: (days: number) => void
  /** Whether the installed app should start after the reader signs in. */
  launchAtLogin: boolean
  onLaunchAtLoginChange: (enabled: boolean) => void
  /** Finish: records the flag and enters the activity view. */
  onFinish: () => void
  finishing: boolean
  finishError: string | null
}

const STEPS = ["welcome", "sources", "repositories", "scan", "ready"] as const
type Step = (typeof STEPS)[number]

/** Steps that want a discovery pass to have run by the time they are read. */
const STEPS_NEEDING_DISCOVERY: readonly Step[] = ["repositories", "scan"]

/** The two recent-activity windows the popover offers. They affect the list,
 *  not how long indexed sessions remain stored. */
const WINDOW_OPTIONS = [
  { value: "7", label: "7 days" },
  { value: "14", label: "14 days" },
] as const

/** A newly mounted step announces itself without a lifecycle effect. */
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
            "h-1.5 w-1.5 rounded-full transition-colors",
            position === index ? "bg-label-secondary" : "bg-label/20",
          )}
        />
      ))}
    </div>
  )
}

/**
 * The copy column on the two centred screens.
 *
 * The window is 680pt wide because the *list* steps needed it; balanced prose
 * at that measure reads as a banner rather than a paragraph, so the centred
 * screens keep roughly the column they were written for.
 */
const CENTRED_COLUMN = "mx-auto flex max-w-[440px] flex-col items-center"

function Welcome() {
  return (
    <div className="flex flex-1 flex-col items-center justify-center px-8 text-center">
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
          Everything stays on this machine
        </h2>
        <p className="mt-2 text-balance type-callout text-label-secondary">
          antiburn reads the coding-agent sessions already on your disk, analyzes them here, and
          shows you what they cost and how they went. No account, nothing uploaded, and no usage
          data collected.
        </p>
        {/* This sentence has to keep matching what "local" actually promises:
            antiburn goes online as your own agent — reading a provider's
            current usage figures with your credentials, checking GitHub
            Releases for new versions — and what makes it local is that none
            of that depends on any service of ours, and nothing about you
            goes to one. */}
        <p className="mt-2 text-balance type-footnote text-label-tertiary">
          antiburn goes online as your own agent — to read a provider&rsquo;s current usage
          figures with your own credentials, and to check GitHub Releases for new versions. None
          of it needs an antiburn account or an antiburn server — there isn&rsquo;t one — and
          nothing about you or your sessions goes anywhere else.
        </p>
      </div>
    </div>
  )
}

function Sources({
  defaultRoots,
  blockedRoots,
  scanRoots,
  onRemoveScanRoot,
}: Pick<OnboardingFlowProps, "defaultRoots" | "scanRoots" | "onRemoveScanRoot"> & {
  blockedRoots: readonly string[]
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col px-8 pt-2">
      <h2 ref={focusHeading} tabIndex={-1} className="type-title-3 text-label outline-none">
        Where to look
      </h2>
      <p className="mt-1 type-footnote text-label-secondary">
        Agent session stores are found automatically. Add a folder only if you keep repositories
        somewhere unusual.
      </p>

      <ScrollPane className="mt-3" viewportClassName="pr-1">
        {defaultRoots.length > 0 && (
          <>
            <p className="pb-1 type-caption font-medium tracking-wide uppercase text-label-tertiary">
              {blockedRoots.length > 0 ? "Default folders" : "Searched already"}
            </p>
            <ul className="space-y-0.5 pb-3">
              {defaultRoots.map((root) => {
                // A default root inside a folder macOS is still guarding has
                // *not* been searched. Ticking it here would be the one lie
                // this step could tell.
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

        <p className="pb-1 type-caption font-medium tracking-wide uppercase text-label-tertiary">
          Added by you
        </p>
        {scanRoots.length === 0 ? (
          <p className="pb-2 type-footnote text-label-tertiary">Nothing extra yet.</p>
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
                  className="shrink-0 rounded p-0.5 text-label-tertiary hover:bg-surface-hover hover:text-label-secondary"
                >
                  <X size={11} strokeWidth={2.5} aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
        )}
      </ScrollPane>
    </div>
  )
}

/**
 * "Add a folder…", which lives in the flow's footer rather than in the Sources
 * step it belongs to.
 *
 * It is the one step-level action in the flow, and putting it inline left it
 * stranded under a scroll region with the real controls a row below. In the
 * footer it sits where the reader's hand already is, next to Continue, and the
 * step's body is nothing but the list it is about.
 */
function AddFolderButton({ onAddScanRoot }: Pick<OnboardingFlowProps, "onAddScanRoot">) {
  return (
    <PushButton className="gap-1.5" onClick={onAddScanRoot}>
      <FolderPlus size={12} aria-hidden="true" />
      Add a folder…
    </PushButton>
  )
}

/**
 * Which repositories antiburn watches.
 *
 * Inclusion is opt-out, so this step is genuinely skippable — the list is here
 * because the reader may have a client's repository on this machine they would
 * rather antiburn never indexed, and the first run is the moment to say so.
 */
function Repositories({
  repositories,
  onToggleRepository,
  scanning,
  permissions,
  permissionFlow,
  onRecheckPermissions,
  recheckingPermissions,
}: {
  repositories: readonly LocalRepositoryItem[]
  onToggleRepository: (item: LocalRepositoryItem, enabled: boolean) => void
  scanning: boolean
  permissions: FolderPermissions
  permissionFlow: FolderPermissionFlow
  onRecheckPermissions: () => void
  recheckingPermissions: boolean
}) {
  return (
    <div className="flex min-h-0 flex-1 flex-col px-8 pt-2">
      <h2 ref={focusHeading} tabIndex={-1} className="type-title-3 text-label outline-none">
        What to include
      </h2>
      <p className="mt-1 type-footnote text-label-secondary">
        Everything found here is watched unless you turn it off. Turning one off also stops its
        sessions being indexed.
      </p>
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
      <div className="mt-2 min-h-0 flex-1">
        <LocalRepositoryList
          repositories={[...repositories]}
          loading={scanning}
          onToggleRepository={onToggleRepository}
          emptyTitle="Nothing found yet"
          emptyDescription="Repositories appear once a coding session has run in one. You can change this later in Settings."
        />
      </div>
    </div>
  )
}

/**
 * The historical scan: choose how much history to keep on screen, watch the
 * pass run, and stop it if it is taking too long.
 *
 * The window choice is deliberately described as what it *is* — how far back
 * the list reaches — rather than as a retention policy. Indexed sessions remain
 * stored until the reader explicitly clears them.
 */
function HistoricalScan({
  scanStatus,
  windowDays,
  onWindowDaysChange,
  onDiscover,
  onCancelScan,
}: Pick<
  OnboardingFlowProps,
  "scanStatus" | "windowDays" | "onWindowDaysChange" | "onDiscover" | "onCancelScan"
>) {
  const running = scanStatus?.running ?? false
  const found = scanStatus?.sessions ?? 0

  return (
    <div className="flex min-h-0 flex-1 flex-col px-8 pt-2">
      <h2 ref={focusHeading} tabIndex={-1} className="type-title-3 text-label outline-none">
        Historical scan
      </h2>
      <p className="mt-1 type-footnote text-label-secondary">
        Choose how much recent activity the popover lists. Indexed sessions stay on this machine
        until you clear them; you can change this view later.
      </p>

      {/* Capped rather than `w-full`: two options stretched across 680pt read
          as a pair of buttons that happen to touch, not as one control. */}
      <SegmentedControl
        className="mt-3 w-full max-w-[280px]"
        options={WINDOW_OPTIONS}
        value={String(windowDays) === "14" ? "14" : "7"}
        onChange={(days) => onWindowDaysChange(Number(days))}
        ariaLabel="How much history to list"
        equalWidth
      />

      <div className="mt-4 flex-1" aria-live="polite">
        {running ? (
          <>
            <p className="type-callout text-label">
              Scanning… {scanStatus?.completedAgents ?? 0} of {scanStatus?.totalAgents ?? 0}{" "}
              agents
            </p>
            <p className="mt-1 type-footnote text-label-secondary">
              {found} {found === 1 ? "session" : "sessions"} so far. Session files are read,
              never written.
            </p>
          </>
        ) : scanStatus?.error ? (
          <>
            <p className="type-callout text-system-orange">The scan did not finish.</p>
            <p className="mt-1 type-footnote text-label-secondary">{scanStatus.error}</p>
          </>
        ) : scanStatus?.cancelled ? (
          <>
            <p className="type-callout text-label">Stopped.</p>
            <p className="mt-1 type-footnote text-label-secondary">
              {found} {found === 1 ? "session" : "sessions"} were indexed before it stopped. You
              can continue and scan again later.
            </p>
          </>
        ) : scanStatus?.finishedAt ? (
          <>
            <p className="type-callout text-label">
              Found {found} {found === 1 ? "session" : "sessions"}.
            </p>
            <p className="mt-1 type-footnote text-label-secondary">
              antiburn keeps looking in the background while you use it.
            </p>
          </>
        ) : (
          <p className="type-footnote text-label-secondary">Ready to scan.</p>
        )}
      </div>

      <div className="pb-1">
        {running ? (
          <PushButton onClick={onCancelScan}>Stop</PushButton>
        ) : scanStatus?.finishedAt ? null : (
          <PushButton onClick={onDiscover}>Scan now</PushButton>
        )}
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
        <h2 ref={focusHeading} tabIndex={-1} className="type-title-3 text-label outline-none">
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
        <Card className="mt-5 w-full text-left">
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

/** The first-run flow. See the module comment for its rhythm and its one
 *  deliberate deviation from the ratified copy. */
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
  onCancelScan,
  scanStatus,
  windowDays,
  onWindowDaysChange,
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
  const discovered = scanStatus?.finishedAt != null
  const running = scanStatus?.running ?? false

  const advance = () => {
    const next = STEPS[index + 1] ?? "ready"
    if (
      STEPS_NEEDING_DISCOVERY.includes(next) &&
      !discovered &&
      !running &&
      !discoveryRequested
    ) {
      setDiscoveryRequested(true)
      onDiscover()
    }
    setStep(next)
  }

  return (
    <div className="relative flex h-full flex-col" aria-label="Set up antiburn" role="region">
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
      <header className="flex h-11 shrink-0 items-center px-4">
        <h1 className={cn("type-headline text-label", isMacOS() && "sr-only")}>antiburn</h1>
        {/* The step is announced as text rather than left to the dots, which
            are decorative and hidden. */}
        <p className="sr-only" aria-live="polite">
          Step {index + 1} of {STEPS.length}
        </p>
      </header>

      <div className="flex min-h-0 flex-1 flex-col">
        {step === "welcome" && <Welcome />}
        {step === "sources" && (
          <Sources
            defaultRoots={defaultRoots}
            blockedRoots={blockedRoots}
            scanRoots={scanRoots}
            onRemoveScanRoot={onRemoveScanRoot}
          />
        )}
        {step === "repositories" && (
          <Repositories
            repositories={repositories}
            onToggleRepository={onToggleRepository}
            scanning={running && repositories.length === 0}
            permissions={permissions}
            permissionFlow={permissionFlow}
            onRecheckPermissions={onRecheckPermissions}
            recheckingPermissions={recheckingPermissions}
          />
        )}
        {step === "scan" && (
          <HistoricalScan
            scanStatus={scanStatus}
            windowDays={windowDays}
            onWindowDaysChange={onWindowDaysChange}
            onDiscover={onDiscover}
            onCancelScan={onCancelScan}
          />
        )}
        {step === "ready" && (
          <Ready
            sessions={scanStatus?.sessions ?? 0}
            launchAtLogin={launchAtLogin}
            onLaunchAtLoginChange={onLaunchAtLoginChange}
            finishError={finishError}
          />
        )}
      </div>

      <footer className="flex shrink-0 items-center gap-2 border-t border-separator px-4 py-3">
        <div className="flex-1">
          {index > 0 && (
            <PushButton onClick={() => setStep(STEPS[index - 1] ?? "welcome")}>Back</PushButton>
          )}
        </div>
        <StepDots step={step} />
        <div className="flex flex-1 items-center justify-end gap-2">
          {/* Left of Continue, so the ordinary way forward stays the rightmost
              button on every step. */}
          {step === "sources" && <AddFolderButton onAddScanRoot={onAddScanRoot} />}
          <PushButton
            variant="primary"
            onClick={() => (last ? onFinish() : advance())}
            disabled={last && finishing}
          >
            {last ? (finishing ? "Finishing…" : "Start using antiburn") : "Continue"}
          </PushButton>
        </div>
      </footer>
    </div>
  )
}
