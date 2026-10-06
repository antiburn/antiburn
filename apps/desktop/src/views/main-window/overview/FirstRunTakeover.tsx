import { useState, useSyncExternalStore } from "react"

import { cn } from "../../../lib/cn"
import type { BurnCheckDetectorId } from "../../../lib/insightsIpc"
import { isMacOS } from "../../../lib/platform"

import { PRIMARY_BUTTON, ProgressStepCard, SKIP_BUTTON } from "./ProgressSteps"
import {
  enhanceFixes,
  firstFailingCheck,
  fixesFound,
  LIVE_LIMITS_TRANSITION_NAME,
  nextStep,
  type OverviewProgress,
  overviewProgress,
  type ProgressStepKey,
  progressStepTransitionName,
  showLiveLimits,
  skipLiveLimits,
  subscribeOverviewProgress,
} from "./overviewProgressStore"
import { StepSettingsDisclosure } from "./stepSettings/StepSettingsDisclosure"

function WelcomeCard() {
  return (
    <div className="flex w-full max-w-lg flex-col gap-(--space-sm) text-center">
      <h2 className="type-title-2 text-label">Welcome</h2>

      <p className="type-body text-label-secondary">
        antiburn reads and analyses your session logs,
        <br />
        keeping you on top of your usage and how you can save.
      </p>
      <p className="type-body text-label-secondary">100% local, no account needed.</p>

      <div className="mt-(--space-lg)">
        <NextButton disabled={false} label="Get Started" onClick={() => void nextStep()} />
      </div>
    </div>
  )
}

function NextButton({
  className = "",
  disabled,
  label = "Next",
  onClick,
}: {
  className?: string
  disabled: boolean
  label?: string
  onClick: () => void
}) {
  return (
    <div className={cn("flex justify-center", className)}>
      <button type="button" disabled={disabled} onClick={onClick} className={PRIMARY_BUTTON}>
        {label}
      </button>
    </div>
  )
}

function LiveLimitsCard() {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleShow() {
    setBusy(true)
    setError(null)
    try {
      await showLiveLimits()
    } catch {
      setError("Could not start live limits. Try again.")
      setBusy(false)
    }
  }

  return (
    <div className="flex w-full max-w-lg flex-col items-center">
      <div
        style={{ viewTransitionName: LIVE_LIMITS_TRANSITION_NAME }}
        className="flex w-full flex-col items-center gap-(--space-lg)"
      >
        <div className="flex flex-col gap-(--space-sm) text-center">
          <h2 className="type-title-2 text-label">Show your plan limits</h2>
          <p className="type-body text-label-secondary">
            antiburn shows you your plan limit usage, and how it tracks against your reset
            times.
          </p>
          {/* Only macOS keeps these credentials in the Keychain. */}
          {isMacOS() && (
            <p className="type-footnote text-label-tertiary">
              macOS may ask for Keychain access, so antiburn can read the credentials your
              coding tools already use.
            </p>
          )}
        </div>

        <div className="mt-(--space-lg) flex flex-col items-center gap-(--space-sm)">
          <button
            type="button"
            disabled={busy}
            onClick={() => void handleShow()}
            className={PRIMARY_BUTTON}
          >
            {busy ? "Starting…" : "Turn on live limits"}
          </button>

          <button
            type="button"
            disabled={busy}
            onClick={skipLiveLimits}
            className={SKIP_BUTTON}
          >
            Skip and turn on in settings later
          </button>
        </div>

        {error && (
          <p role="alert" className="type-footnote text-system-red-text">
            {error}
          </p>
        )}

        <StepSettingsDisclosure step="limits" />
      </div>
    </div>
  )
}

function stepDone(step: ProgressStepKey, progress: OverviewProgress): boolean {
  switch (step) {
    case "agents":
      return progress.agents.done
    case "sessions":
      return progress.sessions.done
    case "checks":
      return progress.checks.done
    case "fixes":
      return true
  }
}

function TakeoverStep({
  step,
  progress,
  onOpenChecks,
}: {
  step: ProgressStepKey
  progress: OverviewProgress
  onOpenChecks: (check: BurnCheckDetectorId | undefined) => void
}) {
  // Enhance finishes the first run the same way Done (Skip) does, then
  // opens the first check that needs a fix.
  async function enhance() {
    const check = firstFailingCheck(progress)
    await enhanceFixes()
    onOpenChecks(check)
  }
  return (
    <>
      <ProgressStepCard
        step={step}
        surface="firstRun"
        progress={progress}
        isSteady={false}
        transitionName={progressStepTransitionName(step)}
      />

      {step !== "fixes" && <StepSettingsDisclosure step={step} />}

      {step === "fixes" && fixesFound(progress) ? (
        <div className="mt-(--space-lg) flex flex-col items-center gap-(--space-sm)">
          <button type="button" onClick={() => void enhance()} className={PRIMARY_BUTTON}>
            Enhance
          </button>
          <button type="button" onClick={() => void nextStep()} className={SKIP_BUTTON}>
            Skip for now
          </button>
        </div>
      ) : (
        <NextButton
          className="mt-(--space-lg)"
          disabled={!stepDone(step, progress)}
          label={step === "fixes" ? "Done" : "Next"}
          onClick={() => void nextStep()}
        />
      )}
    </>
  )
}

export function FirstRunTakeover({
  onOpenChecks,
}: {
  onOpenChecks: (check: BurnCheckDetectorId | undefined) => void
}) {
  const progress = useSyncExternalStore(
    subscribeOverviewProgress,
    overviewProgress,
    overviewProgress,
  )

  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-(--space-lg) p-(--space-2xl)">
      {!progress.stepShown ? null : progress.flow === "welcome" ? (
        <WelcomeCard />
      ) : progress.flow === "limits" ? (
        <LiveLimitsCard />
      ) : progress.flow === "agents" ||
        progress.flow === "sessions" ||
        progress.flow === "checks" ||
        progress.flow === "fixes" ? (
        <TakeoverStep step={progress.flow} progress={progress} onOpenChecks={onOpenChecks} />
      ) : null}
    </div>
  )
}
