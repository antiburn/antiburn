import "../../../styles/burn-checks-report.css"

import { BellRing, ChevronRight, Clock } from "lucide-react"
import { useCallback, useRef, useState, type KeyboardEvent } from "react"

import { BurnCheckFlame } from "../../../components/burn-checks/BurnCheckFlames"
import { BURN_CHECK_MARKS } from "../../../components/burn-checks/burnCheckMarks"
import { ScrollPane } from "../../../components/ui/ScrollPane"
import { Skeleton } from "../../../components/ui/Skeleton"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { agentIconName, GENERIC_AGENT_ICON } from "../../../lib/presentation/agents"
import { cn } from "../../../lib/cn"
import type { ChecksCategoryPayload, ChecksReportPayload } from "../../../lib/insightsIpc"
import { isMacOS } from "../../../lib/platform"
import { checksPresentation, formatTokenBurnPercent } from "../../../lib/presentation/checks"
import {
  formatSnoozeUntil,
  snoozedDetectorIds,
  unsnoozeBurnCheck,
  useSnoozedBurnChecks,
} from "../../../lib/snoozedBurnChecks"
import { checkRowPresentation } from "../../checks/checkUi"
import type { BurnChecksSession, BurnChecksSnapshot } from "../BurnChecksSession"
import { BurnCheckDetail, CheckDetailActions, CHECK_SENTENCES } from "./BurnCheckDetail"
import { BurnCheckTargetDetail } from "./BurnCheckTargetDetail"
import { BurnChecksHeader } from "./BurnChecksHeader"
import { BurnChecksSavings } from "./BurnChecksSavings"
import { BurnCheckDetailBody } from "./BurnCheckDetailBody"

type ReportUiState = {
  reportKey: string
  selectedId: ChecksCategoryPayload["id"] | null
  deliberateIds: ReadonlySet<ChecksCategoryPayload["id"]>
  passedPreference: boolean | null
}

function LoadingCheckDetail() {
  return (
    <article
      role="region"
      aria-label="Loading finding details"
      aria-busy="true"
      className="rounded-control bg-surface-card/75 p-4"
    >
      <p role="status" className="sr-only">
        Loading finding details.
      </p>
      <Skeleton className="h-4 w-72 max-w-full" />
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Skeleton className="h-7 w-28" />
        <Skeleton className="h-7 w-12" />
      </div>
      <div className="mt-3">
        <Skeleton className="h-[17px] w-28" />
      </div>
    </article>
  )
}

function TargetLoadError({ retry }: { retry: () => void }) {
  return (
    <article role="alert" className="rounded-control bg-surface-card/75 p-4">
      <p className="type-callout text-label-secondary">Could not load this check's details.</p>
      <button type="button" onClick={retry} className="burn-check-action mt-2 type-callout">
        Retry
      </button>
    </article>
  )
}

function CheckDetailContent({
  check,
  session,
  state,
}: {
  check: ChecksCategoryPayload
  session: BurnChecksSession
  state: BurnChecksSnapshot
}) {
  const targets = state.targets[check.id]
  if (check.finding === 0) {
    const PassIcon = BURN_CHECK_MARKS.clean.Icon
    return (
      <div className="flex items-start gap-3 rounded-control bg-surface-card/75 p-4">
        <PassIcon
          size={14}
          strokeWidth={BURN_CHECK_MARKS.clean.strokeWidth}
          className={`mt-0.5 ${BURN_CHECK_MARKS.clean.iconClass}`}
          aria-hidden="true"
        />
        <p className="type-callout text-label-secondary">
          {`No finding in ${check.clean} complete sessions.`}
        </p>
      </div>
    )
  }
  if (!targets?.data) {
    return targets?.error ? (
      <TargetLoadError retry={() => session.loadTargets(check.id, true)} />
    ) : (
      <LoadingCheckDetail />
    )
  }
  if (check.id === "unusedSkills" || check.id === "unusedMcpServers") {
    if (targets.data.targets.length === 0) {
      return (
        <BurnCheckDetail
          detector={check.id}
          targets={[]}
          refresh={session.refresh}
          contained
          reportRow
        />
      )
    }
    return (
      <div className="burn-check-target-list">
        {targets.data.targets.map((target) => (
          <BurnCheckTargetDetail
            key={target.findingId}
            target={target}
            detector={check.id}
            refresh={session.refresh}
            reportRow
          />
        ))}
      </div>
    )
  }
  return (
    <BurnCheckDetail
      detector={check.id}
      targets={targets.data.targets}
      refresh={session.refresh}
      contained={targets.data.targets.length === 0}
      reportRow
    />
  )
}

