import { createPortal } from "react-dom"

import type { AutoFixReviewPayload } from "../../../lib/insightsIpc"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { scopeLabel } from "./BurnCheckTargetPresentation"

const settingLabels: Record<AutoFixReviewPayload["setting"], string> = {
  model: "Model",
  reasoning: "Reasoning effort",
  compaction: "Compaction",
  subagentModel: "Subagent model",
  mcpServer: "MCP server",
  builtInTool: "Built-in tool",
  skill: "Skill",
  fastMode: "Fast mode",
}

const sideEffectDescriptions: Record<AutoFixReviewPayload["sideEffect"], string> = {
  modelBehaviorMayChange:
    "Responses can change when future requests use the replacement model.",
  responsesMayUseLessReasoning: "Future responses can use less reasoning.",
  earlierSessionSummarization: "Future sessions can summarize earlier.",
  workerBehaviorMayChange: "Future worker responses can change.",
  serverWillNotBeAvailable: "This server will not be available for future requests.",
  toolWillNotBeAvailable: "This built-in tool will not be available for future requests.",
  skillWillNotBeAvailable: "This skill will not be available for future requests.",
  responsesMayTakeLonger: "Future responses can take longer.",
}

export function BurnCheckReviewDialog({
  title,
  titleId,
  review,
  busy,
  blocked,
  status,
  close,
  apply,
}: {
  title: string
  titleId: string
  review: AutoFixReviewPayload
  busy: boolean
  blocked: boolean
  status: string | null
  close: () => void
  apply: () => void
}) {
  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-surface-window/80 p-6 backdrop-blur-sm"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) close()
      }}
    >
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-busy={busy}
        onKeyDown={(event) => {
          if (event.key === "Escape") return close()
          if (event.key !== "Tab") return
          const buttons = Array.from(
            event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not([disabled])"),
          )
          const first = buttons[0]
          const last = buttons.at(-1)
          if (event.shiftKey && document.activeElement === first) {
            event.preventDefault()
            last?.focus()
          } else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault()
            first?.focus()
          }
        }}
        className="w-full max-w-md rounded-control border border-separator bg-surface-card p-5 text-label shadow-raised"
      >
        <h4 id={titleId} className="type-title-3 text-label">
          Review change
        </h4>
        <p className="mt-2 type-body font-semibold text-label">{title}</p>
        <p className="mt-1 type-body text-label-secondary">
          {sideEffectDescriptions[review.sideEffect]}
        </p>
        <div className="mt-5 rounded-control bg-surface-secondary px-3 py-3">
          <p className="type-callout text-label-secondary">
            {agentDisplayName(review.agent)} · {settingLabels[review.setting]} ·{" "}
            {scopeLabel(review.scope)}
          </p>
          <p className="mt-2 break-all type-footnote font-mono text-label-tertiary">
            {review.configFile} · {review.selectorLabel}
          </p>
          <p className="mt-2 type-callout font-mono text-label">
            {review.currentValue} → {review.proposedValue}
          </p>
        </div>
        {review.behaviorOverrideWarning && (
          <p role="alert" className="mt-4 type-callout text-system-yellow-text">
            An active override can keep current behavior unchanged after this edit.
          </p>
        )}
        {status && (
          <p role="alert" className="mt-4 type-callout text-system-red-text">
            {status}
          </p>
        )}
        <div className="mt-5 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
          <button
            type="button"
            autoFocus
            disabled={busy}
            onClick={close}
            className="ui-push-button"
          >
            {blocked ? "Close" : "Cancel"}
          </button>
          <button
            type="button"
            disabled={busy || blocked}
            onClick={apply}
            className="ui-push-button bg-accent-fill text-white border-transparent"
          >
            {busy ? "Applying…" : "Apply change"}
          </button>
        </div>
      </section>
    </div>,
    document.body,
  )
}
