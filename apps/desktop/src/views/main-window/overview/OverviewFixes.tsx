import { useSyncExternalStore, type ReactNode } from "react"
import { Circle, CircleAlert, CircleCheck } from "lucide-react"

import { renderAgentIcon } from "../../../lib/agentIcon"
import { cn } from "../../../lib/cn"
import { noteInteraction, scanNow } from "../../../lib/ipc"
import { CHECK_PROBLEM_PHRASES } from "../../../lib/presentation/checkDefinitions"
import {
  useFolderPermissionFlow,
  type FolderPermissionFlow,
} from "../../../lib/useFolderPermissionFlow"
import {
  enableNonRepoFolders,
  openFixes,
  openSteps,
  overviewProgress,
  shrinkFixes,
  shrinkSteps,
  subscribeOverviewProgress,
  type FixCategory,
  type FixStatus,
  type OverviewProgress,
} from "./overviewProgressStore"

// The read step's gate details (outside a repository, excluded, unreadable)
// are off while their copy is redesigned.
const SHOW_READ_GATE_DETAILS: boolean = false

function fmt(value: number): string {
  return value.toLocaleString()
}

function pluralize(count: number, singular: string, plural: string): string {
  return count === 1 ? singular : plural
}

function statusLabel(status: FixStatus): string {
  switch (status) {
    case "needsFix":
      return "needs fix"
    case "awaitingVerification":
      return "awaiting verification"
    case "passing":
      return "passing"
    case "notChecked":
      return "not checked"
  }
}

function fixesSubtitle(failing: FixCategory[]): string {
  const phrases = failing.map((category) => CHECK_PROBLEM_PHRASES[category.id])
  const shown = phrases.slice(0, 3)
  const remaining = phrases.length - shown.length
  const sentence = shown.map((phrase, index) => (index === 0 ? capitalize(phrase) : phrase))
  return remaining > 0 ? `${sentence.join(", ")}, +${remaining} more` : sentence.join(", ")
}

function capitalize(value: string): string {
  return value.length === 0 ? value : value[0]!.toUpperCase() + value.slice(1)
}

type StepKey = "find" | "read" | "check"

const STEPS: readonly StepKey[] = ["find", "read", "check"]

/** Shared by a step's middle row and its docked cell, so a view transition
 *  moves the one element between the two places. */
function stepTransitionName(step: StepKey): string {
  return `progress-step-${step}`
}

const FIXES_TRANSITION_NAME = "progress-fixes"

