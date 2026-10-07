import { LoaderCircle } from "lucide-react"
import { useState, useSyncExternalStore } from "react"

import { Card } from "../../components/ui/Card"
import { Pane } from "../../components/ui/Pane"
import { PushButton } from "../../components/ui/PushButton"
import { SegmentedControl } from "../../components/ui/SegmentedControl"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { StatusText } from "../../components/ui/StatusText"
import { ToggleSwitch } from "../../components/ui/ToggleSwitch"
import {
  emptyCheckAvailability,
  getCheckAvailability,
  onCheckAvailabilityChanged,
  runCheckBackfill,
  setCheckHistoryDays,
  setSmartBurnChecksEnabled,
  type CheckAvailability,
  type CheckAvailabilityEvent,
} from "../../lib/checkAvailability"
import { SettingsRow } from "./SettingsSearchRows"
import { CheckProviderSettings } from "./CheckProviderSettings"
import { providerErrorMessage } from "./ProviderSettingsSession"

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

export function ChecksPane({
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
    } catch (error) {
      setError(
        providerErrorMessage(
          error,
          "Could not update Smart Burn Checks. Check the active provider connection.",
        ),
      )
    } finally {
      setBusy(false)
    }
  }

  const usage =
    state.usage.inputTokens > 0 || state.usage.confirmedCalls > 0
      ? `${state.usage.inputTokens.toLocaleString()} input tokens · ${state.usage.estimatedUsd ?? "cost unavailable"} estimated · ${state.usage.confirmedCalls.toLocaleString()} requests`
      : "No model requests yet."
  const historyValue = String(state.historyDays)
  const progress = historyStatus(state)
  const historyRunning = state.backfill.queued + state.backfill.running > 0
  const historyTotal = state.backfill.total

  return (
    <Pane title="Checks">
      <div className="space-y-6">
        {refreshError && (
          <p role="alert" className="type-footnote text-system-red-text">
            {refreshError}
          </p>
        )}
        <SectionGroup title="Smart Burn Checks">
          <Card>
            <SettingsRow
              searchId="smartChecksEnabled"
              description="All Smart Burn Checks use the active model connection."
            >
              <ToggleSwitch
                aria-label="Smart Burn Checks"
                checked={state.configured}
                disabled={busy}
                onCheckedChange={(enabled) => void toggleChecks(enabled)}
              />
              {(error || state.error) && (
                <p role="alert" className="mt-2 type-footnote text-system-red-text">
                  {error || state.error}
                </p>
              )}
            </SettingsRow>
            <SettingsRow
              searchId="ignoredInstructions"
              label="Ignored Instructions"
              description="Find avoidable work after a session is idle. Checks start after 3 minutes of inactivity."
            />
            <p className="mt-2 px-3 pb-3 pt-2 type-footnote text-label-tertiary">
              More Smart Burn Checks coming soon.
            </p>
          </Card>
        </SectionGroup>

        <SectionGroup title="Past sessions">
          <Card>
            <SettingsRow
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
                    busy || historyRunning || !state.configured || state.historyDays === 0
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
                    <StatusText
                      icon={LoaderCircle}
                      iconClassName="animate-spin"
                      tone="secondary"
                    >
                      Running checks; {state.backfill.completed.toLocaleString()}/
                      {historyTotal.toLocaleString()} check jobs complete
                    </StatusText>
                  )}
                  {!historyRunning && progress && (
                    <p className="type-footnote text-label-secondary">{progress}</p>
                  )}
                </div>
              )}
            </SettingsRow>
          </Card>
        </SectionGroup>

        <CheckProviderSettings
          control={control}
          targetRevision={targetRevision}
          legacyKeySaved={state.savedKey}
        />
        <SectionGroup title="Model usage">
          <p className="type-footnote text-label-secondary">{usage}</p>
          {state.usage.unknownOutcomes > 0 && (
            <p className="mt-1 type-footnote text-label-secondary">
              {state.usage.unknownOutcomes.toLocaleString()} request outcomes are unknown and
              are not included in the estimate.
            </p>
          )}
        </SectionGroup>
      </div>
    </Pane>
  )
}
