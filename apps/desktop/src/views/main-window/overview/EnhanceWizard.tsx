import { Check } from "lucide-react"
import { Fragment, type ReactNode } from "react"

import { PushButton } from "../../../components/ui/PushButton"
import { cn } from "../../../lib/cn"
import type { BurnChecksSession, BurnChecksSnapshot } from "../BurnChecksSession"
import { FixStep, ScanStep, SourcesStep } from "./EnhanceSteps"

const ENHANCE_STEPS = [
  {
    label: "Sources",
    heading: "Here's what antiburn reads",
    lead: "antiburn reads the session files your agents write. Nothing leaves your computer.",
  },
  {
    label: "Scan",
    heading: "Here's what's burning tokens",
    lead: "We ran every burn check on your last 30 days of sessions.",
  },
  {
    label: "Fix",
    heading: "Fix the biggest burn first",
    lead: "Apply a fix, copy a prompt for your agent, or snooze it for later.",
  },
  {
    label: "Watch",
    heading: "Now keep working",
    lead: "antiburn watches your next sessions and tells you when a fix is confirmed.",
  },
  {
    label: "Done",
    heading: "Your AI setup is sorted",
    lead: "Here's what your fixes save at your last 30 days' pace.",
  },
] as const

export type EnhanceStep = 1 | 2 | 3 | 4 | 5

export function EnhanceWizard({
  step,
  checks,
  checksState,
  onStepChange,
  onFinish,
}: {
  step: EnhanceStep
  checks: BurnChecksSession
  checksState: BurnChecksSnapshot
  onStepChange: (step: EnhanceStep) => void
  onFinish: () => void
}) {
  const current = ENHANCE_STEPS[step - 1] ?? ENHANCE_STEPS[0]
  const body: ReactNode =
    step === 1 ? (
      <SourcesStep />
    ) : step === 2 ? (
      <ScanStep state={checksState} />
    ) : step === 3 ? (
      <FixStep session={checks} state={checksState} />
    ) : null
  const last = step === ENHANCE_STEPS.length
  return (
    <section aria-label="Enhance my AI setup" className="flex min-h-0 min-w-0 flex-1 flex-col">
      <nav
        aria-label="Enhance steps"
        className="flex items-center gap-(--space-xs) border-b border-separator pb-(--space-md)"
      >
        {ENHANCE_STEPS.map((item, index) => {
          const number = (index + 1) as EnhanceStep
          const done = number < step
          const on = number === step
          return (
            <Fragment key={item.label}>
              {index > 0 && (
                <span
                  aria-hidden="true"
                  className={cn(
                    "h-px w-6 shrink-0",
                    done || on ? "bg-burn-check-pass-fill" : "bg-separator",
                  )}
                />
              )}
              <button
                type="button"
                aria-current={on ? "step" : undefined}
                onClick={() => onStepChange(number)}
                className={cn(
                  "flex items-center gap-(--space-sm) rounded-full px-(--space-sm) py-(--space-xs) type-callout whitespace-nowrap",
                  on
                    ? "bg-surface-card font-semibold text-label"
                    : done
                      ? "text-label-secondary hover:bg-surface-hover"
                      : "text-label-tertiary hover:bg-surface-hover",
                )}
              >
                <span
                  aria-hidden="true"
                  className={cn(
                    "grid size-5 place-items-center rounded-full type-metadata font-semibold",
                    done
                      ? "bg-burn-check-pass-fill text-white"
                      : on
                        ? "border border-brand-tint text-brand"
                        : "border border-separator",
                  )}
                >
                  {done ? <Check size={11} strokeWidth={3} /> : number}
                </span>
                {item.label}
              </button>
            </Fragment>
          )
        })}
      </nav>

      <div key={step} className="animate-step-in min-h-0 flex-1 overflow-auto py-(--space-2xl)">
        <h2 className="type-title-2 text-label">{current.heading}</h2>
        <p className="mt-(--space-xs) max-w-[60ch] type-body text-label-secondary">
          {current.lead}
        </p>
        {body && <div className="mt-(--space-xl)">{body}</div>}
      </div>

      <footer className="flex items-center justify-end gap-(--space-md) border-t border-separator pt-(--space-lg)">
        {step > 1 && (
          <PushButton onClick={() => onStepChange((step - 1) as EnhanceStep)}>Back</PushButton>
        )}
        <PushButton
          variant="primary"
          onClick={() => (last ? onFinish() : onStepChange((step + 1) as EnhanceStep))}
        >
          {last ? "Back to Overview" : "Continue"}
        </PushButton>
      </footer>
    </section>
  )
}
