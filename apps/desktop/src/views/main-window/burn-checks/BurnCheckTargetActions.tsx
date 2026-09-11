import { Check, Clipboard, Wrench } from "lucide-react"
import { useCallback, useRef, useState } from "react"
import { flushSync } from "react-dom"

import {
  noteInteraction,
  type AutoFixAnalyticsOutcome,
  type AutoFixReviewAnalyticsOutcome,
  type PromptPreparationAnalyticsOutcome,
} from "../../../lib/ipc"
import {
  applyPreparedBurnCheckOperation,
  copyPromptFixBurnCheckTarget,
  prepareAutoFixBurnCheckTarget,
  type AutoFixReviewPayload,
  type AutoFixUnavailableReason,
  type BurnCheckTargetPayload,
} from "../../../lib/insightsIpc"
import { BurnCheckReviewDialog } from "./BurnCheckReviewDialog"
import { targetTitle } from "./BurnCheckTargetPresentation"

type ActionState = {
  attemptKey: string
  acceptedWatchId: string | null
  busy: "prepare" | "apply" | "copy" | null
  review: AutoFixReviewPayload | null
  reviewBlocked: boolean
  copied: boolean
  prompt: string | null
  applied: boolean
  status: string | null
}

type FailedCommandOutcome<Outcome, Success extends string> = Exclude<
  Outcome,
  null | { outcome: Success }
>

type FailureInput =
  | {
      stage: "prepare"
      outcome: FailedCommandOutcome<
        Awaited<ReturnType<typeof prepareAutoFixBurnCheckTarget>>,
        "reviewReady"
      > | null
    }
  | {
      stage: "apply"
      outcome: FailedCommandOutcome<
        Awaited<ReturnType<typeof applyPreparedBurnCheckOperation>>,
        "appliedAwaitingVerification"
      > | null
    }

function attemptKey(target: BurnCheckTargetPayload): string {
  const watch = target.watch
  if (!watch) return `${target.findingId}:unwatched`
  if (watch.verification.status === "recurred") {
    return `${watch.watchId}:recurred:${watch.verification.methodRevision}:${watch.verification.evidenceRevision}`
  }
  return watch.watchId
}

function initialAction(key: string): ActionState {
  return {
    attemptKey: key,
    acceptedWatchId: null,
    busy: null,
    review: null,
    reviewBlocked: false,
    copied: false,
    prompt: null,
    applied: false,
    status: null,
  }
}

function reviewAnalytics(
  outcome: Awaited<ReturnType<typeof prepareAutoFixBurnCheckTarget>>,
): AutoFixReviewAnalyticsOutcome {
  if (!outcome) return "failed"
  return outcome.outcome === "reviewReady" ? "ready" : outcome.outcome
}

function applyAnalytics(
  outcome: Awaited<ReturnType<typeof applyPreparedBurnCheckOperation>>,
): AutoFixAnalyticsOutcome {
  if (!outcome) return "failed"
  if (outcome.outcome === "appliedAwaitingVerification") {
    return "applied_awaiting_verification"
  }
  return outcome.outcome === "recoveryNeeded" ? "recovery_needed" : outcome.outcome
}

function promptAnalytics(
  outcome: Awaited<ReturnType<typeof copyPromptFixBurnCheckTarget>>,
): PromptPreparationAnalyticsOutcome {
  if (!outcome) return "failed"
  return outcome.outcome === "promptReady" ? "ready" : outcome.outcome
}

function unavailableMessage(reason: AutoFixUnavailableReason): string {
  switch (reason) {
    case "activeWatch":
      return "Another change for this finding is already being checked."
    case "safetyCheckFailed":
      return "The current setting no longer passes the write safety check."
    case "targetNotFound":
      return "This exact setting is no longer available."
    case "unsupportedOrUnprovenTarget":
      return "Antiburn can no longer prove a safe write target."
  }
}

function commandFailure({ stage, outcome }: FailureInput): string {
  if (!outcome) {
    return stage === "prepare"
      ? "Could not prepare this change. Try again."
      : "Could not confirm the result. Check the setting before you try again."
  }
  switch (outcome.outcome) {
    case "recoveryNeeded":
      return "The write result is uncertain. Review the setting before another change."
    case "expired":
      return stage === "prepare"
        ? "Checking the current change."
        : "Checking the current change before another review."
    case "stale":
      return stage === "prepare"
        ? "Checking the current change."
        : "Checking the current change before another review."
    case "conflict":
      return stage === "prepare"
        ? "Another prepared change conflicts with this setting. Refresh and review it again."
        : "Another change now conflicts with this operation. Close this review and check the setting."
    case "unavailable":
      return unavailableMessage(outcome.reason)
  }
}