export function OverviewFixes() {
  const progress = useSyncExternalStore(
    subscribeOverviewProgress,
    overviewProgress,
    overviewProgress,
  )
  const { dock, mode } = progress
  const isFirstRun = mode === "firstRun"
  const isSteady = mode === "steady"

  const isEmpty = progress.check.done && progress.check.windowSessions === 0
  const isClean =
    progress.check.done && progress.check.windowSessions > 0 && progress.failingCount === 0
  const hasFixes = progress.check.done && progress.failingCount > 0

  // A granted folder's rescan already feeds back through the store's own
  // scan-status subscription, which refreshes `read.deferred` once that pass
  // finishes.
  const permissionFlow = useFolderPermissionFlow(progress.read.deferred, () => {
    noteInteraction({ kind: "firstRunAction", action: "folder_access_granted" })
    void scanNow()
  })

  const firstRunStepDocked = (index: number) => !dock.stepsOpen && index < dock.stepsDocked
  const resultReady = progress.resultReady
  const fixesInMiddle = resultReady && !dock.fixesDocked

  const middleSteps = isFirstRun
    ? STEPS.filter((_, index) => !firstRunStepDocked(index))
    : isSteady && dock.stepsOpen
      ? STEPS
      : []
  const showWelcome = isFirstRun && dock.stepsDocked === 0
  const showMiddle = middleSteps.length > 0 || fixesInMiddle
  const showRow = mode !== "pending"

  return (
    <section aria-label="Fixes" className="flex flex-1 flex-col gap-(--space-lg)">
      <div className="relative min-h-[220px] flex-1">
        <div
          className={cn(
            "absolute inset-0 flex flex-col overflow-hidden mask-b-from-75%",
            isFirstRun
              ? progress.stepsDone
                ? "transition-opacity [transition-delay:2000ms] [transition-duration:1500ms]"
                : "opacity-0"
              : "",
          )}
        >
          <h2 className="mb-(--space-sm) type-caption text-label-secondary">Config checks</h2>

          <ul className="flex flex-col gap-1">
            {progress.categories.map((category) => (
              <CheckRow key={category.id} category={category} />
            ))}
          </ul>
        </div>

        {showMiddle && (
          <div
            style={{ viewTransitionName: "progress-backdrop" }}
            className="absolute inset-0 flex flex-col items-center justify-center gap-(--space-lg) bg-surface-window/75 p-(--space-lg) text-center rounded-(--radius-popover) backdrop-blur-[1.5px]"
          >
            {middleSteps.length > 0 && (
              <div className="flex w-full max-w-[36rem] flex-col gap-(--space-2xl) text-start">
                {showWelcome && <FirstRunWelcome />}
                {middleSteps.map((step) => (
                  <div key={step} style={{ viewTransitionName: stepTransitionName(step) }}>
                    {step === "find" ? (
                      <FindStepRow snapshot={progress} />
                    ) : step === "read" ? (
                      <ReadStepRow snapshot={progress} permissionFlow={permissionFlow} />
                    ) : (
                      <CheckStepRow snapshot={progress} isSteady={isSteady} />
                    )}
                  </div>
                ))}
                {dock.stepsOpen && (
                  <button
                    type="button"
                    onClick={shrinkSteps}
                    className={cn(LINK_BUTTON, "self-center")}
                  >
                    Shrink
                  </button>
                )}
              </div>
            )}

            {fixesInMiddle && (
              <div
                style={{ viewTransitionName: FIXES_TRANSITION_NAME }}
                className="flex flex-col items-center gap-(--space-lg)"
              >
                <FixesHeadline
                  isEmpty={isEmpty}
                  isClean={isClean}
                  hasFixes={hasFixes}
                  progress={progress}
                  onShrink={shrinkFixes}
                />
              </div>
            )}
          </div>
        )}
      </div>

      {showRow && (
        <div className="grid grid-cols-4 gap-(--space-md)">
          {STEPS.map((step, index) => (
            <div key={step} className="flex">
              {isSteady
                ? !dock.stepsOpen && <DockedStep step={step} snapshot={progress} isSteady />
                : isFirstRun &&
                  firstRunStepDocked(index) && (
                    <DockedStep step={step} snapshot={progress} isSteady={false} />
                  )}
            </div>
          ))}

          <div className="flex">
            {resultReady && (
              <DockedFixes
                isEmpty={isEmpty}
                isClean={isClean}
                failingCount={progress.failingCount}
                headlineInMiddle={fixesInMiddle}
              />
            )}
          </div>
        </div>
      )}
    </section>
  )
}

const LINK_BUTTON =
  "type-footnote text-label-secondary underline underline-offset-[3px] hover:text-label"

function stepValue(started: boolean, total: number, completed?: number): string {
  if (!started) return "Waiting"
  return completed == null ? fmt(total) : `${fmt(completed)}/${fmt(total)}`
}

/**
 * One cell of the row above Recent sessions. With `onOpen`, a button covers
 * the whole cell and opens its content in the middle. The content shows
 * above that button and lets clicks through, except a button in `value`,
 * which keeps its own click.
 */
