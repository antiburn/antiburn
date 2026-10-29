import { Check, Clipboard, Wrench } from "lucide-react"
import { useCallback, useRef, useState } from "react"
import { flushSync } from "react-dom"

import {
  noteInteraction,
  type AutoFixAnalyticsOutcome,
  type AutoFixReviewAnalyticsOutcome,
} from "../../../lib/ipc"
import {
  applyPreparedBurnCheckOperation,
  copyPromptFixBurnCheckTarget,
  prepareAutoFixBurnCheckTarget,
  type AutoFixReviewPayload,
  type BurnCheckTargetPayload,
} from "../../../lib/insightsIpc"
import { BurnCheckReviewDialog } from "./BurnCheckReviewDialog"
import { targetTitle } from "./BurnCheckTargetPresentation"
import { useBurnCheckPromptCopy } from "./useBurnCheckPromptCopy"

type ActionState = {
  attemptKey: string
  acceptedWatchId: string | null
  busy: "prepare" | "apply" | null
  review: AutoFixReviewPayload | null
  reviewBlocked: boolean
  applied: boolean
  status: string | null
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
  if (outcome.outcome === "appliedVerificationUnavailable") {
    return "applied_verification_unavailable"
  }
  return outcome.outcome === "recoveryNeeded" ? "recovery_needed" : outcome.outcome
}

export function BurnCheckTargetActions({
  target,
  refresh,
  showPromptFix = true,
  embedded = false,
  compact = false,
  active = true,
}: {
  target: BurnCheckTargetPayload
  refresh: () => void
  showPromptFix?: boolean
  embedded?: boolean
  compact?: boolean
  active?: boolean
}) {
  const key = attemptKey(target)
  const {
    busy: copyBusy,
    copied,
    status: copyStatus,
    bindRoot: bindCopyRoot,
    copy,
  } = useBurnCheckPromptCopy({
    subject: `${target.finding.detector}:${target.findingId}`,
    active,
    prepare: () => copyPromptFixBurnCheckTarget(target.finding.detector, target.findingId),
    refresh,
    onCopied: refresh,
  })
  const [action, setAction] = useState(() => initialAction(key))
  const trigger = useRef<HTMLButtonElement>(null)
  const currentAttemptKey = useRef(key)
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

  const scheduleSuccessReset = (startedAttemptKey: string, acceptedWatchId: string | null) => {
    if (appliedTimeout.current) clearTimeout(appliedTimeout.current)
    appliedTimeout.current = setTimeout(() => {
      appliedTimeout.current = null
      if (completionIsStale(startedAttemptKey, acceptedWatchId)) return
      setAction((value) =>
        value.attemptKey === startedAttemptKey || value.attemptKey === acceptedWatchId
          ? { ...value, applied: false }
          : value,
      )
    }, 3_000)
  }

  const bindActionRoot = useCallback(
    (node: HTMLDivElement | null) => {
      if (node) currentAttemptKey.current = key
      else {
        currentAttemptKey.current = ""
        if (appliedTimeout.current) clearTimeout(appliedTimeout.current)
      }
    },
    [key],
  )

  const prepare = async () => {
    if (action.busy || copyBusy) return
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
        setAction((value) => ({ ...value, busy: null, status: null }))
        if (outcome?.outcome === "expired" || outcome?.outcome === "stale") refresh()
      }
    } catch {
      noteInteraction({ kind: "burnCheckAutoFixReviewed", outcome: "failed" })
      if (completionIsStale(startedAttemptKey)) return
      setAction((value) => ({ ...value, busy: null, status: null }))
    }
  }

  const apply = async () => {
    if (action.busy || copyBusy || action.reviewBlocked) return
    const operationId = action.review?.preparedOperationId
    if (!operationId) return
    const startedAttemptKey = action.attemptKey
    noteInteraction({ kind: "burnCheckAutoFixConfirmed" })
    setAction((value) => ({ ...value, busy: "apply", status: null }))
    try {
      const outcome = await applyPreparedBurnCheckOperation(operationId)
      const completedWatchId =
        outcome?.outcome === "appliedAwaitingVerification" ||
        outcome?.outcome === "appliedVerificationUnavailable" ||
        outcome?.outcome === "recoveryNeeded"
          ? outcome.watchId
          : null
      noteInteraction({ kind: "burnCheckAutoFixCompleted", outcome: applyAnalytics(outcome) })
      if (clearStaleApply(startedAttemptKey, completedWatchId)) return
      if (
        outcome?.outcome === "appliedAwaitingVerification" ||
        outcome?.outcome === "appliedVerificationUnavailable"
      ) {
        const watchId = outcome.watchId
        const verificationUnavailable = outcome.outcome === "appliedVerificationUnavailable"
        flushSync(() => {
          setAction((value) => ({
            ...value,
            acceptedWatchId: watchId,
            busy: null,
            review: null,
            status: verificationUnavailable
              ? "Change applied. Automatic verification is unavailable."
              : null,
          }))
        })
        trigger.current?.focus()
        setAction((value) => ({ ...value, applied: true }))
        scheduleSuccessReset(startedAttemptKey, watchId)
        refresh()
        return
      }
      setAction((value) => ({
        ...value,
        acceptedWatchId:
          outcome?.outcome === "recoveryNeeded" ? outcome.watchId : value.acceptedWatchId,
        busy: null,
        reviewBlocked: true,
        status:
          outcome?.outcome === "conflict"
            ? "The setting changed during this check. Close it and check the current setting."
            : outcome?.outcome === "unavailable"
              ? "The current setting changed, so this edit was not made."
              : null,
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
        status: "The result could not be confirmed. Check the setting before trying again.",
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
          className={`${embedded ? "" : "mt-3 "}flex ${compact ? "flex-row flex-wrap justify-end" : "flex-col items-center"} gap-2`}
        >
          {target.autoFix.status === "available" && (
            <button
              ref={trigger}
              type="button"
              disabled={action.applied || action.busy !== null || copyBusy}
              onClick={() => void prepare()}
              className="burn-check-action type-callout gap-1 disabled:opacity-100"
            >
              {action.applied ? (
                <Check size={12} className="text-token-in" aria-hidden="true" />
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
              ref={bindCopyRoot}
              type="button"
              disabled={action.busy !== null || copyBusy}
              onClick={() => {
                if (action.busy) return
                setAction((value) => ({ ...value, status: null }))
                void copy()
              }}
              className={`burn-check-action type-callout gap-1 disabled:opacity-100 ${compact ? "" : "mx-auto w-full max-w-sm"}`}
            >
              {copied ? (
                <Check size={12} className="text-token-in" aria-hidden="true" />
              ) : (
                <Clipboard size={12} aria-hidden="true" />
              )}
              {copied ? "Copied" : copyBusy ? "Copying…" : "Copy fix prompt"}
            </button>
          )}
        </div>
      )}
      {copyStatus && (
        <p role="alert" className="mt-3 type-callout text-label-secondary">
          {copyStatus}
        </p>
      )}
      {action.status && !action.review && (
        <p role="alert" className="mt-3 type-callout text-label-secondary">
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
