import { CheckCircle2, CircleDashed, Flame, LoaderCircle } from "lucide-react"
import { useCallback, useId, useState } from "react"

import { cn } from "../../../lib/cn"
import { Skeleton } from "../../../components/ui/Skeleton"
import { SegmentedRadialDial } from "../../../components/ui/SegmentedRadialDial"
import type { ChecksCategoryPayload, ChecksReportPayload } from "../../../lib/insightsIpc"
import { checksHeroPresentation, checksPresentation } from "../../../lib/presentation/checks"
import { checkRowPresentation } from "../../checks/checkUi"
import type { BurnChecksSession, BurnChecksSnapshot } from "../BurnChecksSession"
import { BurnCheckDetail } from "./BurnCheckDetail"
import { BurnCheckTargetDetail } from "./BurnCheckTargetDetail"
import { DisclosureChevron } from "./BurnCheckTargetPresentation"
import { BurnChecksSavings } from "./BurnChecksSavings"

const HERO_DIAL_SIZE = 88
const HERO_DIAL_STROKE = 8
const MIN_BURN_ARC_LENGTH = 4
const MIN_BURN_BASIS_POINTS =
  (MIN_BURN_ARC_LENGTH / (Math.PI * (HERO_DIAL_SIZE - HERO_DIAL_STROKE))) * 10_000

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
        className="grid w-full grid-cols-[28px_minmax(0,1fr)_max-content_14px] items-center gap-x-3 px-4 py-3 text-left hover:bg-surface-hover active:transform-none active:opacity-100"
      >
        <Icon
          size={15}
          strokeWidth={2}
          className="justify-self-center text-label-secondary"
          aria-hidden="true"
        />
        <span className="min-w-0">
          <span className="block truncate type-title-3 text-label">{presentation.label}</span>
          <span
            className={cn(
              "mt-0.5 block truncate type-body tabular-nums",
              check.finding > 0 ? "text-share-waste-text" : "text-label-secondary",
            )}
          >
            {presentation.summary}
          </span>
        </span>
        {presentation.metric ? (
          <span className="inline-flex items-baseline gap-1.5 type-body tabular-nums text-label-secondary">
            {presentation.metric.startsWith("<") && (
              <span className="text-label-secondary">Under</span>
            )}{" "}
            <span className="font-mono">
              {presentation.metric.replace("<", "").replace(" token burn", "")}
            </span>{" "}
            <span className="text-label-secondary">burn</span>
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
                <div className="grid grid-cols-[repeat(auto-fit,minmax(min(100%,18rem),1fr))] items-start gap-3 p-4">
                  {targets.data.targets.map((target) => (
                    <BurnCheckTargetDetail
                      key={target.findingId}
                      target={target}
                      refresh={session.refresh}
                    />
                  ))}
                </div>
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
              <CheckCircle2
                size={16}
                className="mt-0.5 text-label-secondary"
                aria-hidden="true"
              />
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
    <section className="mt-8" aria-labelledby={id}>
      <h2 id={id} className="px-1 type-title-2 text-label">
        {title}
      </h2>
      <div className="mt-3 overflow-hidden rounded-control border border-separator/40 bg-surface-card/50">
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
  const burnBasisPoints = presentation.estimate.tokenBurnBasisPoints
  const displayedBurnBasisPoints =
    burnBasisPoints != null && burnBasisPoints > 0
      ? Math.max(burnBasisPoints, MIN_BURN_BASIS_POINTS)
      : burnBasisPoints
  const hasBurnEstimate = hero.state === "failed" && burnBasisPoints != null
  const HeroIcon =
    hero.state === "failed" ? Flame : hero.state === "passed" ? CheckCircle2 : CircleDashed
  const result = hasBurnEstimate ? hero.result.replace(" token burn", "") : hero.result
  return (
    <>
      <section className="grid grid-cols-[88px_minmax(0,1fr)] items-center gap-[var(--space-2xl)] py-[calc(var(--space-lg)*2)]">
        <div className="grid h-[88px] w-[88px] place-items-center">
          <SegmentedRadialDial
            size={HERO_DIAL_SIZE}
            strokeWidth={HERO_DIAL_STROKE}
            gapAngle={0}
            strokeLinecap="butt"
            label={
              burnBasisPoints == null
                ? "Burn estimate unavailable"
                : `Estimated burn: ${burnBasisPoints / 100}%`
            }
            segments={
              displayedBurnBasisPoints == null
                ? [{ id: "unknown", value: 1, className: "text-surface-tertiary" }]
                : [
                    {
                      id: "burn",
                      value: displayedBurnBasisPoints,
                      className: "text-brand-tint",
                    },
                    {
                      id: "remainder",
                      value: Math.max(0, 10_000 - displayedBurnBasisPoints),
                      className: "text-measure",
                    },
                  ]
            }
          />
        </div>
        <div className="flex min-h-[88px] min-w-0 max-w-sm flex-col justify-between">
          {hasBurnEstimate && (
            <p className="flex items-center gap-1 type-callout text-label-secondary">
              <Flame size={12} strokeWidth={1.75} aria-hidden="true" />
              Estimated burn
            </p>
          )}
          <p className="flex items-center gap-2 type-large-title font-semibold! tabular-nums text-label">
            {!hasBurnEstimate && (
              <HeroIcon
                size={22}
                strokeWidth={1.75}
                className="shrink-0 text-label-secondary"
                aria-hidden="true"
              />
            )}
            {hasBurnEstimate ? result.replace("<", "Less than ") : result}
          </p>
          {hasBurnEstimate && (
            <p className="text-pretty type-body text-label-secondary">
              Of assessed usage could be avoided.
            </p>
          )}
          {hero.summary && (
            <p className="flex items-center gap-1.5 type-callout text-label-secondary">
              {hero.state === "failed" && (
                <span
                  className="h-1 w-1 shrink-0 rounded-full bg-share-waste-text"
                  aria-hidden="true"
                />
              )}
              {hero.summary}
            </p>
          )}
          {report.pendingEvidence > 0 && (
            <p
              className="mt-2 flex items-center gap-1.5 type-body text-label-tertiary"
              role="status"
            >
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
