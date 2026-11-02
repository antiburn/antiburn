import "../../../styles/burn-checks-report.css"

import { ChevronRight, Clock, Hourglass } from "lucide-react"
import { useCallback, useRef, useState, type KeyboardEvent, type MouseEvent } from "react"

import { pluralize } from "../../../lib/presentation/text"
import { BurnCheckFlame } from "../../../components/burn-checks/BurnCheckFlames"
import { CheckStatusText } from "../../../components/burn-checks/CheckStatusText"
import { checkRowStatus } from "../../../lib/presentation/checkRowStatus"
import { installedCheckDiagnostics } from "../../../lib/presentation/installedCheckDiagnostics"
import { BURN_CHECK_MARKS } from "../../../components/burn-checks/burnCheckMarks"
import { ScrollPane } from "../../../components/ui/ScrollPane"
import { Skeleton } from "../../../components/ui/Skeleton"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { agentIconName, GENERIC_AGENT_ICON } from "../../../lib/presentation/agents"
import { cn } from "../../../lib/cn"
import type {
  BurnCheckTargetPayload,
  ChecksCategoryPayload,
  ChecksReportPayload,
  CheckId,
} from "../../../lib/insightsIpc"
import { isInstalledCheckId } from "../../../lib/insightsIpc"
import {
  CHECK_LABELS,
  checksPresentation,
  formatTokenBurnPercent,
} from "../../../lib/presentation/checkReport"
import {
  formatSnoozeUntil,
  refreshSnoozedBurnChecks,
  snoozedCheckIds,
  useSnoozedBurnChecks,
  type SnoozedBurnCheck,
} from "../../../lib/snoozedBurnChecks"
import { checkRowPresentation } from "../../checks/checkPresentation"
import type { BurnChecksController, BurnChecksControllerSnapshot } from "./BurnChecksController"
import { BurnCheckCategoryIcon } from "./BurnCheckCategoryIcon"
import { BurnCheckDetail, CheckDetailActions, CHECK_SENTENCES } from "./BurnCheckDetail"
import { BurnCheckTargetDetail } from "./BurnCheckTargetDetail"
import { BurnChecksHeader } from "./BurnChecksHeader"
import { BurnChecksSavings } from "./BurnChecksSavings"
import { BurnCheckDetailBody, PassedCheckDetail } from "./BurnCheckDetailBody"
import { InstalledCheckDetailContent } from "./InstalledCheckDetailContent"
import {
  checkRows,
  isInstalledCheck,
  nativeCheckIds,
  type CheckRow,
} from "../../../lib/presentation/checkRows"
import { RemindLaterAction } from "./RemindLaterAction"

type ReportUiState = {
  reportKey: string
  searchRequest: string | null
  pendingSearch: boolean
  selectedId: CheckId | null
  deliberateIds: ReadonlySet<CheckId>
  passedPreference: boolean | null
}

