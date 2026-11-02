import { ArrowRight, ArrowUpRight, ChevronLeft, Clock3, Copy, Gauge, X } from "lucide-react"
import { useCallback, useId, useState, type Ref } from "react"
import type { PromotionAction } from "../../promotions/types"
import { writeClipboardText } from "../../lib/clipboard"
import { runPromotionAction } from "../../lib/promotions/promotionActions"
import type { PromotionSession, PromotionSnapshot } from "../../lib/promotions/PromotionSession"
import { PromotionMedia } from "./PromotionMedia"
import "./promotion.css"

const icons = { gauge: Gauge, clock: Clock3 }

class CopyFeedback {
  private timer: ReturnType<typeof setTimeout> | null = null
  private alive = false
  attach = (node: HTMLButtonElement): (() => void) => {
    this.alive = true
    void node
    return () => {
      this.alive = false
      if (this.timer) clearTimeout(this.timer)
    }
  }
  async copy(command: string, update: (state: "copied" | "idle") => void): Promise<void> {
    try {
      await writeClipboardText(command)
      if (!this.alive) return
      update("copied")
      this.timer = setTimeout(() => {
        if (this.alive) update("idle")
      }, 2_000)
    } catch {
      if (this.alive) update("idle")
      throw new Error("Could not copy the command. Select and copy it instead.")
    }
  }
}

function CopyCommand({
  command,
  title,
  onCopied,
}: {
  command: string
  title: string
  onCopied: () => void
}) {
  const [feedback] = useState(() => new CopyFeedback())
  const attach = useCallback(
    (node: HTMLButtonElement | null) => (node ? feedback.attach(node) : undefined),
    [feedback],
  )
  const [copied, setCopied] = useState(false)
  const [error, setError] = useState(false)
  return (
    <>
      <button
        ref={attach}
        type="button"
        className="promotion-copy type-footnote text-label-secondary"
        aria-label={copied ? `Copied ${title}` : `Copy ${title}`}
        disabled={copied}
        onClick={() => {
          setError(false)
          void feedback
            .copy(command, (state) => {
              setCopied(state === "copied")
              if (state === "copied") onCopied()
            })
            .catch(() => setError(true))
        }}
      >
        <span className="promotion-copy-fill rounded-control">
          {copied ? "Copied!" : <Copy size={14} aria-hidden="true" />}
        </span>
      </button>
      <span className="sr-only" role="status">
        {copied ? `${title} copied!` : ""}
      </span>
      {error && (
        <p className="promotion-copy-error type-footnote text-label-secondary" role="alert">
          Could not copy. Select and copy the command instead.
        </p>
      )}
    </>
  )
}

