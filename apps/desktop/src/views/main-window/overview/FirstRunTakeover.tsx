import { useState } from "react"

import { cn } from "../../../lib/cn"
import { isMacOS } from "../../../lib/platform"

import {
  LIVE_LIMITS_TRANSITION_NAME,
  nextStep,
  previousStep,
  type OverviewProgress,
  type ProgressStepKey,
  progressStepTransitionName,
  showLiveLimits,
  skipLiveLimits,
  stepDone,
} from "./overviewProgressStore"
import { useOverviewProgress } from "./useOverviewProgress"
import { noteInteraction, scanNow } from "../../../lib/ipc"
import { CountUp } from "../../../components/ui/CountUp"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { capitalize, countWord, pluralize } from "../../../lib/presentation/text"
import {
  useFolderPermissionFlow,
  type FolderPermissionFlow,
} from "../../../lib/useFolderPermissionFlow"
import { StepSettings, type StepSettingsStep } from "./stepSettings/StepSettings"

function useReadPermissionFlow(progress: OverviewProgress): FolderPermissionFlow {
  return useFolderPermissionFlow(progress.sessions.deferred, () => {
    noteInteraction({ kind: "firstRunAction", action: "folder_access_granted" })
    void scanNow()
  })
}

function StepProgressBar({
  completed,
  title,
  started,
  total,
  emptyLabel,
}: {
  completed: number
  title: string
  started: boolean
  total: number
  emptyLabel: string
}) {
  const value = started ? (total === 0 ? 1 : completed / total) : 0

  return (
    <div className="flex flex-col items-center gap-(--space-xs)">
      <div
        role="progressbar"
        aria-label={title}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(value * 100)}
        className="h-2 w-full overflow-hidden rounded-full bg-surface-tertiary opacity-70"
      >
        <div
          className="h-full rounded-full bg-brand-tint transition-[width] duration-medium ease-out"
          style={{ width: `${value * 100}%` }}
        />
      </div>

      <span className="type-callout flex items-baseline tabular-nums text-label-secondary">
        {!started ? (
          "Waiting"
        ) : total === 0 ? (
          emptyLabel
        ) : (
          <>
            <CountUp value={completed} />/<CountUp value={total} />
          </>
        )}
      </span>
    </div>
  )
}

function AgentsStepRow({ snapshot }: { snapshot: OverviewProgress }) {
  const { rows } = snapshot.agents

  return (
    <ul className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 font-mono type-metadata tabular-nums text-label-secondary">
      {rows.map((row) => {
        const found = row.done && row.sessions > 0
        return (
          <li
            key={row.agent}
            aria-label={
              !row.done
                ? `${row.label}: searching`
                : `${row.label}: ${row.sessions.toLocaleString()} ${pluralize(row.sessions, "session", "sessions")}`
            }
            className={cn(
              "flex items-center gap-1.5",
              !row.done && "animate-pulse",
              row.done && !found && "opacity-30 grayscale",
            )}
          >
            {renderAgentIcon(
              row.agent,
              18,
              undefined,
              found || !row.done ? "default" : "neutral",
            )}
            {found && <span aria-hidden="true">{row.sessions.toLocaleString()}</span>}
          </li>
        )
      })}
    </ul>
  )
}

function ReadFolderPermissionNotice({
  deferredCount,
  permissionFlow,
}: {
  deferredCount: number
  permissionFlow: FolderPermissionFlow
}) {
  const asking = permissionFlow.phase === "asking" || permissionFlow.phase === "settling"
  return (
    <p className="type-footnote text-label-tertiary">
      {deferredCount.toLocaleString()}{" "}
      {pluralize(deferredCount, "folder needs", "folders need")} your permission before antiburn
      can read {pluralize(deferredCount, "it", "them")}.{" "}
      <button
        type="button"
        onClick={() => {
          noteInteraction({ kind: "firstRunAction", action: "folder_access_requested" })
          permissionFlow.start()
        }}
        disabled={asking}
        className="underline underline-offset-[3px] hover:text-label-secondary disabled:opacity-50"
      >
        {asking ? "Asking…" : "Allow access"}
      </button>
    </p>
  )
}

function SessionsStepRow({
  snapshot,
  permissionFlow,
}: {
  snapshot: OverviewProgress
  permissionFlow: FolderPermissionFlow
}) {
  const { done, displayCompleted, displayTotal } = snapshot.sessions
  const started = done || displayTotal > 0

  return (
    <div className="flex flex-col gap-2">
      <StepProgressBar
        completed={displayCompleted}
        started={started}
        title="Read session data"
        total={displayTotal}
        emptyLabel="No sessions found"
      />

      {snapshot.sessions.deferred.length > 0 && (
        <ReadFolderPermissionNotice
          deferredCount={snapshot.sessions.deferred.length}
          permissionFlow={permissionFlow}
        />
      )}
    </div>
  )
}

