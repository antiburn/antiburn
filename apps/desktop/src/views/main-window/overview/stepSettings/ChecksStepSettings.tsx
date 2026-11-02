import { useRef, useState, useSyncExternalStore } from "react"

import { Card } from "../../../../components/ui/Card"
import { PushButton } from "../../../../components/ui/PushButton"
import { SectionGroup } from "../../../../components/ui/SectionGroup"
import {
  emptyCheckAvailability,
  getCheckAvailability,
  onCheckAvailabilityChanged,
  setCheckEnabled,
  setSmartBurnChecksEnabled,
  type CheckAvailability,
  type CheckAvailabilityEvent,
} from "../../../../lib/checkAvailability"
import type { BurnCheckDetectorId } from "../../../../lib/insightsIpc"
import { CHECK_DEFINITIONS } from "../../../../lib/presentation/checkDefinitions"
import { SettingsToggleRow } from "../../../settings/SettingsSearchRows"

import { CheckProviderSettings } from "../../../settings/CheckProviderSettings"
import { CheckFilesSettings } from "../../../settings/CheckFilesSettings"
import { CheckModelUsage, hasCurrentCheckUsage } from "../../../settings/CheckModelUsage"
import { providerErrorMessage } from "../../../settings/ProviderSettingsSession"

type ChecksSettingsSnapshot = {
  availability: CheckAvailability
  hasSnapshot: boolean
  refreshError: string | null
  subscriptionFailed: boolean
  refreshing: boolean
}

const initial: ChecksSettingsSnapshot = {
  availability: emptyCheckAvailability,
  hasSnapshot: false,
  refreshError: null,
  subscriptionFailed: false,
  refreshing: false,
}
let snapshot = initial
const listeners = new Set<() => void>()
let stop: (() => void) | undefined
let revision = 0
let generation = 0

function notify() {
  listeners.forEach((listener) => listener())
}

function publish(value: CheckAvailability) {
  if (value.revision < snapshot.availability.revision) return
  revision++
  snapshot = { ...snapshot, availability: value, hasSnapshot: true, refreshError: null }
  notify()
}

function publishRefreshError() {
  snapshot = {
    ...snapshot,
    refreshError: snapshot.hasSnapshot
      ? "Could not refresh check status. The displayed values may be out of date."
      : "Could not load check status. Try again.",
  }
  notify()
}

function publishAvailabilityEvent(event: CheckAvailabilityEvent) {
  if (event.status === "updated") publish(event.snapshot)
  else publishRefreshError()
}

async function refreshAvailability() {
  if (snapshot.refreshing) return
  const currentGeneration = generation
  const request = revision
  snapshot = { ...snapshot, refreshing: true }
  notify()
  try {
    if (!stop) {
      const unsubscribe = await onCheckAvailabilityChanged((event) => {
        if (generation === currentGeneration && listeners.size) publishAvailabilityEvent(event)
      })
      if (generation !== currentGeneration || !listeners.size) {
        unsubscribe()
        return
      }
      stop = unsubscribe
      snapshot = { ...snapshot, subscriptionFailed: false }
      notify()
    }
    const value = await getCheckAvailability()
    if (generation === currentGeneration && revision === request) publish(value)
  } catch {
    if (generation === currentGeneration) {
      if (!stop) {
        snapshot = { ...snapshot, subscriptionFailed: true }
        notify()
      } else if (revision === request) publishRefreshError()
    }
  } finally {
    if (generation === currentGeneration) {
      snapshot = { ...snapshot, refreshing: false }
      notify()
    }
  }
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  if (listeners.size === 1) {
    generation++
    void refreshAvailability()
  }
  return () => {
    listeners.delete(listener)
    if (!listeners.size) {
      generation++
      stop?.()
      stop = undefined
      snapshot = { ...snapshot, refreshing: false }
    }
  }
}

