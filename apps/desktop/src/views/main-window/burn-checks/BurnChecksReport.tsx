import "../../../styles/burn-checks-report.css"

import { ChevronRight, Clock, Hourglass } from "lucide-react"
import { useCallback, useRef, useState, type KeyboardEvent, type MouseEvent } from "react"

import { BurnCheckFlame } from "../../../components/burn-checks/BurnCheckFlames"
import { BURN_CHECK_MARKS } from "../../../components/burn-checks/burnCheckMarks"
import { ScrollPane } from "../../../components/ui/ScrollPane"
import { Skeleton } from "../../../components/ui/Skeleton"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { agentIconName, GENERIC_AGENT_ICON } from "../../../lib/presentation/agents"
import { cn } from "../../../lib/cn"
import { isCheckAvailable } from "../../../lib/presentation/checkDefinitions"
import { noteInteraction, smartCheckForDetector } from "../../../lib/ipc"
import type {
  BurnCheckTargetPayload,
  ChecksCategoryPayload,
  ChecksReportPayload,
} from "../../../lib/insightsIpc"
import {
  CHECK_LABELS,
  checksPresentation,
  formatTokenBurnPercent,
} from "../../../lib/presentation/checkReport"
import {
  formatSnoozeUntil,
  snoozedDetectorIds,
  useSnoozedBurnChecks,
  type SnoozedBurnCheck,
} from "../../../lib/snoozedBurnChecks"
import { checkRowPresentation as baseCheckRowPresentation } from "../../checks/checkPresentation"
import type { BurnChecksController, BurnChecksControllerSnapshot } from "./BurnChecksController"
import { BurnCheckCategoryIcon } from "./BurnCheckCategoryIcon"
import { BurnCheckDetail, CheckDetailActions, CHECK_SENTENCES } from "./BurnCheckDetail"
import {
  BurnCheckTargetDetail,
  EvidenceLimitsIcon,
  SmartCheckEvidenceSkeleton,
} from "./BurnCheckTargetDetail"
import { BurnChecksHeader } from "./BurnChecksHeader"
import { BurnChecksSavings } from "./BurnChecksSavings"
import { BurnCheckDetailBody } from "./BurnCheckDetailBody"
import { RemindLaterAction } from "./RemindLaterAction"

type ReportUiState = {
  reportKey: string
  searchRequest: string | null
  selectedId: ChecksCategoryPayload["id"] | null
  deliberateIds: ReadonlySet<ChecksCategoryPayload["id"]>
  passedPreference: boolean | null
}

function checkRowPresentation(
  check: ChecksCategoryPayload,
  targets?: readonly BurnCheckTargetPayload[],
) {
  const presentation = baseCheckRowPresentation(check, targets)
  const coverage = check.reviewCoverage
  const percent =
    coverage?.total != null && coverage.total > 0
      ? Math.round((coverage.reviewed / coverage.total) * 100)
      : null
  const reviewDetails = [
    coverage?.total === 0
      ? "No eligible review targets."
      : percent == null
        ? "The review percentage is unknown because the full target count is unavailable."
        : `${percent}% of review targets have been reviewed.`,
    "The review target is 50% of eligible targets. This is a sampling goal, not a confidence score or a guarantee that no issues remain.",
    "Reviewed targets have a terminal review answer, including uncertain answers. Unanswered targets have no terminal review answer. Pending completion is a reviewed answer that waits for task completion.",
  ]
  const coverageLabel = coverage
    ? [
        coverage.total == null
          ? `${coverage.reviewed} reviewed`
          : `${coverage.reviewed} of ${coverage.total} reviewed`,
        coverage.uncertain == null
          ? "uncertain count unknown"
          : `${coverage.uncertain} uncertain`,
        coverage.pending == null
          ? "unanswered count unknown"
          : `${coverage.pending} unanswered`,
        ...(coverage.pendingCompletion == null
          ? check.id === "ignoredInstructions"
            ? ["pending completion count unknown"]
            : []
          : coverage.pendingCompletion > 0
            ? [`${coverage.pendingCompletion} pending completion`]
            : []),
        ...(percent == null ? [] : [`${percent}% reviewed`]),
      ].join(" · ")
    : null
  return {
    ...presentation,
    coverage: coverageLabel,
    evidenceLimits: presentation.evidenceLimits.map((limit) => ({
      ...limit,
      details: [...limit.details, ...reviewDetails],
    })),
  }
}