function DockedCell({
  label,
  onOpen,
  transitionName,
  title,
  value,
}: {
  label: string
  onOpen: (() => void) | undefined
  transitionName: string | undefined
  title: string
  value: ReactNode
}) {
  return (
    <div
      style={transitionName ? { viewTransitionName: transitionName } : undefined}
      className={cn(
        "relative flex w-full items-center rounded-(--radius-popover) bg-session-card px-4 py-2.5 text-start",
        onOpen && "transition-colors duration-fast hover:bg-surface-tertiary",
      )}
    >
      {onOpen && (
        <button
          type="button"
          onClick={onOpen}
          aria-label={label}
          className="absolute inset-0 cursor-pointer! rounded-(--radius-popover)"
        />
      )}

      <span className="pointer-events-none relative flex w-full items-baseline justify-between gap-2">
        <span className="truncate type-caption text-label-secondary">{title}</span>
        {value}
      </span>
    </div>
  )
}

function DockedStep({
  step,
  snapshot,
  isSteady,
}: {
  step: StepKey
  snapshot: OverviewProgress
  isSteady: boolean
}) {
  const { title, value } = dockedStepContent(step, snapshot, isSteady)
  return (
    <DockedCell
      label={`${title}: ${value}. Show the steps.`}
      onOpen={openSteps}
      transitionName={stepTransitionName(step)}
      title={title}
      value={
        <span className="font-mono type-caption tabular-nums text-label-secondary">
          {value}
        </span>
      }
    />
  )
}

function dockedStepContent(
  step: StepKey,
  snapshot: OverviewProgress,
  isSteady: boolean,
): { title: string; value: string } {
  switch (step) {
    case "find": {
      const total = snapshot.find.rows.reduce((sum, row) => sum + row.sessions, 0)
      return { title: "Find session files", value: stepValue(true, total) }
    }
    case "read": {
      const { completed, total } = snapshot.read
      return { title: "Read session data", value: stepValue(total > 0, total, completed) }
    }
    case "check": {
      const { windowSessions, pendingEvidence } = snapshot.check
      const completed = Math.max(0, windowSessions - pendingEvidence)
      // `read.done` is the first-run latch. Outside the first run, the check
      // step does not wait on it.
      const started = isSteady || snapshot.read.done
      return {
        title: "Run session checks",
        value: stepValue(started, windowSessions, completed),
      }
    }
  }
}

function DockedFixes({
  isEmpty,
  isClean,
  failingCount,
  headlineInMiddle,
}: {
  isEmpty: boolean
  isClean: boolean
  failingCount: number
  headlineInMiddle: boolean
}) {
  const summary = isEmpty
    ? "No recent sessions"
    : isClean
      ? "No fixes needed"
      : `${failingCount} ${pluralize(failingCount, "fix", "fixes")} found`
  return (
    <DockedCell
      label={`${summary}. Show the result.`}
      onOpen={headlineInMiddle ? undefined : openFixes}
      transitionName={headlineInMiddle ? undefined : FIXES_TRANSITION_NAME}
      title={summary}
      value={
        !isEmpty &&
        !isClean && (
          <button
            type="button"
            className="pointer-events-auto shrink-0 rounded-control bg-brand-tint px-2 py-0.5 type-caption font-semibold! text-white shadow-[var(--shadow-raised)] transition-[filter] duration-fast hover:brightness-110 active:brightness-95"
          >
            Enhance
          </button>
        )
      }
    />
  )
}

function CheckRow({ category }: { category: FixCategory }) {
  const needsFix = category.status === "needsFix"
  const phrase = needsFix ? CHECK_PROBLEM_PHRASES[category.id] : null
  return (
    <li
      className={cn(
        "flex items-center gap-3 rounded-(--radius-popover) px-3 py-1.5",
        needsFix ? "bg-brand-tint/12 ring-1 ring-brand-tint/40" : "bg-session-card",
      )}
    >
      {needsFix ? (
        <CircleAlert size={16} strokeWidth={2} className="shrink-0 text-brand" />
      ) : category.status === "passing" ? (
        <CircleCheck size={16} strokeWidth={2} className="shrink-0 text-label-tertiary" />
      ) : (
        <Circle size={16} strokeWidth={2} className="shrink-0 text-label-tertiary" />
      )}
      <span className="flex min-w-0 items-baseline gap-2">
        <span
          className={cn(
            "shrink-0 type-body font-medium!",
            needsFix ? "text-label" : "text-label-secondary",
          )}
        >
          {category.label}
        </span>
        {phrase && <span className="truncate type-footnote text-label-tertiary">{phrase}</span>}
      </span>

      <span
        className={cn(
          "ms-auto shrink-0 font-mono type-metadata",
          needsFix ? "text-brand" : "text-label-tertiary",
        )}
      >
        {statusLabel(category.status)}
      </span>
    </li>
  )
}

