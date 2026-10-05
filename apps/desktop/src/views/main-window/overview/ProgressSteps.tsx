// The first-run steps' live content: shared by the takeover's centred card
// (`FirstRunTakeover.tsx`) and a docked row's modal (`ProgressNav.tsx`). Each
// component renders one step's data; the card chrome, title, body copy, and
// buttons belong to its caller.

import type { ReactNode } from "react"

import { cn } from "../../../lib/cn"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { noteInteraction, scanNow } from "../../../lib/ipc"
import { CHECK_PROBLEM_PHRASES } from "../../../lib/presentation/checkDefinitions"
import {
  useFolderPermissionFlow,
  type FolderPermissionFlow,
} from "../../../lib/useFolderPermissionFlow"
import {
  enableNonRepoFolders,
  fixesFound,
  type FixCategory,
  type OverviewProgress,
  type ProgressStepKey,
} from "./overviewProgressStore"

/**
 * The Sessions step's folder-permission queue, wired to the store's granted
 * callback. Shared by the takeover and the modal, so both react the same
 * way once a folder is granted: a rescan already feeds back through the
 * store's own scan-status subscription, which refreshes `read.deferred`
 * once that pass finishes.
 */
function useReadPermissionFlow(progress: OverviewProgress): FolderPermissionFlow {
  return useFolderPermissionFlow(progress.sessions.deferred, () => {
    noteInteraction({ kind: "firstRunAction", action: "folder_access_granted" })
    void scanNow()
  })
}

// The Sessions step's gate details (outside a repository, excluded, unreadable)
// are off while their copy is redesigned.
const SHOW_READ_GATE_DETAILS: boolean = false

function fmt(value: number): string {
  return value.toLocaleString()
}

function pluralize(count: number, singular: string, plural: string): string {
  return count === 1 ? singular : plural
}

function capitalize(value: string): string {
  return value.length === 0 ? value : value[0]!.toUpperCase() + value.slice(1)
}

function fixesSubtitle(failing: FixCategory[]): string {
  const phrases = failing.map((category) => CHECK_PROBLEM_PHRASES[category.id])
  const shown = phrases.slice(0, 3)
  const remaining = phrases.length - shown.length
  const sentence = shown.map((phrase, index) => (index === 0 ? capitalize(phrase) : phrase))
  return remaining > 0 ? `${sentence.join(", ")}, +${remaining} more` : sentence.join(", ")
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
    <div className="flex items-center gap-x-3">
      <div
        role="progressbar"
        aria-label={title}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(value * 100)}
        className="flex-1 h-2 overflow-hidden rounded-full bg-surface-tertiary opacity-70"
      >
        <div
          className="h-full rounded-full bg-brand-tint transition-[width] duration-medium ease-out"
          style={{ width: `${value * 100}%` }}
        />
      </div>

      <span className="type-body flex items-baseline justify-end">
        {!started
          ? "Waiting"
          : completed == null
            ? fmt(total)
            : `${fmt(completed)}/${fmt(total)}`}
      </span>
    </div>
  )
}

/** The Agents step's own content: a row of agent logos. No heading row — the
 *  card around it already carries the step's title. */
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
  )
}

/**
 * One line asking for the protected folders the last pass could not read,
 * with a button that starts {@link useFolderPermissionFlow}'s queue. Shown
 * wherever the Sessions step's content shows.
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

function SessionsStepRow({
  snapshot,
  permissionFlow,
}: {
  snapshot: OverviewProgress
  permissionFlow: FolderPermissionFlow
}) {
  const { done, completed, total, gate } = snapshot.sessions
  const started = total > 0

  const data = { completed, done, started, title: "Read session data", total }

  return (
    <div className="flex flex-col gap-2">
      <StepProgressBar {...data} />

      {snapshot.sessions.deferred.length > 0 && (
        <ReadFolderPermissionNotice
          deferredCount={snapshot.sessions.deferred.length}
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

function ChecksStepRow({
  snapshot,
  isSteady,
}: {
  snapshot: OverviewProgress
  isSteady: boolean
}) {
  const { done, windowSessions, pendingEvidence } = snapshot.checks
  const started = isSteady || snapshot.sessions.done
  const completed = Math.max(0, windowSessions - pendingEvidence)

  const data = { completed, done, started, title: "Run session checks", total: windowSessions }

  return (
    <div className="flex flex-col gap-2">
      <StepProgressBar {...data} />
    </div>
  )
}

/** The takeover's primary action, such as Next or Turn on live limits. */
export const PRIMARY_BUTTON =
  "rounded-control bg-brand-tint px-8 py-2.5 type-headline font-semibold! text-white shadow-[var(--shadow-raised)] transition-[filter] duration-fast hover:brightness-110 active:brightness-95 disabled:opacity-50"