function isUnusedResourceDetector(id: ChecksCategoryPayload["id"]) {
  return id === "unusedMcpServers" || id === "unusedBuiltInTools" || id === "unusedSkills"
}

function LoadingCheckDetail({ smart = false }: { smart?: boolean }) {
  return (
    <article
      role="region"
      aria-label="Loading finding details"
      aria-busy="true"
      className="rounded-control bg-surface-card/75 p-4"
    >
      {!smart && (
        <p role="status" className="sr-only">
          Loading finding details.
        </p>
      )}
      <Skeleton className="h-4 w-72 max-w-full" />
      {smart ? (
        <>
          <Skeleton className="mt-3 h-3 w-48 max-w-full" />
          <div className="mt-3 min-h-72">
            <SmartCheckEvidenceSkeleton />
          </div>
        </>
      ) : (
        <>
          <div className="mt-3 flex flex-wrap items-center gap-2">
            <Skeleton className="h-7 w-28" />
            <Skeleton className="h-7 w-12" />
          </div>
          <div className="mt-3">
            <Skeleton className="h-[17px] w-28" />
          </div>
        </>
      )}
    </article>
  )
}

function TargetDetails({
  targets,
  refresh,
  openEvidence = false,
}: {
  targets: BurnCheckTargetPayload[]
  refresh: () => void
  openEvidence?: boolean
}) {
  return (
    <div className="burn-check-target-list">
      {targets.map((target) => (
        <BurnCheckTargetDetail
          key={target.findingId}
          target={target}
          refresh={refresh}
          reportRow
          openEvidence={openEvidence}
        />
      ))}
    </div>
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
  session: BurnChecksController
  state: BurnChecksControllerSnapshot
}) {
  const targets = state.targets[check.id]
  if (check.lifecycle == null || checkRowPresentation(check).provisional) {
    return (
      <p className="type-callout text-label-secondary">
        {checkRowPresentation(check).provisional
          ? "No issues found yet."
          : "This check has not been assessed for the available sessions."}
      </p>
    )
  }
  if (check.lifecycle === "passing") {
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
          {check.sampled
            ? `No finding in the assessed sample across ${check.clean} ${check.clean === 1 ? "session" : "sessions"}. Unassessed work may remain.`
            : `No finding in ${check.clean} complete ${check.clean === 1 ? "session" : "sessions"}.`}
        </p>
      </div>
    )
  }
  if (!targets?.data) {
    return targets?.error ? (
      <TargetLoadError retry={() => session.loadTargets(check.id, true)} />
    ) : (
      <LoadingCheckDetail smart={smartCheckForDetector(check.id) !== undefined} />
    )
  }
  if (isUnusedResourceDetector(check.id)) {
    if (targets.data.targets.length === 0) {
      return (
        <BurnCheckDetail
          detector={check.id}
          targets={[]}
          samples={targets.data.samples}
          failedSessionCount={check.finding}
          refresh={session.refresh}
          contained
          reportRow
        />
      )
    }
    return <TargetDetails targets={targets.data.targets} refresh={session.refresh} />
  }
  if (
    (check.id === "ignoredInstructions" ||
      check.id === "skillOpportunities" ||
      check.id === "overExploring" ||
      check.id === "scopeCreep") &&
    targets.data.targets.length > 0
  ) {
    return (
      <TargetDetails targets={targets.data.targets} refresh={session.refresh} openEvidence />
    )
  }
  return (
    <BurnCheckDetail
      detector={check.id}
      targets={targets.data.targets}
      samples={targets.data.samples}
      failedSessionCount={check.finding}
      refresh={session.refresh}
      contained={targets.data.targets.length === 0}
      reportRow
    />
  )
}