function CheckDetail({
  check,
  visible,
  deliberate,
  snoozed,
  session,
  state,
}: {
  check: ChecksCategoryPayload
  visible: boolean
  deliberate: boolean
  snoozed: boolean
  session: BurnChecksSession
  state: BurnChecksSnapshot
}) {
  const presentation = checkRowPresentation(check, state.targets[check.id]?.data?.targets)
  const targetList = state.targets[check.id]?.data
  const named = check.id === "unusedSkills" || check.id === "unusedMcpServers"
  const resourceName = "affected resource"
  const resourceCount =
    named && targetList
      ? `${targetList.targets.length} ${resourceName}${targetList.targets.length === 1 ? "" : "s"}${targetList.truncated ? " shown" : ""}`
      : null
  const trackVisibility = useCallback(
    (node: HTMLDivElement | null) =>
      session.setTargetsVisible(check.id, node !== null, deliberate),
    [check.id, deliberate, session],
  )
  return (
    <div
      id={`burn-check-${check.id}-detail`}
      ref={visible && check.finding > 0 ? trackVisibility : undefined}
      hidden={!visible}
      tabIndex={-1}
      className="burn-check-detail min-w-0"
    >
      <header
        className="burn-check-detail-heading"
        data-tauri-drag-region={isMacOS() ? "deep" : undefined}
      >
        <div className="burn-check-detail-heading-content">
          <div className="min-w-0 flex-1 basis-56">
            <h2 className="type-title-2 text-label text-balance">{presentation.label}</h2>
            <p className="mt-1 type-footnote tabular-nums text-label-secondary">
              {check.finding} {check.finding === 1 ? "session" : "sessions"} affected
              {resourceCount && <span className="text-label-tertiary"> · {resourceCount}</span>}
            </p>
            <CheckMetadata check={check} presentation={presentation} inline />
            {check.finding > 0 && (
              <p className="mt-3 type-body text-pretty text-label-secondary">
                {check.id === "unusedMcpServers"
                  ? "These servers loaded tools that weren’t used. Disable each server where you don’t need it."
                  : check.id === "unusedSkills"
                    ? "These skills added context that wasn’t used. Load each skill only where the work needs it."
                    : CHECK_SENTENCES[check.id]}
              </p>
            )}
          </div>
          {check.finding > 0 && targetList && (!named || targetList.targets.length === 0) && (
            <CheckDetailActions
              detector={check.id}
              targets={targetList.targets}
              refresh={session.refresh}
              reportRow
              snoozed={snoozed}
            />
          )}
          {snoozed && (
            <button
              type="button"
              onClick={() => void unsnoozeBurnCheck(check.id)}
              className="burn-check-action type-callout gap-1"
            >
              <BellRing size={12} aria-hidden="true" />
              Unsnooze
            </button>
          )}
        </div>
      </header>
      <BurnCheckDetailBody visible={visible}>
        <div className="burn-checks-detail-content">
          <CheckDetailContent check={check} session={session} state={state} />
        </div>
      </BurnCheckDetailBody>
    </div>
  )
}

