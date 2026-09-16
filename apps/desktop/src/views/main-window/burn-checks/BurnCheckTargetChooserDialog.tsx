import { Check } from "lucide-react"
import { useId, useState } from "react"
import { createPortal } from "react-dom"

import { PushButton } from "../../../components/ui/PushButton"
import { cn } from "../../../lib/cn"
import {
  noteInteraction,
  type AutoFixAnalyticsOutcome,
  type AutoFixReviewAnalyticsOutcome,
} from "../../../lib/ipc"
import {
  applyPreparedBurnCheckOperation,
  prepareAutoFixBurnCheckTarget,
  type ApplyPreparedBurnCheckOperationOutcome,
  type AutoFixReviewPayload,
  type BurnCheckTargetPayload,
  type PrepareAutoFixBurnCheckTargetOutcome,
} from "../../../lib/insightsIpc"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { scopeLabel, targetTitle } from "./BurnCheckTargetPresentation"

type PreparedTarget = {
  target: BurnCheckTargetPayload
  review: AutoFixReviewPayload
}

type PreparedGroup = {
  key: string
  agent: string
  scope: AutoFixReviewPayload["scope"]
  configFile: string
  items: PreparedTarget[]
}

type TargetGroup = {
  key: string
  agent: string
  scope: BurnCheckTargetPayload["display"]["scopeKind"]
  targets: BurnCheckTargetPayload[]
}

type Step = "select" | "preparing" | "review" | "applying" | "result"

function reviewAnalytics(
  outcome: PrepareAutoFixBurnCheckTargetOutcome | null,
): AutoFixReviewAnalyticsOutcome {
  if (!outcome) return "failed"
  return outcome.outcome === "reviewReady" ? "ready" : outcome.outcome
}

function applyAnalytics(
  outcome: ApplyPreparedBurnCheckOperationOutcome | null,
): AutoFixAnalyticsOutcome {
  if (!outcome) return "failed"
  if (outcome.outcome === "appliedAwaitingVerification") {
    return "applied_awaiting_verification"
  }
  return outcome.outcome === "recoveryNeeded" ? "recovery_needed" : outcome.outcome
}

function sameReview(left: AutoFixReviewPayload, right: AutoFixReviewPayload): boolean {
  return (
    left.agent === right.agent &&
    left.scope === right.scope &&
    left.setting === right.setting &&
    left.configFile === right.configFile &&
    left.selectorLabel === right.selectorLabel &&
    left.currentValue === right.currentValue &&
    left.proposedValue === right.proposedValue &&
    left.behaviorOverrideWarning === right.behaviorOverrideWarning &&
    left.effect === right.effect &&
    left.sideEffect === right.sideEffect
  )
}

function groupPreparedTargets(prepared: PreparedTarget[]): PreparedGroup[] {
  const groups = new Map<string, PreparedGroup>()
  for (const item of prepared) {
    const { agent, scope, configFile } = item.review
    const key = `${agent}\u0000${scope}\u0000${configFile}`
    const group = groups.get(key)
    if (group) group.items.push(item)
    else groups.set(key, { key, agent, scope, configFile, items: [item] })
  }
  return Array.from(groups.values())
}

function groupTargets(targets: BurnCheckTargetPayload[]): TargetGroup[] {
  const groups = new Map<string, TargetGroup>()
  for (const target of targets) {
    const { agent } = target.finding
    const scope = target.display.scopeKind
    const key = `${agent}\u0000${scope}`
    const group = groups.get(key)
    if (group) group.targets.push(target)
    else groups.set(key, { key, agent, scope, targets: [target] })
  }
  return Array.from(groups.values())
}

function selectionInstruction(targets: BurnCheckTargetPayload[]): string {
  const resourceKind = targets[0]?.display.resourceKind
  if (
    !resourceKind ||
    !targets.every((target) => target.display.resourceKind === resourceKind)
  ) {
    return "Select the config changes to review."
  }
  switch (resourceKind) {
    case "builtInTool":
      return "Select optional built-in tools to disable."
    case "mcpServer":
      return "Select the MCP servers to disable."
    case "skill":
      return "Select the skills to disable."
    default:
      return "Select the config changes to review."
  }
}