function StepHeading({
  completed,
  done,
  started,
  title,
  total,
}: {
  completed?: number
  done: boolean
  title: string
  started: boolean
  total: number
}) {
  return (
    <div
      className={cn(
        done ? "text-label-secondary" : "text-label",
        "type-title-3 flex items-baseline justify-between",
      )}
    >
      <span>{title}</span>
      <span>
        {!started
          ? "Waiting"
          : completed == null
            ? fmt(total)
            : `${fmt(completed)}/${fmt(total)}`}
      </span>
    </div>
  )
}

function StepProgressBar({
  completed,
  done,
  title,
  started,
  total,
}: {
  completed: number
  done: boolean
  title: string
  started: boolean
  total: number
}) {
  const value = started ? completed / total : 0

  return (
    <div
      role="progressbar"
      aria-label={title}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(value * 100)}
      className={cn(
        "h-2 overflow-hidden rounded-full bg-surface-tertiary",
        !started ? "opacity-40" : done && "opacity-70",
      )}
    >
      <div
        className="h-full rounded-full bg-brand-tint transition-[width] duration-medium ease-out"
        style={{ width: `${value * 100}%` }}
      />
    </div>
  )
}

/**
 * The first run's own pitch and privacy line. Shown only before the first
 * step docks, so the steps block gets the full middle area once it is
 * running.
 */
function FirstRunWelcome() {
  return (
    <div className="flex flex-col gap-(--space-xs) text-center">
      <p className="type-title-3 text-label">Stop hitting your token limits.</p>
      <p className="type-footnote text-label-secondary">
        antiburn reads your coding agent session logs and analyses them locally. No account
        needed. Session analysis stays local unless you enable the optional Ignored Instructions
        check with a TypeSafe API key in Settings.
      </p>
    </div>
  )
}

