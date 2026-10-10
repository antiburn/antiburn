import type { ReactNode } from "react"

import { CountUp } from "../../../components/ui/CountUp"
import { cn } from "../../../lib/cn"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { noteInteraction, scanNow } from "../../../lib/ipc"
import { CHECK_PROBLEM_PHRASES } from "../../../lib/presentation/checkDefinitions"
import {
  useFolderPermissionFlow,
  type FolderPermissionFlow,
} from "../../../lib/useFolderPermissionFlow"
import {
  fixesFound,
  type FixCategory,
  type OverviewProgress,
  type ProgressStepKey,
} from "./overviewProgressStore"

function useReadPermissionFlow(progress: OverviewProgress): FolderPermissionFlow {
  return useFolderPermissionFlow(progress.sessions.deferred, () => {
    noteInteraction({ kind: "firstRunAction", action: "folder_access_granted" })
    void scanNow()
  })
}

function fmt(value: number): string {
  return value.toLocaleString()
}

function pluralize(count: number, singular: string, plural: string): string {
  return count === 1 ? singular : plural
}

function capitalize(value: string): string {
  return value.length === 0 ? value : value[0]!.toUpperCase() + value.slice(1)
}

const COUNT_WORDS = [
  "zero",
  "one",
  "two",
  "three",
  "four",
  "five",
  "six",
  "seven",
  "eight",
  "nine",
  "ten",
]

function countWord(count: number): string {
  return COUNT_WORDS[count] ?? String(count)
}

function FixesList({ failing }: { failing: FixCategory[] }) {
  return (
    <ul className="mx-auto flex list-disc flex-col gap-(--space-sm) ps-6 text-start type-title-3 font-normal! text-label-secondary">
      {failing.map((category) => (
        <li key={category.id}>{capitalize(CHECK_PROBLEM_PHRASES[category.id])}</li>
      ))}
    </ul>
  )
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
  const { done, displayCompleted, displayTotal } = snapshot.sessions
  const started = done || displayTotal > 0

  const data = {
    completed: displayCompleted,
    started,
    title: "Read session data",
    total: displayTotal,
    emptyLabel: "No sessions found",
  }

  return (
    <div className="flex flex-col gap-2">
      <StepProgressBar {...data} />

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

  const data = {
    completed,
    started,
    title: "Run session checks",
    total: windowSessions,
    emptyLabel: "No sessions to check",
  }

  return (
    <div className="flex flex-col gap-2">
      <StepProgressBar {...data} />
    </div>
  )
}

export const PRIMARY_BUTTON =
  "rounded-control bg-brand-tint px-10 py-3 type-title-3 font-semibold! text-white shadow-[var(--shadow-raised)] transition-[filter] duration-fast hover:brightness-110 active:brightness-95 disabled:opacity-50"

export const SKIP_BUTTON =
  "type-callout text-label-secondary underline underline-offset-[3px] hover:text-label"

function StepCard({
  transitionName,
  title,
  body,
  bodyAction,
  childrenFirst = false,
  children,
}: {
  transitionName: string | undefined
  title: string
  body?: string
  bodyAction?: ReactNode
  childrenFirst?: boolean
  children?: ReactNode
}) {
  const content = children && (
    <div className="mx-auto flex w-full max-w-lg flex-col gap-(--space-md)">{children}</div>
  )
  return (
    <div
      style={transitionName ? { viewTransitionName: transitionName } : undefined}
      className="flex w-full max-w-xl flex-col items-center gap-(--space-lg)"
    >
      {childrenFirst && content}
      <div className="flex flex-col gap-(--space-xs) text-center">
        <h2 className="type-title-1 text-label">{title}</h2>
        {body && (
          // A narrow, balanced body wraps to lines of even width under the title.
          <p className="mx-auto max-w-lg text-balance type-title-3 font-normal! text-label-secondary">
            {body}
            {bodyAction && <> {bodyAction}</>}
          </p>
        )}
      </div>
      {!childrenFirst && content}
    </div>
  )
}

function stepCopy(
  step: Exclude<ProgressStepKey, "fixes">,
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
  }
}

export function ProgressStepCard({
  step,
  progress,
  transitionName,
  bodyAction,
}: {
  step: ProgressStepKey
  progress: OverviewProgress
  transitionName: string | undefined
  bodyAction?: ReactNode
}) {
  const permissionFlow = useReadPermissionFlow(progress)
  switch (step) {
    case "agents":
      return (
        <StepCard
          transitionName={transitionName}
          bodyAction={bodyAction}
          {...stepCopy("agents", progress)}
          childrenFirst
        >
          <AgentsStepRow snapshot={progress} />
        </StepCard>
      )
    case "sessions":
      return (
        <StepCard
          transitionName={transitionName}
          bodyAction={bodyAction}
          {...stepCopy("sessions", progress)}
        >
          <SessionsStepRow snapshot={progress} permissionFlow={permissionFlow} />
        </StepCard>
      )
    case "checks":
      return (
        <StepCard
          transitionName={transitionName}
          bodyAction={bodyAction}
          {...stepCopy("checks", progress)}
        >
          <ChecksStepRow snapshot={progress} />
        </StepCard>
      )
    case "fixes": {
      const failing = progress.categories.filter((category) => category.status === "needsFix")
      return (
        <StepCard transitionName={transitionName} {...fixesSummary(progress)}>
          {fixesFound(progress) && <FixesList failing={failing} />}
        </StepCard>
      )
    }
  }
}

function fixesSummary(progress: OverviewProgress): { title: string; body?: string } {
  if (progress.checks.windowSessions === 0) return { title: "No sessions in the last 30 days" }
  if (fixesFound(progress)) {
    return {
      title: `${capitalize(countWord(progress.failingCount))} fixable ${pluralize(progress.failingCount, "issue", "issues")} found`,
    }
  }
  return { title: "Nice work!", body: "No fixes needed. Your config already looks efficient." }
}