function CheckStateBadge({
  awaiting,
  snooze,
}: {
  awaiting: boolean
  snooze: SnoozedBurnCheck | undefined
}) {
  if (!snooze && !awaiting) return null
  const Icon = snooze ? Clock : Hourglass
  return (
    <span className="inline-flex shrink-0 items-center gap-1 rounded-full bg-surface-card px-2 py-1 type-footnote whitespace-nowrap text-label-secondary">
      <Icon
        size={12}
        className={cn("shrink-0", !snooze && "text-system-orange")}
        aria-hidden="true"
      />
      {snooze ? formatSnoozeUntil(snooze.until) : "Awaiting verification"}
    </span>
  )
}

function CheckDetail({
  check,
  visible,
  deliberate,
  awaiting,
  snooze,
  session,
  state,
}: {
  check: ChecksCategoryPayload
  visible: boolean
  deliberate: boolean
  awaiting: boolean
  snooze: SnoozedBurnCheck | undefined
  session: BurnChecksController
  state: BurnChecksControllerSnapshot
}) {
  const presentation = checkRowPresentation(
    snooze ? { ...check, checking: false } : check,
    state.targets[check.id]?.data?.targets,
  )
  const targetList = state.targets[check.id]?.data
  const named = isUnusedResourceDetector(check.id)
  const resourceNames =
    check.id === "unusedSkills"
      ? ["skill", "skills"]
      : check.id === "unusedMcpServers"
        ? ["MCP server", "MCP servers"]
        : ["tool", "tools"]
  const resourceCount =
    named && targetList
      ? `${targetList.truncated ? "At least " : ""}${targetList.targets.length} affected ${resourceNames[targetList.targets.length === 1 ? 0 : 1]}`
      : null
  const showFindingActions = check.lifecycle === "failing" && check.finding > 0 && targetList
  const showSnoozedAction = snooze !== undefined || check.lifecycle === "awaitingVerification"
  const findingReported = useRef(false)
  const trackVisibility = useCallback(
    (node: HTMLDivElement | null) => {
      session.setTargetsVisible(check.id, node !== null && visible, deliberate)
      const smartCheck = smartCheckForDetector(check.id)
      if (
        node &&
        visible &&
        smartCheck &&
        check.lifecycle === "failing" &&
        check.finding > 0 &&
        deliberate &&
        !findingReported.current
      ) {
        findingReported.current = true
        noteInteraction({
          kind: "smartCheckObserved",
          check: smartCheck,
          observation: "finding_visible",
        })
      }
    },
    [check.id, check.finding, check.lifecycle, deliberate, session, visible],
  )
  return (
    <div
      id={`burn-check-${check.id}-detail`}
      ref={
        check.lifecycle === "failing" || check.lifecycle === "awaitingVerification"
          ? trackVisibility
          : undefined
      }
      hidden={!visible}
      tabIndex={-1}
      className="burn-check-detail min-w-0"
    >
      <header className="burn-check-detail-heading">
        <div className="burn-check-detail-heading-content">
          <div className="w-full min-w-0">
            <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
              <div className="flex min-w-0 flex-1 items-center gap-2">
                <h2 className="min-w-0 type-title-2 text-label text-balance">
                  {presentation.label}
                </h2>
                {presentation.evidenceLimits.map((limit) => (
                  <EvidenceLimitsIcon
                    key={limit.label}
                    label={limit.label}
                    details={limit.details}
                  />
                ))}
              </div>
              {showFindingActions && (
                <div className="max-w-full shrink-0">
                  <CheckDetailActions
                    detector={check.id}
                    targets={targetList.targets}
                    refresh={session.refresh}
                    reportRow
                  />
                </div>
              )}
              {showSnoozedAction && (
                <div className="max-w-full shrink-0">
                  <RemindLaterAction detector={check.id} />
                </div>
              )}
            </div>
            <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
              <CheckStateBadge awaiting={awaiting} snooze={snooze} />
              <CheckMetadata
                check={check}
                presentation={presentation}
                resourceCount={resourceCount}
                inline
              />
            </div>
          </div>
          {check.lifecycle === "failing" && (
            <p className="w-full type-body text-pretty text-label-secondary">
              {check.id === "unusedMcpServers"
                ? "These servers loaded tools that weren’t used. Disable each server where you don’t need it."
                : check.id === "unusedSkills"
                  ? "These skills added context that wasn’t used. Load each skill only where the work needs it."
                  : CHECK_SENTENCES[check.id]}
            </p>
          )}
        </div>
      </header>
      <BurnCheckDetailBody visible={visible}>
        <div
          className={cn(
            "burn-checks-detail-content",
            check.lifecycle === "failing" && "pt-[var(--space-lg)]",
          )}
        >
          {visible && (
            <CheckDetailContent
              check={snooze ? { ...check, checking: false } : check}
              session={session}
              state={state}
            />
          )}
        </div>
      </BurnCheckDetailBody>
    </div>
  )
}