function isUnusedResourceDetector(id: CheckId) {
  return id === "unusedMcpServers" || id === "unusedBuiltInTools" || id === "unusedSkills"
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

function TargetDetails({
  targets,
  refresh,
  evidenceActive = true,
}: {
  targets: BurnCheckTargetPayload[]
  refresh: () => void
  evidenceActive?: boolean
}) {
  return (
    <div className="burn-check-target-list">
      {targets.map((target) => (
        <BurnCheckTargetDetail
          key={target.findingId}
          target={target}
          refresh={refresh}
          reportRow
          evidenceActive={evidenceActive}
        />
      ))}
    </div>
  )
}

function TargetLoadError({ retry }: { retry: () => void }) {
  return (
    <article role="alert" className="rounded-control bg-surface-card/75 p-4">
      <p className="type-callout text-label-secondary">The check details did not load.</p>
      <button type="button" onClick={retry} className="burn-check-action mt-2 type-callout">
        Retry
      </button>
    </article>
  )
}

type CheckDetailContentProps = {
  check: ChecksCategoryPayload
  session: BurnChecksController
  state: BurnChecksControllerSnapshot
  evidenceActive: boolean
}

function CheckDetailContent({
  check,
  session,
  state,
  evidenceActive,
}: CheckDetailContentProps) {
  const targets = state.targets[check.id]
  return (
    <>
      {targets?.data && targets.error && (
        <TargetLoadError retry={() => session.loadTargets(check.id, true)} />
      )}
      <CheckDetailBody
        check={check}
        session={session}
        state={state}
        evidenceActive={evidenceActive}
      />
    </>
  )
}

function CheckDetailBody({ check, session, state, evidenceActive }: CheckDetailContentProps) {
  const targets = state.targets[check.id]
  if (check.lifecycle == null) {
    return (
      <p className="type-callout text-label-secondary">
        This check has no result in the current report.
      </p>
    )
  }
  if (check.lifecycle === "passing") {
    return (
      <PassedCheckDetail>
        {`No issue was found in ${check.clean} complete ${pluralize(check.clean, "session", "sessions")}.`}
      </PassedCheckDetail>
    )
  }
  if (!targets?.data) {
    return targets?.error ? (
      <TargetLoadError retry={() => session.loadTargets(check.id, true)} />
    ) : (
      <LoadingCheckDetail />
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
          active={evidenceActive}
        />
      )
    }
    return (
      <TargetDetails
        targets={targets.data.targets}
        refresh={session.refresh}
        evidenceActive={evidenceActive}
      />
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
      active={evidenceActive}
    />
  )
}

function CheckStateBadge({ snooze }: { snooze: SnoozedBurnCheck | undefined }) {
  if (!snooze) return null
  return (
    <span className="inline-flex shrink-0 items-center gap-1 rounded-full bg-surface-card px-2 py-1 type-footnote whitespace-nowrap text-label-secondary">
      <Clock size={12} className="shrink-0" aria-hidden="true" />
      {formatSnoozeUntil(snooze.until)}
    </span>
  )
}

function CheckDetail({
  check,
  visible,
  deliberate,
  snooze,
  session,
  state,
}: {
  check: CheckRow
  visible: boolean
  deliberate: boolean
  snooze: SnoozedBurnCheck | undefined
  session: BurnChecksController
  state: BurnChecksControllerSnapshot
}) {
  const presentation = checkRowPresentation(
    snooze && isInstalledCheck(check) ? { ...check, checking: false } : check,
    state.targets[check.id]?.data?.targets,
  )
  const targetList = state.targets[check.id]?.data
  const hasKnownFindings =
    check.finding > 0 ||
    (targetList?.targets.length ?? 0) > 0 ||
    state.aggregate?.wins.some((win) => win.detector === check.id)
  const shouldLoadTargets =
    check.lifecycle === "failing" ||
    check.lifecycle === "awaitingVerification" ||
    hasKnownFindings
  const named = isUnusedResourceDetector(check.id)
  const resourceNames: [string, string] =
    check.id === "unusedSkills"
      ? ["skill", "skills"]
      : check.id === "unusedMcpServers"
        ? ["tool server", "tool servers"]
        : ["tool", "tools"]
  const resourceCount =
    named && targetList
      ? `${targetList.truncated ? "At least " : ""}${targetList.targets.length} affected ${pluralize(targetList.targets.length, resourceNames[0], resourceNames[1])}`
      : null
  const showFindingActions = check.lifecycle === "failing" && check.finding > 0 && targetList
  const showPromptAction =
    targetList &&
    (showFindingActions ||
      (named && targetList.targets.length > 0) ||
      targetList.targets.some((target) => target.promptFix.status === "available"))
  const showSnoozedAction = snooze !== undefined || check.lifecycle === "awaitingVerification"
  const nativeDetector = isInstalledCheck(check) ? null : check.id
  const description = isInstalledCheck(check)
    ? check.installed.targets.some(
        (target) => target.observation?.trim() === check.installed.description?.trim(),
      )
      ? null
      : check.installed.description
    : check.id === "unusedMcpServers"
      ? "These tool servers loaded tools that were not used. Turn off servers you do not need."
      : check.id === "unusedSkills"
        ? "These skills added instructions that were not used. Load each skill only when the work needs it."
        : CHECK_SENTENCES[check.id]
  const trackVisibility = useCallback(
    (node: HTMLDivElement | null) => {
      if (nativeDetector === null) return
      session.setTargetsVisible(nativeDetector, node !== null && visible, deliberate)
    },
    [nativeDetector, deliberate, session, visible],
  )
  return (
    <div
      id={`burn-check-${check.id}-detail`}
      ref={shouldLoadTargets ? trackVisibility : undefined}
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
              </div>
              {showPromptAction && !isInstalledCheck(check) && (
                <div className="max-w-full shrink-0">
                  <CheckDetailActions
                    detector={check.id}
                    targets={targetList.targets}
                    refresh={session.refresh}
                    active={visible && state.active}
                    showFindingActions={Boolean(showFindingActions)}
                    reportRow
                  />
                </div>
              )}
              {(showSnoozedAction ||
                (isInstalledCheck(check) && check.lifecycle === "failing")) && (
                <div className="max-w-full shrink-0">
                  <RemindLaterAction detector={check.id} />
                </div>
              )}
            </div>
            <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
              <CheckStateBadge snooze={snooze} />
              <CheckMetadata
                check={check}
                presentation={presentation}
                resourceCount={resourceCount}
                snoozed={snooze !== undefined}
                inline
              />
            </div>
          </div>
          {check.lifecycle === "failing" && description && (
            <p className="w-full type-body text-pretty text-label-secondary">{description}</p>
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
          {(visible || isInstalledCheck(check)) &&
            (isInstalledCheck(check) ? (
              <InstalledCheckDetailContent
                check={check}
                active={visible && state.active}
                snoozed={snooze !== undefined}
              />
            ) : (
              <CheckDetailContent
                check={check}
                session={session}
                state={state}
                evidenceActive={visible && state.active}
              />
            ))}
        </div>
      </BurnCheckDetailBody>
    </div>
  )
}

function CheckMetadata({
  check,
  presentation,
  resourceCount,
  snoozed = false,
  inline = false,
  className,
}: {
  check: CheckRow
  presentation: ReturnType<typeof checkRowPresentation>
  resourceCount?: string | null
  snoozed?: boolean
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
      <span className="mt-0.5 block">
        <CheckStatusText status={checkRowStatus(check, snoozed)} />
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
  check: CheckRow
  selected: boolean
  bindRef: (node: HTMLButtonElement | null) => void
  onFocus: () => void
  onClick: () => void
  onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => void
  state: BurnChecksControllerSnapshot
  snoozeLabel?: string
}) {
  const presentation = checkRowPresentation(
    snoozeLabel && isInstalledCheck(check) ? { ...check, checking: false } : check,
    state.targets[check.id]?.data?.targets,
  )
  const summary = checkRowStatus(check, snoozeLabel !== undefined).text
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
          : check.lifecycle === "passing" && check.clean > 0
            ? "passed"
            : check.lifecycle === "failing"
              ? "failed"
              : "unassessed"
      }
      tabIndex={selected ? 0 : -1}
      onFocus={onFocus}
      onClick={onClick}
      onKeyDown={onKeyDown}
      className="burn-check-row-trigger overflow-hidden grid w-full grid-cols-[32px_minmax(0,1fr)] items-center gap-x-3 rounded-[var(--radius-popover)] px-3 py-3 text-left active:opacity-100"
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
        <CheckMetadata
          check={snoozeLabel && isInstalledCheck(check) ? { ...check, checking: false } : check}
          presentation={presentation}
          snoozed={snoozeLabel !== undefined}
        />
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
  focusedCheck?: CheckId | undefined
  focusRevision?: number | undefined
}) {
  const snoozeState = useSnoozedBurnChecks()
  const snoozed = snoozeState.records
  const snoozedIds = snoozedCheckIds(snoozed)
  const presentation = checksPresentation(report, false, snoozedIds)
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
    .map((check) => {
      const group = snoozedIds.has(check.id)
        ? "snoozed"
        : activeFailures.some((item) => item.id === check.id)
          ? "failed"
          : activeAwaiting.some((item) => item.id === check.id)
            ? "awaiting"
            : activeWins.some((item) => item.id === check.id)
              ? "passed"
              : "unassessed"
      return `${check.id}:${group}`
    })
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
    pendingSearch: false,
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
      pendingSearch:
        value.pendingSearch && !checks.some((check) => check.id === value.selectedId),
      selectedId:
        (value.pendingSearch && value.selectedId === focusedCheck) ||
        checks.some((check) => check.id === value.selectedId)
          ? value.selectedId
          : initialId,
      passedPreference: activeWins.some((check) => check.id === value.selectedId)
        ? true
        : value.passedPreference,
    }))
    if (ui.selectedId === focusedCheck && focusedCheck && snoozedIds.has(focusedCheck))
      setSnoozedOpen(true)
    if (unassessed.some((check) => check.id === ui.selectedId)) setUnassessedOpen(true)
  }
  const searchRequest = focusedCheck ? `${focusedCheck}:${focusRevision ?? 0}` : null
  if (focusedCheck && ui.searchRequest !== searchRequest) {
    setUi((value) => ({
      ...value,
      searchRequest,
      pendingSearch: !checks.some((check) => check.id === focusedCheck),
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
    ui.pendingSearch &&
    ui.selectedId === focusedCheck &&
    !checkRows(report).some((check) => check.id === focusedCheck)
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
  const rowRefs = useRef(new Map<CheckId, HTMLButtonElement>())
  const collectionRef = useRef<HTMLElement>(null)
  const focusedRow = useRef<HTMLButtonElement | null>(null)
  const focusedDetail = useRef<HTMLElement | null>(null)
  const passedTriggerRef = useRef<HTMLButtonElement>(null)
  const unassessedTriggerRef = useRef<HTMLButtonElement>(null)
  const snoozedTriggerRef = useRef<HTMLButtonElement>(null)
  const selectedCheck = checks.find((check) => check.id === selectedVisibleId)
  const selectedError =
    selectedCheck && isInstalledCheck(selectedCheck)
      ? installedCheckDiagnostics(selectedCheck.installed.diagnostics).find(
          (diagnostic) => diagnostic.severity === "error",
        )
      : undefined
  if (snoozeState.status === "loading")
    return (
      <p role="status" className="type-callout text-label-secondary">
        Loading checks…
      </p>
    )

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

  const selectCheck = (id: CheckId, deliberate = true) => {
    setUi((value) => ({
      ...value,
      pendingSearch: false,
      selectedId: id,
      deliberateIds: deliberate ? new Set([...value.deliberateIds, id]) : value.deliberateIds,
    }))
  }

  const handleRowKey = (event: KeyboardEvent<HTMLButtonElement>, check: CheckRow) => {
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

  const restoreCollectionFocus = (removed: HTMLElement) => {
    if (removed.isConnected) return
    const currentFocus = document.activeElement
    if (
      currentFocus !== document.body &&
      currentFocus?.isConnected &&
      !removed.contains(currentFocus)
    )
      return
    const rows = [...rowRefs.current.values()].filter((row) => !row.closest("[hidden]"))
    const target =
      rows.find((row) => row.getAttribute("aria-pressed") === "true") ??
      rows[0] ??
      collectionRef.current
    target?.focus({ preventScroll: true })
  }

  const renderCheck = (check: CheckRow) => {
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
              (lastFocus.current !== searchRequest || document.activeElement === document.body)
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
            else if (!replacement) restoreCollectionFocus(removed)
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
          ref={collectionRef}
          tabIndex={-1}
          className="main-window-collection burn-checks-collection"
          aria-label="Burn check collection"
          onBlurCapture={(event) => {
            if (event.target.isConnected && !event.currentTarget.contains(event.relatedTarget))
              focusedRow.current = null
          }}
        >
          <BurnChecksHeader report={report} />
          <ScrollPane
            className="min-h-0"
            topEdgeFade
            viewportClassName="burn-checks-collection-scroll"
          >
            <div className="burn-checks-collection-content">
              {snoozeState.status === "error" && (
                <div role="alert" className="type-callout text-label-secondary">
                  Could not update snoozes.
                  <button
                    type="button"
                    className="burn-check-action ml-2"
                    onClick={() => void refreshSnoozedBurnChecks()}
                  >
                    Retry snoozes
                  </button>
                </div>
              )}
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
                      Verifying
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
                passedDetectors={nativeCheckIds(activeWins)}
              />
              {presentation.noEnabledChecks && (
                <p className="type-body text-label-secondary">
                  No checks are turned on. Turn on a check in Settings to check sessions.
                </p>
              )}
              {presentation.noActiveChecks && !presentation.noEnabledChecks && (
                <p className="type-body text-label-secondary">
                  All checks are snoozed. Open Snoozed and unsnooze a check to include it in the
                  summary.
                </p>
              )}
            </div>
          </ScrollPane>
        </section>
        <section
          ref={(node) => {
            const removed = focusedDetail.current
            if (node && removed && !removed.isConnected) {
              focusedDetail.current = null
              queueMicrotask(() => restoreCollectionFocus(removed))
            }
          }}
          onFocusCapture={(event) => {
            focusedDetail.current = event.target.closest<HTMLElement>(".burn-check-detail")
          }}
          onBlurCapture={(event) => {
            if (event.target.isConnected && !event.currentTarget.contains(event.relatedTarget))
              focusedDetail.current = null
          }}
          className="main-window-detail burn-checks-detail-pane"
          aria-label="Burn check details"
        >
          <p role="status" aria-live="polite" aria-atomic="true" className="sr-only">
            {active &&
              selectedCheck &&
              `${checkRowPresentation(selectedCheck).label}: ${checkRowStatus(selectedCheck, snoozedIds.has(selectedCheck.id)).text}${selectedError ? `. ${selectedError.title}` : ""}`}
          </p>
          {unavailableSelected ? (
            <UnavailableSearchCheck check={focusedCheck} revision={focusRevision} />
          ) : selectedVisibleId == null ? (
            <p className="burn-checks-detail-content type-body text-label-secondary">
              {unassessed.length > 0
                ? "Open Not assessed to see checks without a current result."
                : "Select a check to see its result and explanation."}
            </p>
          ) : (
            checks.map((check) => (
              <CheckDetail
                key={check.id}
                check={check}
                visible={active && check.id === selectedVisibleId}
                deliberate={ui.deliberateIds.has(check.id)}
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
}: {
  check: CheckId
  revision: number | undefined
}) {
  const label = isInstalledCheckId(check)
    ? check.slice("installed:".length)
    : CHECK_LABELS[check]
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
      aria-label={label}
      className="mt-6 rounded-control border border-separator/40 bg-surface-card/50 p-4"
    >
      <h2 className="type-title-3 text-label">{label}</h2>
      <p className="mt-2 type-callout text-label-secondary">
        This check has no result in the current report.
      </p>
    </section>
  )
}