export function BurnCheckTargetActions({
  target,
  refresh,
  showPromptFix = true,
  embedded = false,
}: {
  target: BurnCheckTargetPayload
  refresh: () => void
  showPromptFix?: boolean
  embedded?: boolean
}) {
  const key = attemptKey(target)
  const [action, setAction] = useState(() => initialAction(key))
  const trigger = useRef<HTMLButtonElement>(null)
  const currentAttemptKey = useRef(key)
  const copiedTimeout = useRef<ReturnType<typeof setTimeout> | null>(null)
  const appliedTimeout = useRef<ReturnType<typeof setTimeout> | null>(null)
  if (action.attemptKey !== key && action.busy !== "apply") {
    const continuesCreatedWatch =
      target.watch?.watchId === action.acceptedWatchId &&
      target.watch.verification.status !== "recurred"
    setAction(continuesCreatedWatch ? { ...action, attemptKey: key } : initialAction(key))
  }

  const completionIsStale = (startedKey: string, createdWatchId: string | null = null) =>
    currentAttemptKey.current !== startedKey && currentAttemptKey.current !== createdWatchId

  const clearStaleApply = (startedKey: string, createdWatchId: string | null = null) => {
    if (!completionIsStale(startedKey, createdWatchId)) return false
    flushSync(() => {
      setAction((value) =>
        value.attemptKey === startedKey ? initialAction(currentAttemptKey.current) : value,
      )
    })
    trigger.current?.focus()
    return true
  }

  const closeReview = () => {
    if (action.busy === "apply") return
    setAction((value) => ({ ...value, review: null, reviewBlocked: false, status: null }))
    queueMicrotask(() => trigger.current?.focus())
  }

  const scheduleSuccessReset = (
    kind: "copied" | "applied",
    startedAttemptKey: string,
    acceptedWatchId: string | null,
  ) => {
    const timeout = kind === "copied" ? copiedTimeout : appliedTimeout
    if (timeout.current) clearTimeout(timeout.current)
    timeout.current = setTimeout(() => {
      timeout.current = null
      if (completionIsStale(startedAttemptKey, acceptedWatchId)) return
      setAction((value) =>
        value.attemptKey === startedAttemptKey || value.attemptKey === acceptedWatchId
          ? { ...value, [kind]: false }
          : value,
      )
    }, 3_000)
  }

  const bindActionRoot = useCallback(
    (node: HTMLDivElement | null) => {
      if (node) currentAttemptKey.current = key
      else {
        currentAttemptKey.current = ""
        if (copiedTimeout.current) clearTimeout(copiedTimeout.current)
        if (appliedTimeout.current) clearTimeout(appliedTimeout.current)
      }
    },
    [key],
  )

  const prepare = async () => {
    if (action.busy) return
    const startedAttemptKey = action.attemptKey
    setAction((value) => ({ ...value, busy: "prepare", status: null }))
    try {
      const outcome = await prepareAutoFixBurnCheckTarget(target.actionId)
      noteInteraction({ kind: "burnCheckAutoFixReviewed", outcome: reviewAnalytics(outcome) })
      if (completionIsStale(startedAttemptKey)) return
      if (outcome?.outcome === "reviewReady") {
        setAction((value) => ({
          ...value,
          busy: null,
          review: outcome.review,
          reviewBlocked: false,
        }))
      } else {
        setAction((value) => ({
          ...value,
          busy: null,
          status: commandFailure({ stage: "prepare", outcome }),
        }))
        if (outcome?.outcome === "expired" || outcome?.outcome === "stale") refresh()
      }
    } catch {
      noteInteraction({ kind: "burnCheckAutoFixReviewed", outcome: "failed" })
      if (completionIsStale(startedAttemptKey)) return
      setAction((value) => ({
        ...value,
        busy: null,
        status: commandFailure({ stage: "prepare", outcome: null }),
      }))
    }
  }

  const apply = async () => {
    if (action.busy || action.reviewBlocked) return
    const operationId = action.review?.preparedOperationId
    if (!operationId) return
    const startedAttemptKey = action.attemptKey
    noteInteraction({ kind: "burnCheckAutoFixConfirmed" })
    setAction((value) => ({ ...value, busy: "apply", status: null }))
    try {
      const outcome = await applyPreparedBurnCheckOperation(operationId)
      const completedWatchId =
        outcome?.outcome === "appliedAwaitingVerification" ||
        outcome?.outcome === "recoveryNeeded"
          ? outcome.watchId
          : null
      noteInteraction({ kind: "burnCheckAutoFixCompleted", outcome: applyAnalytics(outcome) })
      if (clearStaleApply(startedAttemptKey, completedWatchId)) return
      if (outcome?.outcome === "appliedAwaitingVerification") {
        flushSync(() => {
          setAction((value) => ({
            ...value,
            acceptedWatchId: outcome.watchId,
            busy: null,
            review: null,
            status: null,
          }))
        })
        trigger.current?.focus()
        setAction((value) => ({ ...value, applied: true }))
        scheduleSuccessReset("applied", startedAttemptKey, outcome.watchId)
        refresh()
        return
      }
      setAction((value) => ({
        ...value,
        acceptedWatchId:
          outcome?.outcome === "recoveryNeeded" ? outcome.watchId : value.acceptedWatchId,
        busy: null,
        reviewBlocked: true,
        status: commandFailure({ stage: "apply", outcome }),
      }))
      if (outcome?.outcome === "recoveryNeeded") refresh()
      if (outcome?.outcome === "expired" || outcome?.outcome === "stale") {
        setAction((value) => ({ ...value, review: null, reviewBlocked: false }))
        refresh()
      }
    } catch {
      noteInteraction({ kind: "burnCheckAutoFixCompleted", outcome: "failed" })
      if (clearStaleApply(startedAttemptKey)) return
      setAction((value) => ({
        ...value,
        busy: null,
        reviewBlocked: true,
        status: commandFailure({ stage: "apply", outcome: null }),
      }))
    }
  }

  const copy = async () => {
    if (action.busy || action.copied) return
    const startedAttemptKey = action.attemptKey
    setAction((value) => ({ ...value, busy: "copy", status: null }))
    let prompt = action.prompt
    let acceptedWatchId = action.acceptedWatchId
    try {
      if (!prompt) {
        const outcome = await copyPromptFixBurnCheckTarget(target.actionId)
        const completedWatchId =
          outcome?.outcome === "promptReady" ? outcome.watch.watchId : null
        noteInteraction({ kind: "burnCheckPromptPrepared", outcome: promptAnalytics(outcome) })
        if (completionIsStale(startedAttemptKey, completedWatchId)) return
        if (!outcome || outcome.outcome !== "promptReady") {
          setAction((value) => ({
            ...value,
            busy: null,
            status:
              outcome?.outcome === "expired" || outcome?.outcome === "stale"
                ? "Checking the current change."
                : "A prompt fix is unavailable for this finding.",
          }))
          if (outcome?.outcome === "expired" || outcome?.outcome === "stale") refresh()
          return
        }
        prompt = outcome.prompt
        acceptedWatchId = outcome.watch.watchId
      }
      if (!navigator.clipboard) throw new Error("Clipboard unavailable")
      await navigator.clipboard.writeText(prompt)
      if (completionIsStale(startedAttemptKey, acceptedWatchId)) return
      noteInteraction({ kind: "burnCheckPromptCopied" })
      setAction((value) => ({
        ...value,
        acceptedWatchId,
        busy: null,
        prompt,
        copied: true,
        status: null,
      }))
      scheduleSuccessReset("copied", startedAttemptKey, acceptedWatchId)
      refresh()
    } catch {
      if (!prompt) noteInteraction({ kind: "burnCheckPromptPrepared", outcome: "failed" })
      if (completionIsStale(startedAttemptKey, acceptedWatchId)) return
      setAction((value) => ({
        ...value,
        busy: null,
        prompt,
        status: "Could not copy the prompt. Check clipboard access and try again.",
      }))
    }
  }

  const title = targetTitle(target)
  const hasAction =
    target.autoFix.status === "available" ||
    (showPromptFix && target.promptFix.status === "available")
  return (
    <>
      {hasAction && (
        <div
          ref={bindActionRoot}
          className={`${embedded ? "" : "mt-3 "}flex flex-wrap items-center gap-2`}
        >
          {target.autoFix.status === "available" && (
            <button
              ref={trigger}
              type="button"
              disabled={action.applied || action.busy !== null}
              onClick={() => void prepare()}
              className="ui-push-button burn-check-action type-callout gap-1 disabled:opacity-100"
            >
              {action.applied ? (
                <Check size={12} className="text-share-work-text" aria-hidden="true" />
              ) : (
                <Wrench size={12} aria-hidden="true" />
              )}
              {action.applied
                ? "Change applied"
                : action.busy === "prepare"
                  ? "Preparing…"
                  : "Fix"}
            </button>
          )}
          {showPromptFix && target.promptFix.status === "available" && (
            <button
              type="button"
              disabled={action.copied || action.busy !== null}
              onClick={() => void copy()}
              className="ui-push-button burn-check-action type-callout gap-1 disabled:opacity-100"
            >
              {action.copied ? (
                <Check size={12} className="text-share-work-text" aria-hidden="true" />
              ) : (
                <Clipboard size={12} aria-hidden="true" />
              )}
              {action.copied ? "Copied" : "Copy fix prompt"}
            </button>
          )}
        </div>
      )}
      {action.status && !action.review && (
        <p role="alert" className="mt-3 type-callout text-system-red-text">
          {action.status}
        </p>
      )}
      {action.review && (
        <BurnCheckReviewDialog
          title={title}
          titleId={`fix-${target.findingId}-title`}
          review={action.review}
          busy={action.busy === "apply"}
          blocked={action.reviewBlocked}
          status={action.status}
          close={closeReview}
          apply={() => void apply()}
        />
      )}
    </>
  )
}