function CheckMetadata({
  check,
  presentation,
  inline = false,
}: {
  check: ChecksCategoryPayload
  presentation: ReturnType<typeof checkRowPresentation>
  inline?: boolean
}) {
  return (
    <span className={cn(inline ? "flex flex-wrap items-center gap-x-3 gap-y-1" : "block")}>
      <span className="mt-0.5 block font-mono type-footnote tabular-nums">
        <span
          className={cn(
            check.finding > 0
              ? "font-semibold! text-burn-check-failure-text"
              : "text-label-secondary",
          )}
        >
          {check.finding} failed
        </span>
        <span className="text-label-tertiary"> · </span>
        <span
          className={check.finding > 0 ? "text-label-secondary" : "text-burn-check-pass-fill"}
        >
          {check.clean} passed
        </span>
      </span>
      {(check.estimatedTokenBurnBasisPoints != null || presentation.costLine) && (
        <span className="mt-1 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          {check.estimatedTokenBurnBasisPoints != null && check.finding > 0 && (
            <span className="inline-flex items-center gap-1 type-footnote tabular-nums text-label-tertiary">
              <BurnCheckFlame basisPoints={check.estimatedTokenBurnBasisPoints} />
              {formatTokenBurnPercent(check.estimatedTokenBurnBasisPoints)} burn
            </span>
          )}
          {presentation.costLine && (
            <span className="type-footnote tabular-nums text-label-tertiary">
              {presentation.costLine} wasted
            </span>
          )}
        </span>
      )}
    </span>
  )
}