function FindStepRow({ snapshot }: { snapshot: OverviewProgress }) {
  const { done, rows } = snapshot.find
  const total = rows.reduce((sum, row) => sum + row.sessions, 0)

  return (
    <div className="flex flex-col gap-2">
      <StepHeading done={done} started={true} title="Find session files" total={total} />

      <ul className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 font-mono type-metadata tabular-nums text-label-secondary">
        {rows.map((row) => {
          const found = row.done && row.sessions > 0
          return (
            <li
              key={row.agent}
              aria-label={
                !row.done
                  ? `${row.label}: searching`
                  : `${row.label}: ${fmt(row.sessions)} ${pluralize(row.sessions, "session", "sessions")}`
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
              {found && <span aria-hidden="true">{fmt(row.sessions)}</span>}
            </li>
          )
        })}
      </ul>
    </div>
  )
}

/**
 * One line asking for the protected folders the last pass could not read,
 * with a button that starts {@link useFolderPermissionFlow}'s queue. Shown
 * in the Read step wherever that step shows: the first run and the opened
 * steps block outside it.
 */
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
      {fmt(deferredCount)} {pluralize(deferredCount, "folder needs", "folders need")} your
      permission before antiburn can read {pluralize(deferredCount, "it", "them")}.{" "}
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

function ReadStepRow({
  snapshot,
  permissionFlow,
}: {
  snapshot: OverviewProgress
  permissionFlow: FolderPermissionFlow
}) {
  const { done, completed, total, gate } = snapshot.read
  const started = total > 0

  const data = { completed, done, started, title: "Read session data", total }

  return (
    <div className="flex flex-col gap-2">
      <StepHeading {...data} />
      <StepProgressBar {...data} />

      {snapshot.read.deferred.length > 0 && (
        <ReadFolderPermissionNotice
          deferredCount={snapshot.read.deferred.length}
          permissionFlow={permissionFlow}
        />
      )}

      {SHOW_READ_GATE_DETAILS && done && gate && (
        <div className="flex flex-col gap-0.5 type-footnote text-label-tertiary">
          {gate.outsideRepository > 0 && (
            <p>
              {fmt(gate.outsideRepository)}{" "}
              {pluralize(gate.outsideRepository, "session was", "sessions were")} outside a git
              repository — they get no check results.{" "}
              <button
                type="button"
                onClick={() => void enableNonRepoFolders()}
                className="underline underline-offset-[3px] hover:text-label-secondary"
              >
                Include them
              </button>
            </p>
          )}
          {gate.excluded > 0 && (
            <p>
              {fmt(gate.excluded)} {pluralize(gate.excluded, "session was", "sessions were")} in
              folders you excluded.
            </p>
          )}
          {gate.unreadable > 0 && (
            <p>
              {fmt(gate.unreadable)} {pluralize(gate.unreadable, "session's", "sessions'")}{" "}
              folder
              {pluralize(gate.unreadable, "", "s")} {pluralize(gate.unreadable, "was", "were")}{" "}
              missing or unreadable.
            </p>
          )}
        </div>
      )}
    </div>
  )
}

function CheckStepRow({
  snapshot,
  isSteady,
}: {
  snapshot: OverviewProgress
  isSteady: boolean
}) {
  const { done, windowSessions, pendingEvidence } = snapshot.check
  const started = isSteady || snapshot.read.done
  const completed = Math.max(0, windowSessions - pendingEvidence)

  const data = { completed, done, started, title: "Run session checks", total: windowSessions }

  return (
    <div className="flex flex-col gap-2">
      <StepHeading {...data} />
      <StepProgressBar {...data} />
    </div>
  )
}

function FixesHeadline({
  isEmpty,
  isClean,
  hasFixes,
  progress,
  onShrink,
}: {
  isEmpty: boolean
  isClean: boolean
  hasFixes: boolean
  progress: OverviewProgress
  onShrink: () => void
}) {
  if (isEmpty || isClean) {
    return (
      <div className="mt-6 flex max-w-[36rem] flex-col items-center gap-(--space-xs)">
        <p className="type-title-1 font-semibold! text-label">
          {isEmpty ? "No sessions in the last 30 days" : "No fixes needed"}
        </p>
        {isClean && (
          <p className="type-body text-label-secondary">Your config already looks efficient.</p>
        )}
        <button type="button" onClick={onShrink} className={cn(LINK_BUTTON, "mt-(--space-sm)")}>
          Shrink
        </button>
      </div>
    )
  }

  if (!hasFixes) return null

  const failing = progress.categories.filter((category) => category.status === "needsFix")

  return (
    <>
      <div className="mt-6 flex max-w-[36rem] flex-col gap-(--space-xs)">
        <p className="type-title-1 font-semibold! text-label">
          {progress.failingCount} {pluralize(progress.failingCount, "fix", "fixes")} found in
          your config
        </p>
        <p className="type-body text-label-secondary">{fixesSubtitle(failing)}</p>
      </div>
      {progress.history && (
        <p className="type-footnote text-label-tertiary">
          Reading older history · {fmt(progress.history.completed)} of{" "}
          {fmt(progress.history.total)}
        </p>
      )}
      <div className="flex flex-col items-center gap-(--space-sm)">
        <button
          type="button"
          className="rounded-control bg-brand-tint px-8 py-2.5 type-headline font-semibold! text-white shadow-[var(--shadow-raised)] transition-[filter] duration-fast hover:brightness-110 active:brightness-95"
        >
          Enhance
        </button>
        <button type="button" onClick={onShrink} className={LINK_BUTTON}>
          Shrink
        </button>
      </div>
    </>
  )
}
