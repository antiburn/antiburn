import { useRef, useSyncExternalStore, type RefObject } from "react"
import { flushSync } from "react-dom"
import { Circle, CircleAlert, CircleCheck } from "lucide-react"

import { cn } from "../../../lib/cn"
import { CHECK_PROBLEM_PHRASES } from "../../../lib/presentation/checkDefinitions"
import {
  dismissFtueCallout,
  enableNonRepoFolders,
  ftueSnapshot,
  subscribeFtue,
  type FtueFixCategory,
  type FtueFixStatus,
  type FtueSnapshot,
} from "./ftueStore"

const FLY_DURATION_MS = 550

function fmt(value: number): string {
  return value.toLocaleString()
}

function pluralize(count: number, singular: string, plural: string): string {
  return count === 1 ? singular : plural
}

function statusLabel(status: FtueFixStatus): string {
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

function fixesSubtitle(failing: FtueFixCategory[]): string {
  const phrases = failing.map((category) => CHECK_PROBLEM_PHRASES[category.id])
  const shown = phrases.slice(0, 3)
  const remaining = phrases.length - shown.length
  const sentence = shown.map((phrase, index) => (index === 0 ? capitalize(phrase) : phrase))
  return remaining > 0 ? `${sentence.join(", ")}, +${remaining} more` : sentence.join(", ")
}

function capitalize(value: string): string {
  return value.length === 0 ? value : value[0]!.toUpperCase() + value.slice(1)
}

export function OverviewFixes() {
  const ftue = useSyncExternalStore(subscribeFtue, ftueSnapshot, ftueSnapshot)
  const ctaRef = useRef<HTMLButtonElement | null>(null)
  const cornerRef = useRef<HTMLButtonElement | null>(null)

  function dismiss(): void {
    const from = ctaRef.current?.getBoundingClientRect()
    flushSync(dismissFtueCallout)
    const target = cornerRef.current
    if (!from || !target) return
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return
    const to = target.getBoundingClientRect()
    const dx = from.left + from.width / 2 - (to.left + to.width / 2)
    const dy = from.top + from.height / 2 - (to.top + to.height / 2)
    target.animate(
      [
        {
          transform: `translate(${dx}px, ${dy}px) scale(${from.width / to.width}, ${from.height / to.height})`,
        },
        { transform: "none" },
      ],
      { duration: FLY_DURATION_MS, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" },
    )
  }

  const allStepsDone = ftue.find.done && ftue.read.done && ftue.check.done
  const isEmpty = ftue.check.done && ftue.check.windowSessions === 0
  const isClean = ftue.check.done && ftue.check.windowSessions > 0 && ftue.failingCount === 0
  const hasFixes = ftue.check.done && ftue.failingCount > 0

  return (
    <section aria-label="Fixes" className="relative min-h-[220px] flex-1">
      {/* Out of flow, so the list takes the space the page leaves it and
          never grows the page. It clips and fades out at the bottom. */}
      <div
        className={cn(
          "absolute inset-0 flex flex-col overflow-hidden mask-b-from-75%",
          ftue.showSteps
            ? allStepsDone
              ? "transition-opacity [transition-delay:2000ms] [transition-duration:1500ms]"
              : "opacity-0"
            : "",
        )}
      >
        <h2 className="mb-(--space-sm) type-caption text-label-secondary">Config checks</h2>

        <ul className="flex flex-col gap-1">
          {ftue.categories.map((category) => (
            <CheckRow key={category.id} category={category} />
          ))}
        </ul>
      </div>

      {ftue.showSteps && (
        <div
          inert={ftue.dismissed}
          className={cn(
            "absolute inset-0 flex flex-col items-center justify-center gap-(--space-lg) bg-surface-window/75 p-(--space-lg) text-center rounded-(--radius-popover) backdrop-blur-[1.5px] transition-opacity duration-slow",
            ftue.dismissed && "pointer-events-none opacity-0",
          )}
        >
          <div className="flex w-full max-w-[26rem] flex-col gap-(--space-md) text-start">
            <FindStepRow snapshot={ftue} />
            <ReadStepRow snapshot={ftue} />
            <CheckStepRow snapshot={ftue} />
          </div>

          {/* Holds its space while the scan runs, so the bars do not move
              when it arrives. */}
          <div
            inert={!allStepsDone}
            className={cn(
              "flex flex-col items-center gap-(--space-lg)",
              !allStepsDone && "opacity-0",
            )}
          >
            <FixesHeadline
              isEmpty={isEmpty}
              isClean={isClean}
              hasFixes={hasFixes}
              ftue={ftue}
              ctaRef={ctaRef}
              dismissed={ftue.dismissed}
              onDismiss={dismiss}
              offerPlainDismiss
            />
          </div>
        </div>
      )}

      {!ftue.showSteps && ftue.check.done && (
        <div className="flex flex-col items-center gap-(--space-lg) text-center">
          <FixesHeadline
            isEmpty={isEmpty}
            isClean={isClean}
            hasFixes={hasFixes}
            ftue={ftue}
            ctaRef={ctaRef}
            dismissed={ftue.dismissed}
            onDismiss={dismiss}
            offerPlainDismiss={false}
          />
        </div>
      )}

      {ftue.dismissed && ftue.failingCount > 0 && (
        <button
          ref={cornerRef}
          type="button"
          className="absolute right-0 bottom-0 origin-center rounded-control bg-brand-tint px-4 py-1.5 type-callout font-semibold! text-white shadow-[var(--shadow-raised)] transition-[filter] duration-fast hover:brightness-110 active:brightness-95"
        >
          Enhance
        </button>
      )}
    </section>
  )
}

function CheckRow({ category }: { category: FtueFixCategory }) {
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

function FindStepRow({ snapshot }: { snapshot: FtueSnapshot }) {
  const { done, rows } = snapshot.find
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between">
        <span className={cn("type-callout", done ? "text-label-secondary" : "text-label")}>
          Find sessions
        </span>
        {done && <CircleCheck size={14} strokeWidth={2} className="text-label-tertiary" />}
      </div>
      <ul className="flex flex-col gap-0.5 font-mono type-metadata tabular-nums text-label-tertiary">
        {rows.length === 0 ? (
          <li>{done ? "No sessions found" : "Looking…"}</li>
        ) : (
          rows.map((row) => (
            <li key={row.agent} className="flex items-baseline justify-between">
              <span>{row.agent}</span>
              <span>{fmt(row.sessions)}</span>
            </li>
          ))
        )}
      </ul>
    </div>
  )
}

function ReadStepRow({ snapshot }: { snapshot: FtueSnapshot }) {
  const { done, completed, total, gate, includeNonRepoFolders } = snapshot.read
  // Discovery has not told this step how many sessions there are to read
  // yet, so there is nothing to count up from — "0 of 0" would read as
  // already-finished progress rather than a step that has not begun.
  const started = total > 0
  const value = started ? completed / total : 0
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between">
        <span className={cn("type-callout", done ? "text-label-secondary" : "text-label")}>
          Read sessions
        </span>
        {!done && (
          <span className="font-mono type-metadata tabular-nums text-label-tertiary">
            {started ? `${fmt(completed)} of ${fmt(total)}` : "Waiting"}
          </span>
        )}
      </div>
      {!done && (
        <div
          role="progressbar"
          aria-label="Read sessions"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(value * 100)}
          className={cn(
            "h-1.5 overflow-hidden rounded-full bg-surface-tertiary",
            !started && "opacity-40",
          )}
        >
          <div
            className="h-full rounded-full bg-brand-tint transition-[width] duration-medium ease-out"
            style={{ width: `${value * 100}%` }}
          />
        </div>
      )}
      {done && gate && (
        <div className="flex flex-col gap-0.5 type-footnote text-label-tertiary">
          <p>
            {fmt(gate.kept)} {pluralize(gate.kept, "session", "sessions")}{" "}
            {includeNonRepoFolders ? "kept" : "in git repositories"}
          </p>
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

function CheckStepRow({ snapshot }: { snapshot: FtueSnapshot }) {
  const { done, windowSessions, pendingEvidence } = snapshot.check
  // The checks report has its own denominator and settles on its own clock,
  // independent of the scan. Reading it as "done" before step 2 finishes
  // would show a finished check step next to a read step still in progress,
  // so this step waits for step 2 regardless of what the report says.
  const started = snapshot.read.done
  const isDone = started && done
  const completed = Math.max(0, windowSessions - pendingEvidence)
  const value = windowSessions > 0 ? completed / windowSessions : 0
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between">
        <span className={cn("type-callout", isDone ? "text-label-secondary" : "text-label")}>
          Check sessions
        </span>
        <span className="font-mono type-metadata tabular-nums text-label-tertiary">
          {isDone
            ? `Checked ${fmt(windowSessions)}`
            : started
              ? `${fmt(completed)} of ${fmt(windowSessions)}`
              : "Waiting"}
        </span>
      </div>
      {!isDone && (
        <div
          role="progressbar"
          aria-label="Check sessions"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round((started ? value : 0) * 100)}
          className={cn(
            "h-1.5 overflow-hidden rounded-full bg-surface-tertiary",
            !started && "opacity-40",
          )}
        >
          <div
            className="h-full rounded-full bg-brand-tint transition-[width] duration-medium ease-out"
            style={{ width: `${(started ? value : 0) * 100}%` }}
          />
        </div>
      )}
    </div>
  )
}

function FixesHeadline({
  isEmpty,
  isClean,
  hasFixes,
  ftue,
  ctaRef,
  dismissed,
  onDismiss,
  offerPlainDismiss,
}: {
  isEmpty: boolean
  isClean: boolean
  hasFixes: boolean
  ftue: FtueSnapshot
  ctaRef: RefObject<HTMLButtonElement | null>
  dismissed: boolean
  onDismiss: () => void
  /** Whether a state with no Enhance CTA still needs its own Dismiss link —
   *  true only while the steps overlay would otherwise stay blurred over the
   *  checklist with no other way past it. */
  offerPlainDismiss: boolean
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
        {offerPlainDismiss && !dismissed && (
          <button
            type="button"
            onClick={onDismiss}
            className="mt-(--space-sm) type-footnote text-label-secondary underline underline-offset-[3px] hover:text-label"
          >
            Dismiss
          </button>
        )}
      </div>
    )
  }

  if (!hasFixes) return null

  const failing = ftue.categories.filter((category) => category.status === "needsFix")

  return (
    <>
      <div className="mt-6 flex max-w-[36rem] flex-col gap-(--space-xs)">
        <p className="type-title-1 font-semibold! text-label">
          {ftue.failingCount} {pluralize(ftue.failingCount, "fix", "fixes")} found in your
          config
        </p>
        <p className="type-body text-label-secondary">{fixesSubtitle(failing)}</p>
      </div>
      {ftue.history && (
        <p className="type-footnote text-label-tertiary">
          Reading older history · {fmt(ftue.history.completed)} of {fmt(ftue.history.total)}
        </p>
      )}
      <div
        className={cn("flex flex-col items-center gap-(--space-sm)", dismissed && "invisible")}
      >
        <button
          ref={ctaRef}
          type="button"
          className="rounded-control bg-brand-tint px-8 py-2.5 type-headline font-semibold! text-white shadow-[var(--shadow-raised)] transition-[filter] duration-fast hover:brightness-110 active:brightness-95"
        >
          Enhance
        </button>
        <button
          type="button"
          onClick={onDismiss}
          className="type-footnote text-label-secondary underline underline-offset-[3px] hover:text-label"
        >
          Dismiss
        </button>
      </div>
    </>
  )
}