export function ChecksStepSettings({
  control,
  targetRevision,
}: {
  control?: string | null | undefined
  targetRevision?: number | undefined
}) {
  const loadState = useSyncExternalStore(
    subscribe,
    () => snapshot,
    () => initial,
  )
  const state = loadState.availability
  const usage = loadState.hasSnapshot ? state.usage : null
  const usageStale = Boolean(loadState.refreshError || loadState.subscriptionFailed)
  const usageSearchFallback =
    control === "checkModelUsage" &&
    !hasCurrentCheckUsage(usage, loadState.refreshing, usageStale)
  const [busy, setBusy] = useState(false)
  const [hasSelectedConnection, setHasSelectedConnection] = useState<boolean | null>(null)
  const [hasPendingConnectionOperation, setHasPendingConnectionOperation] = useState(false)
  const pending = useRef(new Set<BurnCheckDetectorId>())
  const [pendingChecks, setPendingChecks] = useState<ReadonlySet<BurnCheckDetectorId>>(
    new Set(),
  )
  const [checkErrors, setCheckErrors] = useState<Partial<Record<BurnCheckDetectorId, string>>>(
    {},
  )
  const [error, setError] = useState<string | null>(null)

  async function toggleChecks(enabled: boolean) {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      publish(await setSmartBurnChecksEnabled(enabled))
    } catch (reason) {
      setError(
        providerErrorMessage(
          reason,
          "Could not update model-based checks. Check the active connection.",
        ),
      )
    } finally {
      setBusy(false)
    }
  }

  async function toggleCheck(detector: BurnCheckDetectorId, enabled: boolean) {
    if (pending.current.has(detector)) return
    pending.current.add(detector)
    setPendingChecks(new Set(pending.current))
    setCheckErrors((errors) => ({ ...errors, [detector]: undefined }))
    try {
      publish(await setCheckEnabled(detector, enabled))
    } catch {
      setCheckErrors((errors) => ({
        ...errors,
        [detector]: `Could not ${enabled ? "enable" : "disable"} ${CHECK_DEFINITIONS[detector].label}. Try again.`,
      }))
    } finally {
      pending.current.delete(detector)
      setPendingChecks(new Set(pending.current))
    }
  }

  const enabledById = new Map(state.checks.map((check) => [check.id, check.enabled]))
  const localChecks = checkRows()
  const refreshError = loadState.subscriptionFailed
    ? loadState.hasSnapshot
      ? "Could not refresh check status. The displayed values may be out of date."
      : "Could not load check status. Try again."
    : loadState.refreshError

  function renderCheck(id: keyof typeof CHECK_SEARCH_IDS) {
    const definition = CHECK_DEFINITIONS[id]
    return (
      <SettingsToggleRow
        key={id}
        searchId={CHECK_SEARCH_IDS[id]}
        description={definition.description}
        checked={enabledById.get(id) === true}
        disabled={pendingChecks.has(id)}
        aria-busy={pendingChecks.has(id)}
        onChange={(enabled) => void toggleCheck(id, enabled)}
      />
    )
  }

  return (
    <div className="space-y-6">
      <SectionGroup title="Local checks">
        <Card>{localChecks.map(({ id }) => renderCheck(id))}</Card>
        {Object.entries(checkErrors).map(
          ([id, message]) =>
            message && (
              <p key={id} role="alert" className="mt-2 type-footnote text-system-red-text">
                {message}
              </p>
            ),
        )}
      </SectionGroup>

      <SectionGroup
        title="Model-based checks"
        data-settings-control={usageSearchFallback ? "checkModelUsage" : undefined}
        data-settings-focus-self={usageSearchFallback || undefined}
        tabIndex={usageSearchFallback ? -1 : undefined}
      >
        <Card>
          <div>
            <SettingsToggleRow
              searchId="smartChecksEnabled"
              description="Each check file defines which session data the check sends to your provider."
              checked={state.yamlEnabled}
              disabled={busy || (!state.yamlEnabled && !hasSelectedConnection)}
              aria-busy={busy}
              onChange={(enabled) => void toggleChecks(enabled)}
            />
            {hasSelectedConnection === false && !hasPendingConnectionOperation && (
              <p className="px-4 pb-3 type-footnote text-label-secondary">
                Add and select a connection before running checks.
              </p>
            )}
            {[...new Set([error, state.error])].map(
              (message) =>
                message && (
                  <p
                    key={message}
                    role="alert"
                    className="px-4 pb-3 type-footnote text-system-red-text"
                  >
                    {message}
                  </p>
                ),
            )}
          </div>
          <CheckProviderSettings
            control={control}
            targetRevision={targetRevision}
            legacyKeySaved={state.savedKey}
            onSelectionChange={setHasSelectedConnection}
            onPendingChange={setHasPendingConnectionOperation}
          />
          <CheckFilesSettings control={control} targetRevision={targetRevision} />
          <CheckModelUsage
            usage={usage}
            control={control}
            targetRevision={targetRevision}
            loading={loadState.refreshing}
            stale={usageStale}
          />
        </Card>
        <div className="mt-2 px-4">
          {refreshError && (
            <div className="space-y-2 pb-3">
              <p role="alert" className="type-footnote text-system-red-text">
                {refreshError}
              </p>
              <PushButton
                disabled={loadState.refreshing}
                onClick={() => void refreshAvailability()}
              >
                {loadState.refreshing ? "Loading check status…" : "Retry loading check status"}
              </PushButton>
            </div>
          )}
        </div>
      </SectionGroup>
    </div>
  )
}

const CHECK_SEARCH_IDS = {
  sessionsOverDepth: "sessionsOverDepthCheck",
  modelOverthinking: "modelOverthinkingCheck",
  overpoweredSubagents: "overpoweredSubagentsCheck",
  unusedMcpServers: "unusedMcpServersCheck",
  unusedBuiltInTools: "unusedBuiltInToolsCheck",
  unusedSkills: "unusedSkillsCheck",
  oldModelUsage: "oldModelUsageCheck",
  overuseOfFastMode: "overuseOfFastModeCheck",
  cacheChurn: "cacheChurnCheck",
} as const

function checkRows() {
  return (Object.keys(CHECK_SEARCH_IDS) as Array<keyof typeof CHECK_SEARCH_IDS>).map((id) => ({
    id,
    searchId: CHECK_SEARCH_IDS[id],
  }))
}