/** The quiet way past a step's primary action, under it. */
export const SKIP_BUTTON =
  "type-footnote text-label-secondary underline underline-offset-[3px] hover:text-label"

/**
 * One step's card chrome: a title, one or two lines of body copy, and the
 * step's own live content below. Shared by the takeover (which adds its own
 * Next button after this) and a docked row's modal (which adds none), so
 * both show literally the same card.
 */
function StepCard({
  transitionName,
  title,
  body,
  finePrint,
  children,
}: {
  transitionName: string | undefined
  title: string
  body?: string
  finePrint?: string
  children?: ReactNode
}) {
  return (
    <div
      style={transitionName ? { viewTransitionName: transitionName } : undefined}
      className="flex w-full max-w-lg flex-col items-center gap-(--space-lg)"
    >
      <div className="flex flex-col gap-(--space-xs) text-center">
        <h2 className="type-title-2 text-label">{title}</h2>
        {body && <p className="type-body text-label-secondary">{body}</p>}
        {finePrint && <p className="type-footnote text-label-tertiary">{finePrint}</p>}
      </div>
      {children && <div className="flex w-full flex-col gap-(--space-md)">{children}</div>}
    </div>
  )
}

/**
 * One step's card, body copy and content only — no button. `transitionName`
 * is the step's shared name while this card owns it (the takeover's centred
 * card, or an open modal), and `undefined` while its docked row owns it
 * instead.
 */
export function ProgressStepCard({
  step,
  progress,
  isSteady,
  transitionName,
}: {
  step: ProgressStepKey
  progress: OverviewProgress
  isSteady: boolean
  transitionName: string | undefined
}) {
  const permissionFlow = useReadPermissionFlow(progress)
  switch (step) {
    case "agents":
      return (
        <StepCard
          transitionName={transitionName}
          title="Finding agents"
          body="Scanning the last 30 days of session logs to find out which coding agents you're using on this machine."
        >
          <AgentsStepRow snapshot={progress} />
        </StepCard>
      )
    case "sessions":
      return (
        <StepCard
          transitionName={transitionName}
          title="Reading sessions"
          body="antiburn pulls each session's metadata - every line of the log - into a local unencrypted sqlite db, for indexed access."
        >
          <SessionsStepRow snapshot={progress} permissionFlow={permissionFlow} />
        </StepCard>
      )
    case "checks":
      return (
        <StepCard
          transitionName={transitionName}
          title="Running session checks"
          body="antiburn checks for anti-patterns, especially problems with the context window, caching, and unused tools or skills."
        >
          <ChecksStepRow snapshot={progress} isSteady={isSteady} />
        </StepCard>
      )
    case "fixes": {
      const failing = progress.categories.filter((category) => category.status === "needsFix")
      const history = progress.history
      return (
        <StepCard
          transitionName={transitionName}
          title={
            progress.checks.windowSessions === 0
              ? "No sessions in the last 30 days"
              : fixesFound(progress)
                ? `${progress.failingCount} ${pluralize(progress.failingCount, "fix", "fixes")} found in your config`
                : "No fixes needed"
          }
          {...(fixesFound(progress)
            ? { body: fixesSubtitle(failing) }
            : progress.checks.windowSessions > 0
              ? { body: "Your config already looks efficient." }
              : {})}
          {...(history
            ? {
                finePrint: `Reading older history · ${fmt(history.completed)} of ${fmt(history.total)}`,
              }
            : {})}
        />
      )
    }
  }
}
