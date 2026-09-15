import { createPortal } from "react-dom"

import type { AutoFixReviewPayload } from "../../../lib/insightsIpc"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { scopeLabel } from "./BurnCheckTargetPresentation"

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
          Fix {title}
        </h4>
        <p className="mt-2 type-body text-label-secondary">
          {review.effect === "futureModelSelection"
            ? "This plan changes future model selection. Existing sessions do not change."
            : "This plan lowers reasoning effort for future requests. Existing sessions do not change."}
        </p>
        <dl className="mt-5 grid grid-cols-2 gap-3 rounded-control bg-surface-secondary px-3 py-3">
          <div>
            <dt className="type-footnote text-label-tertiary">Agent</dt>
            <dd className="mt-0.5 type-callout text-label">{agentDisplayName(review.agent)}</dd>
          </div>
          <div>
            <dt className="type-footnote text-label-tertiary">Setting</dt>
            <dd className="mt-0.5 type-callout text-label">
              {review.setting === "model" ? "Model" : "Reasoning effort"}
            </dd>
          </div>
          <div className="col-span-2">
            <dt className="type-footnote text-label-tertiary">Scope</dt>
            <dd className="mt-0.5 type-callout text-label">{scopeLabel(review.scope)}</dd>
          </div>
          <div className="col-span-2">
            <dt className="type-footnote text-label-tertiary">Config file</dt>
            <dd className="mt-0.5 break-all type-callout font-mono text-label">
              {review.configFile}
            </dd>
          </div>
          <div className="col-span-2">
            <dt className="type-footnote text-label-tertiary">Config change</dt>
            <dd className="mt-0.5 type-callout font-mono text-label">
              {review.currentValue} → {review.proposedValue}
            </dd>
          </div>
        </dl>
        {status && (
          <p role="alert" className="mt-4 type-callout text-system-red-text">
            {status}
          </p>
        )}
        <div className="mt-5 flex justify-end gap-2">
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
