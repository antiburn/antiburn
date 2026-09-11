import { CheckCircle2, CircleDashed, Flame, LoaderCircle } from "lucide-react"
import { useCallback, useId, useState } from "react"

import { cn } from "../../../lib/cn"
import { Skeleton } from "../../../components/ui/Skeleton"
import type { ChecksCategoryPayload, ChecksReportPayload } from "../../../lib/insightsIpc"
import { checksHeroPresentation, checksPresentation } from "../../../lib/presentation/checks"
import { checkRowPresentation } from "../../checks/checkUi"
import type { BurnChecksSession, BurnChecksSnapshot } from "../BurnChecksSession"
import { BurnCheckDetail } from "./BurnCheckDetail"
import { BurnCheckTargetDetail } from "./BurnCheckTargetDetail"
import { DisclosureChevron } from "./BurnCheckTargetPresentation"
import { BurnChecksSavings } from "./BurnChecksSavings"

function LoadingCheckDetail() {
  return (
    <article
      role="region"
      aria-label="Loading finding details"
      aria-busy="true"
      className="px-4 py-4"
    >
      <p role="status" className="sr-only">
        Loading finding details.
      </p>
      <Skeleton className="h-4 w-72 max-w-full" />
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Skeleton className="h-[var(--control-height-regular)] w-28" />
        <Skeleton className="h-[var(--control-height-regular)] w-12" />
      </div>
      <div className="mt-3">
        <Skeleton className="h-[17px] w-28" />
      </div>
    </article>
  )
}

function TargetLoadError({ retry }: { retry: () => void }) {
  return (
    <article role="alert" className="px-4 py-4">
      <p className="type-callout text-label-secondary">Could not load this check's details.</p>
      <button type="button" onClick={retry} className="mt-3 ui-push-button">
        Retry
      </button>
    </article>
  )
}

function CheckRow({
  check,
  session,
  state,
  defaultOpen = false,
}: {
  check: ChecksCategoryPayload
  session: BurnChecksSession
  state: BurnChecksSnapshot
  defaultOpen?: boolean
}) {
  const [open, setOpen] = useState(defaultOpen)
  const [deliberateOpen, setDeliberateOpen] = useState(false)
  const bodyId = useId()
  const passed = check.finding === 0 && check.clean > 0
  const presentation = checkRowPresentation(check)
  const { Icon } = presentation
  const detector = check.id
  const targets = state.targets[detector]
  const trackVisibility = useCallback(
    (node: HTMLDivElement | null) =>
      session.setTargetsVisible(detector, node !== null, deliberateOpen),
    [deliberateOpen, detector, session],
  )
  return (
    <section className="border-b border-separator last:border-b-0">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => {
          const next = !open
          setDeliberateOpen(true)
          setOpen(next)
          if (check.finding > 0) session.setTargetsVisible(detector, next)
        }}
        className="grid w-full grid-cols-[28px_minmax(0,1fr)_max-content_14px] items-center gap-x-2 px-3 py-2.5 text-left hover:bg-surface-hover active:transform-none active:opacity-100"
      >
        <span
          className={cn(
            "flex h-7 w-7 items-center justify-center rounded-control",
            presentation.iconTone,
          )}
        >
          <Icon size={15} strokeWidth={2} aria-hidden="true" />
        </span>
        <span className="min-w-0">
          <span className="block truncate type-body font-medium! text-label">
            {presentation.label}
          </span>
          <span className="block truncate type-footnote tabular-nums text-label-tertiary">
            {presentation.summary}
          </span>
        </span>
        {presentation.metric ? (
          <span
            className={cn("type-footnote font-medium! tabular-nums", presentation.metricTone)}
          >
            {presentation.metric}
          </span>
        ) : (
          <span />
        )}
        <DisclosureChevron open={open} />
      </button>
      <div
        id={bodyId}
        ref={open && check.finding > 0 ? trackVisibility : undefined}
        hidden={!open}
        className="border-t border-separator bg-surface-window/40"
      >
        {check.finding > 0 ? (
          targets?.data ? (
            targets.data.targets.length > 0 ? (
              check.id === "unusedSkills" || check.id === "unusedMcpServers" ? (
                <>
                  {targets.data.targets.map((target) => (
                    <BurnCheckTargetDetail
                      key={target.findingId}
                      target={target}
                      refresh={session.refresh}
                    />
                  ))}
                </>
              ) : (
                <>
                  <BurnCheckDetail
                    detector={detector}
                    targets={targets.data.targets}
                    refresh={session.refresh}
                  />
                </>
              )
            ) : (
              <BurnCheckDetail detector={detector} targets={[]} refresh={session.refresh} />
            )
          ) : targets?.error ? (
            <TargetLoadError retry={() => session.loadTargets(detector, true)} />
          ) : (
            <LoadingCheckDetail />
          )
        ) : (
          <div className="flex items-start gap-3 px-4 py-3">
            {passed ? (
              <CheckCircle2 size={16} className="mt-0.5 text-system-green" aria-hidden="true" />
            ) : null}
            <p className="type-callout text-label-secondary">
              {`No finding in ${check.clean} complete sessions.`}
            </p>
          </div>
        )}
      </div>
    </section>
  )
}