function ChecksStepRow({ snapshot }: { snapshot: OverviewProgress }) {
  const { windowSessions, pendingEvidence, deferredEvidence } = snapshot.checks
  const started = snapshot.sessions.done
  const completed = Math.max(0, windowSessions - (pendingEvidence - deferredEvidence))

  return (
    <StepProgressBar
      completed={completed}
      started={started}
      title="Run session checks"
      total={windowSessions}
      emptyLabel="No sessions to check"
    />
  )
}

const PRIMARY_BUTTON =
  "rounded-control bg-brand-tint px-10 py-3 type-title-3 font-semibold! text-white shadow-[var(--shadow-raised)] transition-[filter] duration-fast hover:brightness-110 active:brightness-95 disabled:opacity-50"

function stepCopy(
  step: ProgressStepKey,
  progress: OverviewProgress,
): { title: string; body: string } {
  switch (step) {
    case "agents": {
      if (!progress.agents.done) {
        return {
          title: "Finding agents…",
          body: "Scanning last 30 days of session logs to find your coding agents.",
        }
      }
      const foundCount = progress.agents.rows.filter(
        (row) => row.done && row.sessions > 0,
      ).length
      return {
        title:
          foundCount === 0
            ? "No agents found"
            : `${capitalize(countWord(foundCount))} ${pluralize(foundCount, "agent", "agents")} found`,
        body: "Scanned the last 30 days of session logs to find your coding agents.",
      }
    }
    case "sessions":
      return {
        title: progress.sessions.done
          ? "Finished reading recent sessions"
          : "Reading recent sessions…",
        body: "antiburn pulls each session's metadata - every line of the log - into a local unencrypted sqlite db, for indexed access.",
      }
    case "checks":
      return {
        title: progress.checks.done
          ? "Finished running session checks"
          : "Running session checks…",
        body: "Checking for anti-patterns: problems with context window, caching, unused tools and skills, and more.",
      }
    case "fixes":
      return {
        title: "You're all set!",
        body: "Setup is complete. Time to get started with antiburn.",
      }
  }
}

function NextButton({ disabled, label }: { disabled: boolean; label: string }) {
  return (
    <div className="flex justify-center">
      <button
        type="button"
        disabled={disabled}
        onClick={() => void nextStep()}
        className={PRIMARY_BUTTON}
      >
        {label}
      </button>
    </div>
  )
}

function OpenSettingsToggle({
  step,
  open,
  onToggle,
}: {
  step: Exclude<StepSettingsStep, "fixes">
  open: boolean
  onToggle: () => void
}) {
  return (
    <button
      type="button"
      aria-expanded={open}
      onClick={() => {
        if (!open) {
          noteInteraction({ kind: "stepSettingsViewed", label: step, detail: "first_run" })
        }
        onToggle()
      }}
      className="text-accent underline decoration-accent/40 underline-offset-[3px] hover:decoration-accent"
    >
      {open ? "Hide settings" : "Expand settings"}
    </button>
  )
}

function StepSettingsPanel({ step }: { step: StepSettingsStep }) {
  return (
    <div className="flex w-full flex-col gap-(--space-md)">
      <StepSettings step={step} />
    </div>
  )
}

function WelcomeCard() {
  return (
    <div className="flex w-full flex-col gap-(--space-lg) text-center">
      <h2 className="type-large-title text-label">Welcome to antiburn</h2>

      <ul className="mx-auto flex list-disc flex-col gap-(--space-sm) ps-6 text-start type-title-3 font-normal! text-label-secondary">
        <li>Reads and analyses your session logs</li>
        <li>Helps you avoid hitting limits</li>
        <li>100% local, no account needed</li>
      </ul>

      <div className="mt-(--space-lg)">
        <NextButton disabled={false} label="Get Started" />
      </div>
    </div>
  )
}