function CheckTrigger({
  check,
  selected,
  bindRef,
  onClick,
  onKeyDown,
  state,
  snoozeLabel,
}: {
  check: ChecksCategoryPayload
  selected: boolean
  bindRef: (node: HTMLButtonElement | null) => void
  onClick: () => void
  onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => void
  state: BurnChecksSnapshot
  snoozeLabel?: string
}) {
  const presentation = checkRowPresentation(check, state.targets[check.id]?.data?.targets)
  const { Icon } = presentation
  const summary = `${check.finding} failed · ${check.clean} passed`
  const metric = presentation.metric?.replace("<", "Under ").replace(" token", "")
  const agents = [
    ...new Map(
      (check.agents ?? [])
        .map((agent) => (agent === "claude" ? "claude-code" : agent))
        .filter((agent) => agentIconName(agent) !== GENERIC_AGENT_ICON)
        .map((agent) => [agentIconName(agent), agent]),
    ).values(),
  ]
  return (
    <button
      ref={bindRef}
      type="button"
      aria-pressed={selected}
      aria-controls={`burn-check-${check.id}-detail`}
      aria-label={`${presentation.label}, ${summary}${metric ? `, ${metric}` : ""}`}
      data-outcome={check.finding > 0 ? "failed" : "passed"}
      tabIndex={selected ? 0 : -1}
      onClick={onClick}
      onKeyDown={onKeyDown}
      className="burn-check-row-trigger overflow-hidden grid w-full grid-cols-[32px_minmax(0,1fr)] items-center gap-x-3 rounded-[var(--radius-popover)] px-3 py-3 text-left active:transform-none active:opacity-100"
    >
      {agents.length > 0 && (
        <span
          aria-hidden="true"
          data-check-vendor-watermarks=""
          className="session-vendor-watermark burn-check-vendor-watermarks pointer-events-none absolute -right-1.5 -bottom-1.5 z-0 flex gap-1"
        >
          {agents.map((agent) => (
            <span
              key={agentIconName(agent)}
              className="inline-flex h-10 w-10 items-center justify-center"
            >
              {renderAgentIcon(agent, 40, undefined, "neutral")}
            </span>
          ))}
        </span>
      )}
      <span className="relative z-10 grid h-8 w-8 place-items-center rounded-full bg-surface-card text-label-secondary">
        <Icon size={15} strokeWidth={2} aria-hidden="true" />
      </span>
      <span className="relative z-10 min-w-0">
        <span className="block wrap-anywhere type-body font-medium! text-label">
          {presentation.label}
        </span>
        <CheckMetadata check={check} presentation={presentation} />
        {snoozeLabel && (
          <span className="mt-1 flex items-center gap-1 type-footnote text-label-secondary">
            <Clock size={13} className="text-label-secondary" aria-hidden="true" />
            {snoozeLabel}
          </span>
        )}
      </span>
    </button>
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
  const snoozed = useSnoozedBurnChecks()
  const snoozedIds = snoozedDetectorIds(snoozed)
  const PassIcon = BURN_CHECK_MARKS.clean.Icon
  const activeFailures = presentation.failures.filter((check) => !snoozedIds.has(check.id))
  const activeWins = presentation.wins.filter((check) => !snoozedIds.has(check.id))
  const snoozedChecks = [...presentation.failures, ...presentation.wins].filter((check) =>
    snoozedIds.has(check.id),
  )
  const checks = [...activeFailures, ...activeWins, ...snoozedChecks]
  const reportKey = checks.map((check) => check.id).join(":")
  const initialId = activeFailures[0]?.id ?? activeWins[0]?.id ?? snoozedChecks[0]?.id ?? null
  const [ui, setUi] = useState<ReportUiState>(() => ({
    reportKey,
    selectedId: initialId,
    deliberateIds: new Set(),
    passedPreference: null,
  }))
  if (ui.reportKey !== reportKey) {
    setUi((value) => ({
      ...value,
      reportKey,
      selectedId: checks.some((check) => check.id === value.selectedId)
        ? value.selectedId
        : initialId,
    }))
  }
  const [snoozedOpen, setSnoozedOpen] = useState(false)
  const passedOpen = ui.passedPreference ?? activeFailures.length === 0
  const selectedId = checks.some((check) => check.id === ui.selectedId)
    ? ui.selectedId
    : initialId
  const visibleChecks = [
    ...activeFailures,
    ...(passedOpen ? activeWins : []),
    ...(snoozedOpen ? snoozedChecks : []),
  ]
  const selectedVisibleId = visibleChecks.some((check) => check.id === selectedId)
    ? selectedId
    : (activeFailures[0]?.id ??
      (passedOpen ? activeWins[0]?.id : null) ??
      (snoozedOpen ? snoozedChecks[0]?.id : null) ??
      null)
  const rowRefs = useRef(new Map<ChecksCategoryPayload["id"], HTMLButtonElement>())

  const selectCheck = (id: ChecksCategoryPayload["id"], deliberate = true) => {
    setUi((value) => ({
      ...value,
      selectedId: id,
      deliberateIds: deliberate ? new Set([...value.deliberateIds, id]) : value.deliberateIds,
    }))
  }

  const handleRowKey = (
    event: KeyboardEvent<HTMLButtonElement>,
    check: ChecksCategoryPayload,
  ) => {
    const index = visibleChecks.findIndex((item) => item.id === check.id)
    let nextIndex: number
    if (event.key === "ArrowDown") nextIndex = Math.min(index + 1, visibleChecks.length - 1)
    else if (event.key === "ArrowUp") nextIndex = Math.max(index - 1, 0)
    else if (event.key === "Home") nextIndex = 0
    else if (event.key === "End") nextIndex = visibleChecks.length - 1
    else if (event.key === "Enter") {
      event.preventDefault()
      selectCheck(check.id)
      queueMicrotask(() => document.getElementById(`burn-check-${check.id}-detail`)?.focus())
      return
    } else return
    event.preventDefault()
    const next = visibleChecks[nextIndex]
    if (!next) return
    selectCheck(next.id)
    rowRefs.current.get(next.id)?.focus()
  }

  const renderCheck = (check: ChecksCategoryPayload) => {
    const snooze = snoozed.find((item) => item.detector === check.id)
    return (
      <CheckTrigger
        key={check.id}
        check={check}
        selected={check.id === selectedVisibleId}
        state={state}
        bindRef={(node) => {
          if (node) rowRefs.current.set(check.id, node)
          else rowRefs.current.delete(check.id)
        }}
        onClick={() => selectCheck(check.id)}
        onKeyDown={(event) => handleRowKey(event, check)}
        {...(snooze ? { snoozeLabel: formatSnoozeUntil(snooze.until) } : {})}
      />
    )
  }

  return (
    <div className="burn-checks-report">
      <div className="burn-checks-layout">
        <section
          className="main-window-collection burn-checks-collection"
          aria-label="Burn check collection"
        >
          <BurnChecksHeader report={report} />
          <ScrollPane
            className="min-h-0"
            topEdgeFade
            viewportClassName="burn-checks-collection-scroll"
          >
            <div className="burn-checks-collection-content">
              {activeFailures.length > 0 && (
                <section className="burn-checks-group" aria-labelledby="burn-checks-failed">
                  <div className="burn-checks-group-body">
                    {activeFailures.map((check) => renderCheck(check))}
                  </div>
                </section>
              )}
              {activeWins.length > 0 && (
                <section className="burn-checks-group" aria-labelledby="burn-checks-passed">
                  <h2>
                    <button
                      id="burn-checks-passed"
                      type="button"
                      aria-expanded={passedOpen}
                      aria-controls="burn-checks-passed-body"
                      onClick={() =>
                        setUi((value) => {
                          const nextPassedOpen = !passedOpen
                          const nextSelected =
                            !nextPassedOpen &&
                            activeWins.some((item) => item.id === value.selectedId)
                              ? (activeFailures[0]?.id ?? null)
                              : value.selectedId
                          return {
                            ...value,
                            passedPreference: nextPassedOpen,
                            selectedId: nextSelected,
                          }
                        })
                      }
                      className="burn-checks-passed-trigger flex w-full items-center gap-2 rounded-control px-1 text-left hover:text-label"
                    >
                      <PassIcon
                        size={14}
                        strokeWidth={BURN_CHECK_MARKS.clean.strokeWidth}
                        className="text-burn-check-pass-fill"
                        aria-hidden="true"
                      />
                      <span className="type-footnote font-medium! text-label-tertiary">
                        Passed checks
                      </span>{" "}
                      <span className="burn-check-group-count type-footnote tabular-nums text-label-tertiary">
                        {activeWins.length}
                      </span>
                      <ChevronRight
                        size={14}
                        className={cn(
                          "ml-auto text-label-tertiary ",
                          passedOpen && "rotate-90",
                        )}
                        aria-hidden="true"
                      />
                    </button>
                  </h2>
                  <div
                    id="burn-checks-passed-body"
                    className="burn-checks-group-body"
                    hidden={!passedOpen}
                  >
                    {activeWins.map((check) => renderCheck(check))}
                  </div>
                </section>
              )}
              <section className="burn-checks-group" aria-labelledby="burn-checks-snoozed">
                <h2>
                  <button
                    id="burn-checks-snoozed"
                    type="button"
                    aria-expanded={snoozedOpen}
                    aria-controls="burn-checks-snoozed-body"
                    onClick={() => setSnoozedOpen((open) => !open)}
                    className="burn-checks-passed-trigger flex w-full items-center gap-2 rounded-control px-1 text-left hover:text-label"
                  >
                    <Clock size={14} className="text-label-tertiary" aria-hidden="true" />
                    <span className="type-footnote font-medium! text-label-tertiary">
                      Snoozed
                    </span>{" "}
                    <span className="burn-check-group-count type-footnote tabular-nums text-label-tertiary">
                      {snoozedChecks.length}
                    </span>
                    <ChevronRight
                      size={14}
                      className={cn("ml-auto text-label-tertiary", snoozedOpen && "rotate-90")}
                      aria-hidden="true"
                    />
                  </button>
                </h2>
                <div
                  id="burn-checks-snoozed-body"
                  hidden={!snoozedOpen}
                  className="burn-checks-group-body"
                >
                  {snoozedChecks.map((check) => (
                    <div key={check.id} className="animate-burn-check-snooze-in">
                      {renderCheck(check)}
                    </div>
                  ))}
                </div>
              </section>
              <BurnChecksSavings wins={state.aggregate?.wins ?? []} />
              {checks.length === 0 && (
                <p className="type-body text-label-secondary">No assessed checks yet.</p>
              )}
            </div>
          </ScrollPane>
        </section>
        <section
          className="main-window-detail burn-checks-detail-pane"
          aria-label="Burn check details"
        >
          {checks.length === 0 ? (
            <p className="burn-checks-detail-content type-body text-label-secondary">
              Details will appear when a check has enough evidence.
            </p>
          ) : (
            checks.map((check) => (
              <CheckDetail
                key={check.id}
                check={check}
                visible={check.id === selectedVisibleId}
                deliberate={ui.deliberateIds.has(check.id)}
                snoozed={snoozedIds.has(check.id)}
                session={session}
                state={state}
              />
            ))
          )}
        </section>
      </div>
    </div>
  )
}