function CheckMetadata({
  check,
  presentation,
  resourceCount,
  inline = false,
  className,
}: {
  check: ChecksCategoryPayload
  presentation: ReturnType<typeof checkRowPresentation>
  resourceCount?: string | null
  inline?: boolean
  className?: string
}) {
  return (
    <span
      className={cn(
        inline ? "flex flex-wrap items-center gap-x-3 gap-y-1" : "block",
        className,
      )}
    >
      <span className="mt-0.5 block font-mono type-footnote tabular-nums">
        {presentation.provisional ? (
          <CheckingText text="Checking" />
        ) : check.lifecycle === "passing" ? (
          <span className="text-burn-check-pass-fill">Passed</span>
        ) : (
          <>
            <span
              className={cn(
                check.finding > 0
                  ? "font-semibold! text-burn-check-failure-text"
                  : "text-label-secondary",
              )}
            >
              {check.finding} failed
            </span>
            <span className="mx-0.5 inline-block text-label-tertiary" aria-hidden="true">
              ·
            </span>
            <span
              className={
                check.finding > 0 ? "text-label-secondary" : "text-burn-check-pass-fill"
              }
            >
              {check.clean} passed
            </span>
            {presentation.checking && (
              <>
                <span className="mx-0.5 inline-block text-label-tertiary" aria-hidden="true">
                  ·
                </span>
                <CheckingText text={checkingLabel(check)} />
              </>
            )}
          </>
        )}
        {resourceCount && (
          <>
            <span className="mx-0.5 inline-block text-label-tertiary" aria-hidden="true">
              ·
            </span>
            <span className="text-label-tertiary">{resourceCount}</span>
          </>
        )}
      </span>
      {((check.lifecycle === "failing" &&
        check.finding > 0 &&
        check.estimatedTokenBurnBasisPoints != null) ||
        presentation.costLine) && (
        <span className="mt-1 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          {check.lifecycle === "failing" &&
            check.finding > 0 &&
            check.estimatedTokenBurnBasisPoints != null && (
              <span className="inline-flex items-center gap-1 type-footnote tabular-nums text-label-tertiary">
                <BurnCheckFlame basisPoints={check.estimatedTokenBurnBasisPoints} />
                {formatTokenBurnPercent(check.estimatedTokenBurnBasisPoints)} estimated burn
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

function checkingLabel(check: ChecksCategoryPayload) {
  return check.checkingCount == null ? "checking" : `${check.checkingCount} checking`
}

function CheckingText({ text }: { text: string }) {
  return (
    <span className="activity-row-active inline-block">
      <span className="activity-row-title-shimmer inline-block" data-text={text}>
        {text}
      </span>
    </span>
  )
}

function CheckTrigger({
  check,
  selected,
  bindRef,
  onFocus,
  onClick,
  onKeyDown,
  state,
  snoozeLabel,
}: {
  check: ChecksCategoryPayload
  selected: boolean
  bindRef: (node: HTMLButtonElement | null) => void
  onFocus: () => void
  onClick: () => void
  onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => void
  state: BurnChecksControllerSnapshot
  snoozeLabel?: string
}) {
  const presentation = checkRowPresentation(
    snoozeLabel ? { ...check, checking: false } : check,
    state.targets[check.id]?.data?.targets,
  )
  const summary = presentation.provisional
    ? "Checking"
    : check.lifecycle == null
      ? "Not assessed"
      : check.lifecycle === "awaitingVerification"
        ? "Awaiting verification"
        : check.lifecycle === "passing"
          ? "Passed"
          : `${check.finding} failed · ${check.clean} passed${presentation.checking ? ` · ${checkingLabel(check)}` : ""}`
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
      data-outcome={
        check.lifecycle === "awaitingVerification"
          ? "awaiting"
          : check.lifecycle === "passing"
            ? "passed"
            : check.lifecycle === "failing"
              ? "failed"
              : presentation.provisional
                ? "passed"
                : "unassessed"
      }
      tabIndex={selected ? 0 : -1}
      onFocus={onFocus}
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
      <BurnCheckCategoryIcon detector={check.id} />
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
  active,
  report,
  session,
  state,
  focusedCheck,
  focusRevision,
}: {
  active: boolean
  report: ChecksReportPayload
  session: BurnChecksController
  state: BurnChecksControllerSnapshot
  focusedCheck?: ChecksCategoryPayload["id"] | undefined
  focusRevision?: number | undefined
}) {
  const snoozeState = useSnoozedBurnChecks()
  const snoozed = snoozeState.records
  const snoozedIds = snoozedDetectorIds(snoozed)
  const availableReport = {
    ...report,
    categories: report.categories.filter((check) =>
      isCheckAvailable(check.id, report.smartChecksAvailable ?? true),
    ),
  }
  const presentation = checksPresentation(availableReport, false, snoozedIds)
  const PassIcon = BURN_CHECK_MARKS.clean.Icon
  const activeAwaiting = presentation.awaiting ?? []
  const activeFailures = presentation.failures
  const activeWins = presentation.wins
  const snoozedChecks = presentation.snoozed
  const unassessed = presentation.activeUnavailable
  const checks = [
    ...activeFailures,
    ...activeAwaiting,
    ...activeWins,
    ...unassessed,
    ...snoozedChecks,
  ]
  const reportKey = checks
    .map((check) => check.id)
    .sort()
    .join(":")
  const initialId =
    activeFailures[0]?.id ??
    activeAwaiting[0]?.id ??
    activeWins[0]?.id ??
    snoozedChecks[0]?.id ??
    null
  const [ui, setUi] = useState<ReportUiState>(() => ({
    reportKey,
    searchRequest: null,
    selectedId: initialId,
    deliberateIds: new Set(),
    passedPreference: null,
  }))
  const [snoozedOpen, setSnoozedOpen] = useState(false)
  const [unassessedOpen, setUnassessedOpen] = useState(false)
  if (active && ui.reportKey !== reportKey) {
    setUi((value) => ({
      ...value,
      reportKey,
      selectedId:
        value.selectedId === focusedCheck ||
        checks.some((check) => check.id === value.selectedId)
          ? value.selectedId
          : initialId,
      passedPreference:
        value.selectedId === focusedCheck &&
        activeWins.some((check) => check.id === focusedCheck)
          ? true
          : value.passedPreference,
    }))
    if (ui.selectedId === focusedCheck && focusedCheck && snoozedIds.has(focusedCheck))
      setSnoozedOpen(true)
    if (ui.selectedId === focusedCheck && unassessed.some((check) => check.id === focusedCheck))
      setUnassessedOpen(true)
  }
  const searchRequest = focusedCheck ? `${focusedCheck}:${focusRevision ?? 0}` : null
  if (focusedCheck && ui.searchRequest !== searchRequest) {
    setUi((value) => ({
      ...value,
      searchRequest,
      selectedId: focusedCheck,
      deliberateIds: new Set([...value.deliberateIds, focusedCheck]),
      passedPreference: activeWins.some((check) => check.id === focusedCheck)
        ? true
        : value.passedPreference,
    }))
    if (snoozedIds.has(focusedCheck)) setSnoozedOpen(true)
    if (unassessed.some((check) => check.id === focusedCheck)) setUnassessedOpen(true)
  }
  const lastFocus = useRef<string | null>(null)
  const passedOpen = ui.passedPreference ?? true
  const selectedId = checks.some((check) => check.id === ui.selectedId)
    ? ui.selectedId
    : initialId
  const visibleChecks = [
    ...activeFailures,
    ...activeAwaiting,
    ...(passedOpen ? activeWins : []),
    ...(unassessedOpen ? unassessed : []),
    ...(snoozedOpen ? snoozedChecks : []),
  ]
  const unavailableSelected =
    focusedCheck &&
    ui.selectedId === focusedCheck &&
    !availableReport.categories.some((check) => check.id === focusedCheck)
  const selectedVisibleId = unavailableSelected
    ? null
    : visibleChecks.some((check) => check.id === selectedId)
      ? selectedId
      : (activeFailures[0]?.id ??
        activeAwaiting[0]?.id ??
        (passedOpen ? activeWins[0]?.id : null) ??
        (unassessedOpen ? unassessed[0]?.id : null) ??
        (snoozedOpen ? snoozedChecks[0]?.id : null) ??
        null)
  const rowRefs = useRef(new Map<ChecksCategoryPayload["id"], HTMLButtonElement>())
  const focusedRow = useRef<HTMLButtonElement | null>(null)
  const passedTriggerRef = useRef<HTMLButtonElement>(null)
  const unassessedTriggerRef = useRef<HTMLButtonElement>(null)
  const snoozedTriggerRef = useRef<HTMLButtonElement>(null)
  if (snoozeState.status !== "ready") return null

  const togglePassed = (event: MouseEvent<HTMLButtonElement>) => {
    const trigger = event.currentTarget
    setUi((value) => {
      const nextPassedOpen = !passedOpen
      if (!nextPassedOpen) queueMicrotask(() => trigger.focus())
      return {
        ...value,
        passedPreference: nextPassedOpen,
        selectedId:
          !nextPassedOpen && activeWins.some((item) => item.id === value.selectedId)
            ? (activeFailures[0]?.id ?? activeAwaiting[0]?.id ?? null)
            : value.selectedId,
      }
    })
  }

  const toggleSnoozed = (event: MouseEvent<HTMLButtonElement>) => {
    const trigger = event.currentTarget
    const nextSnoozedOpen = !snoozedOpen
    setSnoozedOpen(nextSnoozedOpen)
    if (!nextSnoozedOpen) {
      setUi((value) => ({
        ...value,
        selectedId: snoozedChecks.some((item) => item.id === value.selectedId)
          ? (activeFailures[0]?.id ??
            activeAwaiting[0]?.id ??
            (passedOpen ? activeWins[0]?.id : null) ??
            null)
          : value.selectedId,
      }))
      queueMicrotask(() => trigger.focus())
    }
  }

  const toggleUnassessed = (event: MouseEvent<HTMLButtonElement>) => {
    const trigger = event.currentTarget
    const nextOpen = !unassessedOpen
    setUnassessedOpen(nextOpen)
    if (nextOpen && selectedVisibleId == null) {
      selectCheck(unassessed[0]!.id, false)
    } else if (!nextOpen) {
      setUi((value) => ({
        ...value,
        selectedId: unassessed.some((item) => item.id === value.selectedId)
          ? (activeFailures[0]?.id ??
            activeAwaiting[0]?.id ??
            (passedOpen ? activeWins[0]?.id : null) ??
            (snoozedOpen ? snoozedChecks[0]?.id : null) ??
            null)
          : value.selectedId,
      }))
      queueMicrotask(() => trigger.focus())
    }
  }

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
          if (node) {
            rowRefs.current.set(check.id, node)
            if (
              check.id === focusedCheck &&
              check.id === selectedVisibleId &&
              lastFocus.current !== searchRequest
            ) {
              lastFocus.current = searchRequest
              node.scrollIntoView({ block: "nearest" })
              node.focus({ preventScroll: true })
            }
            return
          }
          const removed = rowRefs.current.get(check.id)
          rowRefs.current.delete(check.id)
          if (!removed) return
          const activeElement = document.activeElement
          if (
            removed !== activeElement &&
            !(focusedRow.current === removed && activeElement === document.body)
          )
            return
          queueMicrotask(() => {
            const currentFocus = document.activeElement
            if (
              currentFocus !== removed &&
              currentFocus !== document.body &&
              currentFocus?.isConnected
            )
              return
            if (removed.isConnected) return
            const replacement = rowRefs.current.get(check.id)
            const hiddenGroup = replacement?.closest<HTMLElement>("[hidden]")
            if (hiddenGroup?.id === "burn-checks-snoozed-body")
              snoozedTriggerRef.current?.focus()
            else if (hiddenGroup?.id === "burn-checks-passed-body")
              passedTriggerRef.current?.focus()
            else if (hiddenGroup?.id === "burn-checks-unassessed-body")
              unassessedTriggerRef.current?.focus()
            else if (replacement && !hiddenGroup) replacement.focus({ preventScroll: true })
          })
        }}
        onClick={() => selectCheck(check.id)}
        onFocus={() => {
          focusedRow.current = rowRefs.current.get(check.id) ?? null
        }}
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
          onBlurCapture={(event) => {
            if (event.target.isConnected && !event.currentTarget.contains(event.relatedTarget))
              focusedRow.current = null
          }}
        >
          <BurnChecksHeader report={availableReport} />
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
              {activeAwaiting.length > 0 && (
                <section className="burn-checks-group" aria-labelledby="burn-checks-awaiting">
                  <h2 id="burn-checks-awaiting" className="flex items-center gap-2 px-1">
                    <Hourglass size={14} className="text-system-orange" aria-hidden="true" />
                    <span className="type-footnote font-medium! text-label-tertiary">
                      Awaiting verification
                    </span>{" "}
                    <span className="burn-check-group-count type-footnote tabular-nums text-label-tertiary">
                      {activeAwaiting.length}
                    </span>
                  </h2>
                  <div className="burn-checks-group-body">
                    {activeAwaiting.map((check) => renderCheck(check))}
                  </div>
                </section>
              )}
              {activeWins.length > 0 && (
                <section className="burn-checks-group" aria-labelledby="burn-checks-passed">
                  <h2>
                    <button
                      ref={passedTriggerRef}
                      id="burn-checks-passed"
                      type="button"
                      aria-expanded={passedOpen}
                      aria-controls="burn-checks-passed-body"
                      onClick={togglePassed}
                      onFocus={() => {
                        focusedRow.current = null
                      }}
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
              {unassessed.length > 0 && (
                <section className="burn-checks-group" aria-labelledby="burn-checks-unassessed">
                  <h2>
                    <button
                      ref={unassessedTriggerRef}
                      id="burn-checks-unassessed"
                      type="button"
                      aria-label={`Not assessed (${unassessed.length})`}
                      aria-expanded={unassessedOpen}
                      aria-controls="burn-checks-unassessed-body"
                      onClick={toggleUnassessed}
                      onKeyDown={(event) => {
                        if (event.key !== "ArrowDown") return
                        event.preventDefault()
                        if (!unassessedOpen) setUnassessedOpen(true)
                        selectCheck(unassessed[0]!.id)
                        queueMicrotask(() => rowRefs.current.get(unassessed[0]!.id)?.focus())
                      }}
                      onFocus={() => {
                        focusedRow.current = null
                      }}
                      className="burn-checks-passed-trigger flex w-full items-center gap-2 rounded-control px-1 text-left hover:text-label"
                    >
                      <span
                        className="inline-block size-3.5 shrink-0 rounded-full border-2 border-burn-check-neutral"
                        aria-hidden="true"
                      />
                      <span className="type-footnote font-medium! text-label-tertiary">
                        Not assessed
                      </span>
                      <span className="burn-check-group-count type-footnote tabular-nums text-label-tertiary">
                        {unassessed.length}
                      </span>
                      <ChevronRight
                        size={14}
                        className={cn(
                          "ml-auto text-label-tertiary",
                          unassessedOpen && "rotate-90",
                        )}
                        aria-hidden="true"
                      />
                    </button>
                  </h2>
                  <div
                    id="burn-checks-unassessed-body"
                    className="burn-checks-group-body"
                    hidden={!unassessedOpen}
                  >
                    {unassessed.map((check) => renderCheck(check))}
                  </div>
                </section>
              )}
              <section className="burn-checks-group" aria-labelledby="burn-checks-snoozed">
                <h2>
                  <button
                    ref={snoozedTriggerRef}
                    id="burn-checks-snoozed"
                    type="button"
                    aria-expanded={snoozedOpen}
                    aria-controls="burn-checks-snoozed-body"
                    onClick={toggleSnoozed}
                    onFocus={() => {
                      focusedRow.current = null
                    }}
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
              <BurnChecksSavings
                wins={state.aggregate?.wins ?? []}
                passedDetectors={new Set(activeWins.map((check) => check.id))}
              />
              {presentation.noEnabledChecks && (
                <p className="type-body text-label-secondary">No checks enabled.</p>
              )}
              {presentation.noActiveChecks && !presentation.noEnabledChecks && (
                <p className="type-body text-label-secondary">No active checks.</p>
              )}
            </div>
          </ScrollPane>
        </section>
        <section
          className="main-window-detail burn-checks-detail-pane"
          aria-label="Burn check details"
        >
          {unavailableSelected ? (
            <UnavailableSearchCheck
              check={focusedCheck}
              revision={focusRevision}
              available={isCheckAvailable(focusedCheck, report.smartChecksAvailable ?? true)}
            />
          ) : selectedVisibleId == null ? (
            <p className="burn-checks-detail-content type-body text-label-secondary">
              {unassessed.length > 0
                ? "Open Not assessed to inspect a check."
                : "Details will appear when a check has enough evidence."}
            </p>
          ) : (
            checks.map((check) => (
              <CheckDetail
                key={check.id}
                check={check}
                visible={active && check.id === selectedVisibleId}
                deliberate={ui.deliberateIds.has(check.id)}
                awaiting={check.lifecycle === "awaitingVerification"}
                snooze={snoozed.find((item) => item.detector === check.id)}
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

function UnavailableSearchCheck({
  check,
  revision,
  available,
}: {
  check: keyof typeof CHECK_LABELS
  revision: number | undefined
  available: boolean
}) {
  const lastFocus = useRef<string | null>(null)
  const focusTarget = useCallback(
    (node: HTMLElement | null) => {
      const key = `${check}:${revision ?? 0}`
      if (node && lastFocus.current !== key) {
        lastFocus.current = key
        node.scrollIntoView({ block: "center" })
        node.focus({ preventScroll: true })
      }
    },
    [check, revision],
  )
  return (
    <section
      ref={focusTarget}
      tabIndex={-1}
      aria-label={CHECK_LABELS[check]}
      className="mt-6 rounded-control border border-separator/40 bg-surface-card/50 p-4"
    >
      <h2 className="type-title-3 text-label">{CHECK_LABELS[check]}</h2>
      <p className="mt-2 type-callout text-label-secondary">
        {available
          ? "This check has not been assessed for the available sessions."
          : "Smart Burn Checks are unavailable."}
      </p>
    </section>
  )
}