function LiveLimitsCard() {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [infoOpen, setInfoOpen] = useState(false)

  async function handleShow() {
    setBusy(true)
    setError(null)
    try {
      await showLiveLimits()
    } catch {
      setError("Could not start live limits. Try again.")
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex w-full flex-col items-center">
      <div
        style={{ viewTransitionName: LIVE_LIMITS_TRANSITION_NAME }}
        className="flex w-full flex-col items-center gap-(--space-lg)"
      >
        <div className="flex flex-col gap-(--space-sm) text-center">
          <h2 className="type-title-1 text-label">Plan limits</h2>

          <p className="type-title-3 font-normal! text-label-secondary">
            See plan limit usage against your reset times.{" "}
          </p>

          {isMacOS() && !infoOpen && (
            <p className="mx-auto max-w-lg text-balance type-callout text-label-tertiary">
              macOS may ask for Keychain access, allowing antiburn to read existing coding tool
              credentials.
            </p>
          )}
        </div>

        <div className="mt-(--space-lg) grid grid-cols-[1fr_1fr] items-center gap-x-(--space-sm)">
          <button
            type="button"
            disabled={busy}
            onClick={previousStep}
            className="ui-push-button h-full type-title-3 font-normal!"
          >
            Back
          </button>

          <button
            type="button"
            disabled={busy}
            onClick={() => void handleShow()}
            className={PRIMARY_BUTTON}
          >
            {busy ? "Starting…" : "Turn on limits"}
          </button>
        </div>

        <button
          type="button"
          disabled={busy}
          onClick={skipLiveLimits}
          className="type-callout text-label-secondary underline underline-offset-[3px] hover:text-label"
        >
          Skip until later
        </button>

        {error && (
          <p role="alert" className="type-footnote text-system-red-text">
            {error}
          </p>
        )}

        <OpenSettingsToggle
          step="limits"
          open={infoOpen}
          onToggle={() => setInfoOpen(!infoOpen)}
        />

        {infoOpen && <StepSettingsPanel step="limits" />}
      </div>
    </div>
  )
}

function TakeoverStepCard({
  step,
  progress,
}: {
  step: ProgressStepKey
  progress: OverviewProgress
}) {
  const [infoStep, setInfoStep] = useState<ProgressStepKey | null>(null)
  const infoOpen = infoStep === step
  const permissionFlow = useReadPermissionFlow(progress)
  const { title, body } = stepCopy(step, progress)
  const stepContent = {
    agents: <AgentsStepRow snapshot={progress} />,
    sessions: <SessionsStepRow snapshot={progress} permissionFlow={permissionFlow} />,
    checks: <ChecksStepRow snapshot={progress} />,
    fixes: null,
  }[step]
  const content = stepContent && (
    <div className="mx-auto flex w-full max-w-lg flex-col gap-(--space-md)">{stepContent}</div>
  )

  return (
    <>
      <div
        style={{ viewTransitionName: progressStepTransitionName(step) }}
        className="flex w-full max-w-xl flex-col items-center gap-(--space-lg)"
      >
        {step === "agents" && content}

        <div className="flex flex-col gap-(--space-xs) text-center">
          <h2 className="type-title-1 text-label">{title}</h2>
          <p className="mx-auto max-w-lg text-balance type-title-3 font-normal! text-label-secondary">
            {body}
          </p>
        </div>

        {step !== "agents" && content}
      </div>

      <div className="mt-(--space-lg) grid grid-cols-[1fr_1fr] gap-x-(--space-sm)">
        <button
          type="button"
          onClick={previousStep}
          className="ui-push-button h-full type-title-3 font-normal!"
        >
          Back
        </button>
        <NextButton
          disabled={!stepDone(step, progress)}
          label={step === "fixes" ? "Get Started" : "Next"}
        />
      </div>

      {step !== "fixes" && (
        <>
          <OpenSettingsToggle
            step={step}
            open={infoOpen}
            onToggle={() => setInfoStep(infoOpen ? null : step)}
          />

          {infoOpen && <StepSettingsPanel step={step} />}
        </>
      )}
    </>
  )
}

export function FirstRunTakeover({ className }: { className?: string }) {
  const progress = useOverviewProgress()

  return (
    <fieldset
      disabled={progress.actionPending}
      className={cn(
        "flex flex-1 flex-col items-center justify-center p-(--space-2xl)",
        className,
      )}
    >
      <div className="flex w-full max-w-xl flex-col items-center gap-(--space-lg)">
        {!progress.stepShown ? null : progress.flow === "welcome" ? (
          <WelcomeCard />
        ) : progress.flow === "limits" ? (
          <LiveLimitsCard />
        ) : progress.flow === "agents" ||
          progress.flow === "sessions" ||
          progress.flow === "checks" ||
          progress.flow === "fixes" ? (
          <TakeoverStepCard step={progress.flow} progress={progress} />
        ) : null}

        {progress.actionError && (
          <p role="alert" className="type-footnote text-system-red-text">
            {progress.actionError}
          </p>
        )}
      </div>
    </fieldset>
  )
}
