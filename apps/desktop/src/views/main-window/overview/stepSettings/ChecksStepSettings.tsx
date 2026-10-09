import { LoaderCircle } from "lucide-react"
import { useState, useSyncExternalStore } from "react"

import { Card } from "../../../../components/ui/Card"
import { PushButton } from "../../../../components/ui/PushButton"
import { SegmentedControl } from "../../../../components/ui/SegmentedControl"
import { SectionGroup } from "../../../../components/ui/SectionGroup"
import { StatusText } from "../../../../components/ui/StatusText"
import {
  emptyCheckAvailability,
  getCheckAvailability,
  onCheckAvailabilityChanged,
  runCheckBackfill,
  setCheckEnabled,
  setCheckHistoryDays,
  setSmartBurnChecksEnabled,
  type CheckAvailability,
  type CheckAvailabilityEvent,
} from "../../../../lib/checkAvailability"
import type { BurnCheckDetectorId } from "../../../../lib/insightsIpc"
import { CHECK_DEFINITIONS } from "../../../../lib/presentation/checkDefinitions"
import type { StepSettingsControlId } from "../../../../lib/stepSettingsTargets"
import { StepSettingsRow, StepSettingsToggleRow } from "./StepSettingsSearchRows"
import { CheckProviderSettings } from "../../../settings/CheckProviderSettings"
import { providerErrorMessage } from "../../../settings/ProviderSettingsSession"

const initial = emptyCheckAvailability
let snapshot = initial
const listeners = new Set<() => void>()
let stop: (() => void) | undefined
let revision = 0
let generation = 0
let refreshError: string | null = null

function notify() {
  listeners.forEach((listener) => listener())
}

function publish(value: CheckAvailability) {
  if (value.revision < snapshot.revision) return
  revision++
  snapshot = value
  refreshError = null
  notify()
}

function publishRefreshError() {
  refreshError = "Could not refresh check status. The displayed values may be out of date."
  snapshot = { ...snapshot }
  notify()
}

function publishAvailabilityEvent(event: CheckAvailabilityEvent) {
  if (event.status === "updated") publish(event.snapshot)
  else publishRefreshError()
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  if (listeners.size === 1) {
    const currentGeneration = ++generation
    const current = ++revision
    void onCheckAvailabilityChanged(publishAvailabilityEvent)
      .then((unsubscribe) => {
        if (generation === currentGeneration && listeners.size) stop = unsubscribe
        else unsubscribe()
        if (!listeners.size) return
        const request = revision
        void getCheckAvailability()
          .then((value) => {
            if (revision === request) publish(value)
          })
          .catch(() => {
            if (revision === request) publishRefreshError()
          })
      })
      .catch(() => {
        if (revision === current) publishRefreshError()
      })
  }
  return () => {
    listeners.delete(listener)
    if (!listeners.size) {
      generation++
      stop?.()
      stop = undefined
    }
  }
}

function historyStatus(state: CheckAvailability): string | null {
  const { backfill } = state
  const waiting = backfill.ready + backfill.queued
  const finished =
    backfill.total > 0 &&
    backfill.completed === backfill.total &&
    backfill.running === 0 &&
    waiting === 0 &&
    backfill.waitingForData === 0 &&
    backfill.waitingForIdle === 0
  if (finished) {
    return `Finished ${backfill.completed.toLocaleString()} ${backfill.completed === 1 ? "check job" : "check jobs"}`
  }
  const parts = [
    waiting > 0 ? `${waiting} waiting to be checked` : null,
    backfill.waitingForData > 0
      ? `${backfill.waitingForData} waiting for session analysis`
      : null,
    backfill.waitingForIdle > 0
      ? `${backfill.waitingForIdle} waiting for session to be idle`
      : null,
    backfill.completed > 0 ? `${backfill.completed} check jobs complete` : null,
    backfill.skipped > 0 ? `${backfill.skipped} not eligible` : null,
    backfill.failed > 0 ? `${backfill.failed} failed` : null,
  ].filter((part): part is string => part !== null)
  return parts.length > 0 ? parts.join(" · ") : null
}