function CheckGroup({
  title,
  checks,
  session,
  state,
}: {
  title: string
  checks: readonly ChecksCategoryPayload[]
  session: BurnChecksSession
  state: BurnChecksSnapshot
}) {
  const id = `burn-checks-${title.replaceAll(" ", "-").toLowerCase()}`
  if (checks.length === 0) return null
  return (
    <section className="mt-5" aria-labelledby={id}>
      <h2 id={id} className="px-1 type-caption text-label-tertiary">
        {title}
      </h2>
      <div className="mt-2 overflow-hidden rounded-control border border-separator bg-surface-card/50">
        {checks.map((check, index) => (
          <CheckRow
            key={check.id}
            check={check}
            session={session}
            state={state}
            defaultOpen={index === 0 && title === "Failed checks"}
          />
        ))}
      </div>
    </section>
  )
}

export function BurnChecksReport({
  report,
  session,
  state,
}: {
  report: ChecksReportPayload
  session: BurnChecksSession
  state: BurnChecksSnapshot
}) {
  const presentation = checksPresentation(report)
  const hero = checksHeroPresentation(presentation)
  const HeroIcon =
    hero.state === "failed" ? Flame : hero.state === "passed" ? CheckCircle2 : CircleDashed
  return (
    <>
      <section className="flex flex-wrap items-center gap-4 rounded-control border border-separator bg-surface-card p-4">
        <span
          className={cn(
            "flex h-10 w-10 items-center justify-center rounded-full",
            hero.state === "failed"
              ? "bg-system-red/10 text-system-red-text"
              : hero.state === "passed"
                ? "bg-system-green/10 text-system-green"
                : "bg-surface-secondary text-label-tertiary",
          )}
        >
          <HeroIcon
            size={24}
            strokeWidth={hero.state === "pending" ? 2 : 2.5}
            aria-hidden="true"
          />
        </span>
        <div className="min-w-0 flex-1">
          <p className={cn("type-title-2 tabular-nums", hero.tone)}>{hero.result}</p>
          {(hero.summary || report.pendingEvidence > 0) && (
            <div className="flex items-center gap-3 type-footnote text-label-secondary">
              {hero.summary && <p>{hero.summary}</p>}
              {report.pendingEvidence > 0 && (
                <p className="flex items-center gap-1.5 text-label-tertiary" role="status">
                  <LoaderCircle
                    size={12}
                    strokeWidth={2}
                    className="animate-spin"
                    aria-hidden="true"
                  />
                  {`${report.pendingEvidence} session${report.pendingEvidence === 1 ? "" : "s"} processing`}
                </p>
              )}
            </div>
          )}
        </div>
      </section>
      <BurnChecksSavings wins={state.aggregate?.wins ?? []} />
      <CheckGroup
        title="Failed checks"
        checks={presentation.failures}
        session={session}
        state={state}
      />
      <CheckGroup
        title="Passed checks"
        checks={presentation.wins}
        session={session}
        state={state}
      />
    </>
  )
}
