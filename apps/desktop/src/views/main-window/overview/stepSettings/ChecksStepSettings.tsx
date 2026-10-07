import { LoaderCircle } from "lucide-react"
import { useState, useSyncExternalStore } from "react"

import { Card } from "../../../../components/ui/Card"
import { Disclosure, DisclosureGroup } from "../../../../components/ui/Disclosure"
import { PushButton } from "../../../../components/ui/PushButton"
import { SegmentedControl } from "../../../../components/ui/SegmentedControl"
import { SectionGroup } from "../../../../components/ui/SectionGroup"
import { StatusText } from "../../../../components/ui/StatusText"
import { ToggleSwitch } from "../../../../components/ui/ToggleSwitch"
import {
  emptyCheckAvailability,
  getCheckAvailability,
  onCheckAvailabilityChanged,
  removeTypeSafeApiKey,
  runCheckBackfill,
  setCheckEnabled,
  setCheckHistoryDays,
  setTypeSafeApiKey,
  setSmartBurnChecksEnabled,
  type CheckAvailability,
  type CheckAvailabilityEvent,
} from "../../../../lib/checkAvailability"
import type { BurnCheckDetectorId } from "../../../../lib/insightsIpc"
import { CHECK_DEFINITIONS } from "../../../../lib/presentation/checkDefinitions"
import type { StepSettingsControlId } from "../../../../lib/stepSettingsTargets"
import { StepSettingsRow, StepSettingsToggleRow } from "./StepSettingsSearchRows"

/**
 * The Checks step's settings: Ignored Instructions, the check history
 * window, and the TypeSafe account.
 */

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
    return `Finished checking ${backfill.completed.toLocaleString()} ${backfill.completed === 1 ? "session" : "sessions"}`
  }
  const parts = [
    waiting > 0 ? `${waiting} waiting to be checked` : null,
    backfill.waitingForData > 0
      ? `${backfill.waitingForData} waiting for session analysis`
      : null,
    backfill.waitingForIdle > 0
      ? `${backfill.waitingForIdle} waiting for session to be idle`
      : null,
    backfill.completed > 0 ? `${backfill.completed} sessions checked` : null,
    backfill.skipped > 0 ? `${backfill.skipped} not eligible` : null,
    backfill.failed > 0 ? `${backfill.failed} failed` : null,
  ].filter((part): part is string => part !== null)
  return parts.length > 0 ? parts.join(" · ") : null
}

export function ChecksStepSettings() {
  const state = useSyncExternalStore(
    subscribe,
    () => snapshot,
    () => initial,
  )
  const [key, setKey] = useState("")
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
      setError("Could not start checks. Check the TypeSafe API key and try again.")
    } finally {
      setBusy(false)
    }
  }

  async function save() {
    setBusy(true)
    setError(null)
    try {
      publish(await setTypeSafeApiKey(key))
      setKey("")
    } catch {
      setError("Could not save the key in secure storage.")
    } finally {
      setBusy(false)
    }
  }

  async function remove() {
    setBusy(true)
    setError(null)
    try {
      publish(await removeTypeSafeApiKey())
    } catch {
      setError("Could not remove the key from secure storage.")
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
      setError(reason instanceof Error ? reason.message : "Could not update Smart Burn Checks.")
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

  const usage =
    state.usage.inputTokens > 0 || state.usage.confirmedCalls > 0
      ? `${state.usage.inputTokens.toLocaleString()} input tokens · ${state.usage.estimatedUsd ?? "cost unavailable"} estimated · ${state.usage.confirmedCalls.toLocaleString()} requests`
      : "No TypeSafe requests yet."
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
    const needsSetup = definition.kind === "smart" && !state.savedKey
    const isPaused = definition.kind === "smart" && state.savedKey && !state.configured
    const availability = needsSetup
      ? " Set up TypeSafe below before this check can run."
      : isPaused
        ? " Smart Burn Checks are paused."
        : ""
    return (
      <StepSettingsToggleRow
        key={id}
        searchId={searchId}
        label={definition.label}
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
        <Card>{smartChecks.map(renderCheck)}</Card>
      </SectionGroup>

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
              <StatusText tone="secondary">TypeSafe charges may apply.</StatusText>
            </div>
            {(progress || historyRunning || state.backfill.ready > 0) && (
              <div className="mt-3 space-y-1" role="status" aria-live="polite">
                {historyRunning && (
                  <StatusText icon={LoaderCircle} iconClassName="animate-spin" tone="secondary">
                    Checking sessions; {state.backfill.completed.toLocaleString()}/
                    {historyTotal.toLocaleString()} sessions checked so far
                  </StatusText>
                )}
                {!historyRunning && progress && (
                  <p className="type-footnote text-label-secondary">{progress}</p>
                )}
              </div>
            )}
          </StepSettingsRow>
        </Card>
      </SectionGroup>

      <SectionGroup title="TypeSafe account">
        <Card>
          <StepSettingsRow
            searchId="typeSafeApiKey"
            label="API key"
            description="Enter your TypeSafe API key to enable Smart Burn Checks. TypeSafe usage charges may apply."
          >
            <input
              id="typesafe-key"
              aria-label="TypeSafe API key"
              type="password"
              autoComplete="off"
              placeholder={state.savedKey ? "••••••••••••" : undefined}
              value={key}
              onChange={(event) => setKey(event.target.value)}
              className="mt-2 min-h-[var(--control-height-regular)] w-full rounded-control border border-separator bg-input-fill px-3 type-body text-label"
            />
            <div className="mt-2 flex flex-wrap items-center gap-2">
              {state.savedKey && (
                <ToggleSwitch
                  aria-label="Smart Burn Checks"
                  checked={state.configured}
                  disabled={busy || Boolean(state.error)}
                  onCheckedChange={(enabled) => void toggleChecks(enabled)}
                />
              )}
              <PushButton disabled={busy || !key.trim()} onClick={() => void save()}>
                {state.savedKey ? "Replace key" : "Save key and enable"}
              </PushButton>
              {(state.configured || state.savedKey) && (
                <PushButton disabled={busy} onClick={() => void remove()}>
                  Remove key
                </PushButton>
              )}
            </div>
            {(error || state.error) && (
              <p role="alert" className="mt-2 type-footnote text-system-red-text">
                {error || state.error}
              </p>
            )}
          </StepSettingsRow>
        </Card>
        <DisclosureGroup className="mt-2 px-1">
          <Disclosure label="Privacy and usage">
            <p>
              Ignored Instructions sends selected project instructions and these session fields
              to TypeSafe using your key: assistant messages, Bash command input (including
              inline scripts, heredocs, and patches), file edit paths, read file paths, search
              queries and explicit request constraints, and other tool input. User messages,
              tool output, dedicated edit bodies, and private thinking are excluded. Current
              global and project instruction snapshots and selected paths also leave this
              device. API usage can cost money; local totals count confirmed requests and
              tokens.
            </p>
            <p className="mt-2">{usage}</p>
            {state.usage.unknownOutcomes > 0 && (
              <p className="mt-1">
                {state.usage.unknownOutcomes.toLocaleString()} request outcomes are unknown and
                are not included in the estimate.
              </p>
            )}
          </Disclosure>
        </DisclosureGroup>
      </SectionGroup>
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