export function ChecksStepSettings({
  control,
  targetRevision,
}: {
  control?: string | null | undefined
  targetRevision?: number | undefined
}) {
  const state = useSyncExternalStore(
    subscribe,
    () => snapshot,
    () => initial,
  )
  const [busy, setBusy] = useState(false)
  const [pendingChecks, setPendingChecks] = useState<ReadonlySet<BurnCheckDetectorId>>(
    new Set(),
  )
  const [error, setError] = useState<string | null>(null)

  async function saveHistoryDays(days: 0 | 7 | 30) {
    setBusy(true)
    setError(null)
    try {
      publish(await setCheckHistoryDays(days))
    } catch {
      setError("Could not save the history period.")
    } finally {
      setBusy(false)
    }
  }

  async function runHistory() {
    setBusy(true)
    setError(null)
    try {
      const result = await runCheckBackfill()
      publish(result.availability)
    } catch {
      setError("Could not start checks. Check the active provider connection and try again.")
    } finally {
      setBusy(false)
    }
  }

  async function toggleChecks(enabled: boolean) {
    setBusy(true)
    setError(null)
    try {
      publish(await setSmartBurnChecksEnabled(enabled))
    } catch (reason) {
      setError(
        providerErrorMessage(
          reason,
          "Could not update Smart Burn Checks. Check the active provider connection.",
        ),
      )
    } finally {
      setBusy(false)
    }
  }

  async function toggleCheck(detector: BurnCheckDetectorId, enabled: boolean) {
    setPendingChecks((current) => new Set(current).add(detector))
    setError(null)
    try {
      publish(await setCheckEnabled(detector, enabled))
    } catch {
      setError(
        `Could not ${enabled ? "enable" : "disable"} ${CHECK_DEFINITIONS[detector].label}.`,
      )
    } finally {
      setPendingChecks((current) => {
        const next = new Set(current)
        next.delete(detector)
        return next
      })
    }
  }

  const historyValue = String(state.historyDays)
  const progress = historyStatus(state)
  const historyRunning = state.backfill.queued + state.backfill.running > 0
  const historyTotal = state.backfill.total
  const enabledById = new Map(state.checks.map((check) => [check.id, check.enabled]))
  const localChecks = checkRows("local")
  const smartChecks = checkRows("smart")
  const enabledSmartChecks = smartChecks.some(({ id }) => enabledById.get(id) === true)

  function renderCheck({
    id,
    searchId,
  }: {
    id: BurnCheckDetectorId
    searchId: StepSettingsControlId
  }) {
    const definition = CHECK_DEFINITIONS[id]
    const availability =
      definition.kind === "smart" && !state.configured
        ? " Enable Smart Burn Checks with an active provider connection below before this check can run."
        : ""
    return (
      <StepSettingsToggleRow
        key={id}
        searchId={searchId}
        description={`${definition.description}${availability}`}
        checked={enabledById.get(id) === true}
        disabled={pendingChecks.has(id)}
        onChange={(enabled) => void toggleCheck(id, enabled)}
      />
    )
  }

  return (
    <div className="space-y-6">
      {refreshError && (
        <p role="alert" className="type-footnote text-system-red-text">
          {refreshError}
        </p>
      )}
      <SectionGroup title="Local Checks">
        <Card>{localChecks.map(renderCheck)}</Card>
      </SectionGroup>

      <SectionGroup title="Smart Burn Checks">
        <Card>
          <StepSettingsToggleRow
            searchId="smartChecksEnabled"
            description="Run smart burn checks using the decision model below."
            checked={state.configured}
            disabled={busy}
            onChange={(enabled) => void toggleChecks(enabled)}
          />
          {smartChecks.map(renderCheck)}
        </Card>
      </SectionGroup>
      {(error || state.error) && (
        <p role="alert" className="type-footnote text-system-red-text">
          {error || state.error}
        </p>
      )}

      <SectionGroup title="Past sessions">
        <Card>
          <StepSettingsRow
            searchId="checkHistory"
            label="Check history"
            description="Choose a period. Checks start only when you ask."
          >
            <SegmentedControl
              ariaLabel="Check history window"
              value={historyValue}
              onChange={(value) => {
                if (value === "0") void saveHistoryDays(0)
                else if (value === "7") void saveHistoryDays(7)
                else if (value === "30") void saveHistoryDays(30)
              }}
              disabled={busy}
              equalWidth
              variant="native-tabs"
              className="mt-2 w-full"
              options={[
                { value: "0", label: "Future only" },
                { value: "7", label: "7 days" },
                { value: "30", label: "30 days" },
              ]}
            />
            <div className="mt-3 flex flex-wrap items-center gap-3">
              <PushButton
                variant="primary"
                disabled={
                  busy ||
                  historyRunning ||
                  !state.configured ||
                  !enabledSmartChecks ||
                  state.historyDays === 0
                }
                onClick={() => void runHistory()}
              >
                Check past sessions
              </PushButton>
              <StatusText tone="secondary">Provider charges may apply.</StatusText>
            </div>
            {(progress || historyRunning || state.backfill.ready > 0) && (
              <div className="mt-3 space-y-1" role="status" aria-live="polite">
                {historyRunning && (
                  <div>
                    <StatusText
                      icon={LoaderCircle}
                      iconClassName="animate-spin"
                      tone="secondary"
                    >
                      Reviewing past sessions · {state.backfill.completed.toLocaleString()} of{" "}
                      {historyTotal.toLocaleString()} session checks finished
                    </StatusText>
                    {state.backfill.reviewed > 0 && (
                      <p className="ml-5 type-footnote tabular-nums text-label-tertiary">
                        {state.backfill.reviewed.toLocaleString()} items reviewed so far
                      </p>
                    )}
                  </div>
                )}
                {!historyRunning && progress && (
                  <p className="type-footnote text-label-secondary">{progress}</p>
                )}
              </div>
            )}
          </StepSettingsRow>
        </Card>
      </SectionGroup>

      <CheckProviderSettings
        control={control}
        targetRevision={targetRevision}
        legacyKeySaved={state.savedKey}
        usage={state.usage}
      />
    </div>
  )
}

const CHECK_SEARCH_IDS: Record<BurnCheckDetectorId, StepSettingsControlId> = {
  sessionsOverDepth: "sessionsOverDepthCheck",
  modelOverthinking: "modelOverthinkingCheck",
  overpoweredSubagents: "overpoweredSubagentsCheck",
  unusedMcpServers: "unusedMcpServersCheck",
  unusedBuiltInTools: "unusedBuiltInToolsCheck",
  unusedSkills: "unusedSkillsCheck",
  oldModelUsage: "oldModelUsageCheck",
  overuseOfFastMode: "overuseOfFastModeCheck",
  cacheChurn: "cacheChurnCheck",
  ignoredInstructions: "ignoredInstructions",
  skillOpportunities: "skillOpportunitiesCheck",
  overExploring: "overExploringCheck",
  scopeCreep: "scopeCreepCheck",
}

function checkRows(kind: "local" | "smart") {
  return (
    Object.entries(CHECK_DEFINITIONS) as Array<
      [BurnCheckDetectorId, (typeof CHECK_DEFINITIONS)[BurnCheckDetectorId]]
    >
  )
    .filter(([, definition]) => definition.kind === kind)
    .map(([id]) => ({ id, searchId: CHECK_SEARCH_IDS[id] }))
}