export function PromotionPanel({
  snapshot,
  session,
  panelRef,
}: {
  snapshot: PromotionSnapshot
  session: PromotionSession
  panelRef?: Ref<HTMLElement>
}) {
  const titleId = useId()
  const [actionError, setActionError] = useState(false)
  const promo = snapshot.selected
  if (!promo) return null
  const setup = snapshot.screen === "setup" ? promo.screens.setup : undefined
  const content = setup ?? promo.screens.announcement
  const analyticsContext = session.analyticsContext()
  const action = (item: PromotionAction) => {
    setActionError(false)
    void runPromotionAction(item, session.navigate)
      .then(() =>
        session.record(
          item === promo.screens.announcement.primary_action
            ? "primary_activated"
            : "learn_more_activated",
          analyticsContext,
        ),
      )
      .catch(() => setActionError(true))
  }
  const close = () => session.close()
  return (
    <section
      ref={panelRef}
      role="dialog"
      aria-modal="false"
      aria-labelledby={titleId}
      className="promotion-panel bg-surface-overlay text-label rounded-promotion"
      data-promotion-id={promo.id}
      data-promotion-screen={snapshot.screen}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (
          event.key === "Escape" &&
          !event.defaultPrevented &&
          !event.nativeEvent.isComposing
        ) {
          event.preventDefault()
          event.stopPropagation()
          close()
        }
      }}
    >
      <button
        type="button"
        className="promotion-close rounded-full text-label-secondary"
        aria-label="Close promotion"
        onClick={close}
      >
        <span className="promotion-close-fill rounded-full">
          <X size={14} strokeWidth={2.5} aria-hidden="true" />
        </span>
      </button>
      {snapshot.screen === "catalog" ? (
        <div className="promotion-catalog">
          <h2 id={titleId} className="type-title-2">
            What’s new
          </h2>
          <ul>
            {session.catalog.map((item) => (
              <li key={item.id}>
                <button
                  type="button"
                  className="promotion-catalog-item type-body"
                  onClick={() => session.select(item.id)}
                >
                  {item.title}
                  <ArrowRight size={14} aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
        </div>
      ) : (
        <>
          {setup && (
            <header className="promotion-navigation">
              <button
                type="button"
                className="promotion-back type-callout text-label-secondary"
                onClick={() => session.navigate("announcement")}
              >
                <ChevronLeft size={14} aria-hidden="true" />
                Back
              </button>
            </header>
          )}
          <div className="promotion-scroll">
            {!setup && promo.screens.announcement.media && (
              <PromotionMedia key={promo.id} media={promo.screens.announcement.media} />
            )}
            <div className={setup ? "promotion-body promotion-setup" : "promotion-body"}>
              {!setup && promo.screens.announcement.eyebrow && (
                <p className="promotion-eyebrow type-footnote text-brand">
                  {promo.screens.announcement.eyebrow}
                </p>
              )}
              <h2 id={titleId} className="type-title-2">
                {content.title}
              </h2>
              <p className="promotion-description type-body text-label-secondary">
                {content.description}
              </p>
              {setup ? (
                <ol className="promotion-steps">
                  {setup.steps.map((step, index) => (
                    <li key={`${index}-${step.title}`}>
                      <div className="promotion-step-heading">
                        <span
                          className="promotion-step-number rounded-full type-footnote bg-surface-secondary"
                          aria-hidden="true"
                        >
                          {index + 1}
                        </span>
                        <h3 className="type-headline">{step.title}</h3>
                        {step.command && (
                          <CopyCommand
                            command={step.command}
                            title={step.title}
                            onCopied={() => session.record("command_copied", analyticsContext)}
                          />
                        )}
                      </div>
                      {step.command && (
                        <pre className="promotion-command type-footnote text-label-secondary border-separator rounded-control">
                          <code>{step.command}</code>
                        </pre>
                      )}
                      {step.description && (
                        <p className="promotion-step-description type-callout text-label-secondary">
                          {step.description}
                        </p>
                      )}
                    </li>
                  ))}
                </ol>
              ) : (
                <ul className="promotion-features">
                  {promo.screens.announcement.features?.map((feature) => {
                    const Icon = icons[feature.icon]
                    return (
                      <li key={feature.title}>
                        <Icon size={15} aria-hidden="true" className="text-label-secondary" />
                        <div>
                          <h3 className="type-headline">{feature.title}</h3>
                          <p className="type-callout text-label-secondary">
                            {feature.description}
                          </p>
                        </div>
                      </li>
                    )
                  })}
                </ul>
              )}
              {actionError && (
                <p
                  className="promotion-action-error type-callout text-label-secondary"
                  role="alert"
                >
                  Could not open the destination. Try again.
                </p>
              )}
            </div>
          </div>
          <footer className="promotion-footer border-separator">
            {content.secondary_action && (
              <button
                type="button"
                className="promotion-link type-callout text-label-secondary"
                onClick={() => action(content.secondary_action!)}
              >
                {content.secondary_action.label}
                {content.secondary_action.type === "external" && (
                  <ArrowUpRight size={13} aria-hidden="true" />
                )}
              </button>
            )}
            {!setup && promo.screens.announcement.primary_action && (
              <button
                type="button"
                className="promotion-primary type-callout font-medium! text-brand-action-ink"
                onClick={() => action(promo.screens.announcement.primary_action!)}
              >
                <span className="promotion-primary-fill bg-brand-action rounded-control">
                  {promo.screens.announcement.primary_action.label}
                  <ArrowRight size={14} aria-hidden="true" />
                </span>
              </button>
            )}
          </footer>
        </>
      )}
    </section>
  )
}