function preparationMessage(outcome: PrepareAutoFixBurnCheckTargetOutcome | null): string {
  if (!outcome) return "The changes could not be prepared. Try again."
  switch (outcome.outcome) {
    case "reviewReady":
      return ""
    case "stale":
    case "expired":
      return "The findings changed. Review the current targets and try again."
    case "conflict":
      return "A config change conflicts with this selection. Review the current settings."
    case "unavailable":
      return "One or more selected changes no longer pass the safety checks."
  }
}

function applyMessage(
  outcome: ApplyPreparedBurnCheckOperationOutcome | null,
  applied: number,
  total: number,
): string {
  const progress = applied > 0 ? `${applied} of ${total} changes were applied. ` : ""
  if (!outcome) return `${progress}The next change could not be applied.`
  switch (outcome.outcome) {
    case "appliedAwaitingVerification":
      return ""
    case "recoveryNeeded":
      return `${progress}The last write has an uncertain result. Review the config before another change.`
    case "stale":
    case "expired":
      return `${progress}The findings changed before the remaining changes could be applied.`
    case "conflict":
      return `${progress}The next change conflicts with the current config.`
    case "unavailable":
      return `${progress}The next change no longer passes the safety checks.`
  }
}

export function BurnCheckTargetChooserDialog({
  targets,
  refresh,
  close,
}: {
  targets: BurnCheckTargetPayload[]
  refresh: () => void
  close: () => void
}) {
  const titleId = useId()
  const [step, setStep] = useState<Step>("select")
  const [selected, setSelected] = useState<Set<string>>(() => new Set())
  const [prepared, setPrepared] = useState<PreparedTarget[]>([])
  const [status, setStatus] = useState<string | null>(null)
  const [appliedCount, setAppliedCount] = useState(0)
  const selectedTargets = targets.filter((target) => selected.has(target.actionId))
  const targetGroups = groupTargets(targets)
  const preparedGroups = groupPreparedTargets(prepared)
  const busy = step === "preparing" || step === "applying"

  const toggle = (actionId: string) => {
    setSelected((current) => {
      const next = new Set(current)
      if (next.has(actionId)) next.delete(actionId)
      else next.add(actionId)
      return next
    })
    setStatus(null)
  }

  const prepare = async () => {
    if (selectedTargets.length === 0 || busy) return
    setStep("preparing")
    setStatus(null)
    const next: PreparedTarget[] = []
    let failed: PrepareAutoFixBurnCheckTargetOutcome | null = null
    try {
      for (const target of selectedTargets) {
        const outcome = await prepareAutoFixBurnCheckTarget(target.actionId)
        if (!outcome || outcome.outcome !== "reviewReady") {
          failed = outcome
          break
        }
        next.push({ target, review: outcome.review })
      }
    } catch {
      failed = null
    }
    noteInteraction({
      kind: "burnCheckAutoFixReviewed",
      outcome: failed
        ? reviewAnalytics(failed)
        : next.length === selectedTargets.length
          ? "ready"
          : "failed",
    })
    if (next.length !== selectedTargets.length) {
      setStep("select")
      setPrepared([])
      setStatus(preparationMessage(failed))
      if (failed?.outcome === "stale" || failed?.outcome === "expired") refresh()
      return
    }
    setPrepared(next)
    setStep("review")
  }

  const apply = async () => {
    if (prepared.length === 0 || busy) return
    noteInteraction({ kind: "burnCheckAutoFixConfirmed" })
    setStep("applying")
    setStatus(null)
    let applied = 0
    let failed: ApplyPreparedBurnCheckOperationOutcome | null = null
    try {
      for (const item of prepared) {
        const refreshed = await prepareAutoFixBurnCheckTarget(item.target.actionId)
        if (
          !refreshed ||
          refreshed.outcome !== "reviewReady" ||
          !sameReview(item.review, refreshed.review)
        ) {
          failed =
            refreshed?.outcome === "reviewReady" ? { outcome: "conflict" } : (refreshed ?? null)
          break
        }
        const outcome = await applyPreparedBurnCheckOperation(
          refreshed.review.preparedOperationId,
        )
        if (!outcome || outcome.outcome !== "appliedAwaitingVerification") {
          failed = outcome
          break
        }
        applied += 1
        setAppliedCount(applied)
      }
    } catch {
      failed = null
    }
    noteInteraction({
      kind: "burnCheckAutoFixCompleted",
      outcome: failed
        ? applyAnalytics(failed)
        : applied === prepared.length
          ? "applied_awaiting_verification"
          : "failed",
    })
    setAppliedCount(applied)
    setStatus(
      failed
        ? applyMessage(failed, applied, prepared.length)
        : `${applied} ${applied === 1 ? "change" : "changes"} applied.`,
    )
    setStep("result")
    refresh()
  }

  const title =
    step === "select" || step === "preparing"
      ? "Choose changes"
      : step === "result"
        ? appliedCount === prepared.length
          ? "Changes applied"
          : "Some changes need attention"
        : `Review ${prepared.length} ${prepared.length === 1 ? "change" : "changes"}`

  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-surface-window/80 p-6 backdrop-blur-sm"
      onMouseDown={(event) => {
        if (!busy && event.target === event.currentTarget) close()
      }}
    >
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-busy={busy}
        onKeyDown={(event) => {
          if (event.key === "Escape" && !busy) return close()
          if (event.key !== "Tab") return
          const controls = Array.from(
            event.currentTarget.querySelectorAll<HTMLElement>(
              "button:not([disabled]), input:not([disabled])",
            ),
          )
          const first = controls[0]
          const last = controls.at(-1)
          if (event.shiftKey && document.activeElement === first) {
            event.preventDefault()
            last?.focus()
          } else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault()
            first?.focus()
          }
        }}
        className="flex max-h-[calc(100vh-3rem)] w-full max-w-lg flex-col rounded-control border border-separator bg-surface-card p-5 text-label shadow-raised"
      >
        <h4 id={titleId} className="type-title-3 text-label">
          {title}
        </h4>
        {step === "select" || step === "preparing" ? (
          <>
            <p className="mt-2 type-body text-label-secondary">
              {selectionInstruction(targets)}
            </p>
            <div className="mt-3 flex items-center justify-between gap-3">
              <p className="type-callout text-label-tertiary">
                {selectedTargets.length} of {targets.length} selected
              </p>
              <div className="flex shrink-0 gap-2">
                <PushButton
                  disabled={busy || selected.size === targets.length}
                  onClick={() => setSelected(new Set(targets.map((target) => target.actionId)))}
                >
                  Select all
                </PushButton>
                <PushButton
                  disabled={busy || selected.size === 0}
                  onClick={() => setSelected(new Set())}
                >
                  Clear all
                </PushButton>
              </div>
            </div>
            <div className="mt-4 min-h-0 flex-1 space-y-4 overflow-y-auto pr-1">
              {targetGroups.map((group, groupIndex) => (
                <div key={group.key}>
                  <p className="mb-2 type-footnote font-medium text-label-secondary">
                    {agentDisplayName(group.agent)} · {scopeLabel(group.scope)}
                  </p>
                  <div className="space-y-1">
                    {group.targets.map((target, targetIndex) => {
                      const checked = selected.has(target.actionId)
                      const { currentValue, replacementValue } = target.display
                      return (
                        <button
                          key={target.actionId}
                          type="button"
                          role="checkbox"
                          aria-checked={checked}
                          autoFocus={groupIndex === 0 && targetIndex === 0}
                          disabled={busy}
                          onClick={() => toggle(target.actionId)}
                          className={cn(
                            "flex w-full items-center gap-2.5 rounded-control px-3 py-2 text-left transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover disabled:opacity-50",
                            checked ? "bg-surface-selected" : "bg-surface-secondary",
                          )}
                        >
                          <span
                            aria-hidden="true"
                            className={cn(
                              "flex size-3.5 shrink-0 items-center justify-center rounded-small border transition-colors duration-[var(--duration-fast)]",
                              checked
                                ? "border-accent-fill bg-accent-fill text-white"
                                : "border-separator bg-input-fill",
                            )}
                          >
                            {checked && <Check size={12} strokeWidth={3} />}
                          </span>
                          <span className="min-w-0 flex-1 truncate type-body font-medium text-label">
                            {targetTitle(target)}
                          </span>
                          {currentValue && replacementValue && (
                            <span className="shrink-0 type-footnote font-mono text-label-tertiary">
                              {currentValue} → {replacementValue}
                            </span>
                          )}
                        </button>
                      )
                    })}
                  </div>
                </div>
              ))}
            </div>
          </>
        ) : (
          <>
            <p className="mt-2 type-body text-label-secondary">
              These config changes affect future requests.
            </p>
            <div className="mt-4 min-h-0 flex-1 space-y-3 overflow-y-auto pr-1">
              {preparedGroups.map((group) => (
                <div key={group.key} className="rounded-control bg-surface-secondary px-3 py-3">
                  <p className="type-callout font-semibold text-label">
                    {agentDisplayName(group.agent)} · {scopeLabel(group.scope)}
                  </p>
                  <p className="mt-0.5 break-all type-footnote font-mono text-label-tertiary">
                    {group.configFile}
                  </p>
                  <div className="mt-2 divide-y divide-separator">
                    {group.items.map(({ target, review }) => (
                      <div
                        key={target.findingId}
                        className="flex items-start justify-between gap-4 py-2 first:pt-0 last:pb-0"
                      >
                        <span className="min-w-0">
                          <span className="block type-body font-semibold text-label">
                            {targetTitle(target)}
                          </span>
                          <span className="block break-all type-footnote font-mono text-label-tertiary">
                            {review.selectorLabel}
                          </span>
                        </span>
                        <span className="shrink-0 type-callout font-mono text-label">
                          {review.currentValue} → {review.proposedValue}
                        </span>
                      </div>
                    ))}
                  </div>
                </div>
              ))}
            </div>
            {prepared.some(({ review }) => review.behaviorOverrideWarning) && (
              <p role="alert" className="mt-3 type-callout text-system-yellow-text">
                An active override can keep some behavior unchanged after these edits.
              </p>
            )}
          </>
        )}
        {status && (
          <p
            role={step === "result" && appliedCount === prepared.length ? "status" : "alert"}
            className={cn(
              "mt-4 type-callout",
              step === "result" && appliedCount === prepared.length
                ? "text-system-green"
                : "text-system-red-text",
            )}
          >
            {status}
          </p>
        )}
        <div className="mt-5 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
          {step === "select" || step === "preparing" ? (
            <>
              <PushButton disabled={busy} onClick={close}>
                Cancel
              </PushButton>
              <PushButton
                variant="primary"
                disabled={busy || selectedTargets.length === 0}
                onClick={() => void prepare()}
              >
                {step === "preparing"
                  ? "Preparing…"
                  : `Review ${selectedTargets.length} ${selectedTargets.length === 1 ? "change" : "changes"}`}
              </PushButton>
            </>
          ) : step === "review" || step === "applying" ? (
            <>
              <PushButton
                disabled={busy}
                onClick={() => {
                  setPrepared([])
                  setStep("select")
                }}
              >
                Back
              </PushButton>
              <PushButton variant="primary" disabled={busy} onClick={() => void apply()}>
                {step === "applying"
                  ? `Applying ${appliedCount + 1} of ${prepared.length}…`
                  : `Apply ${prepared.length} ${prepared.length === 1 ? "change" : "changes"}`}
              </PushButton>
            </>
          ) : (
            <PushButton onClick={close}>Done</PushButton>
          )}
        </div>
      </section>
    </div>,
    document.body,
  )
}
