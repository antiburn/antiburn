import { useRef, useState, useSyncExternalStore } from "react"

import { Card } from "../../../../components/ui/Card"
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
  const pending = useRef(new Set<BurnCheckDetectorId>())
  const [pendingChecks, setPendingChecks] = useState<ReadonlySet<BurnCheckDetectorId>>(
    new Set(),
  )
  const [checkErrors, setCheckErrors] = useState<Partial<Record<BurnCheckDetectorId, string>>>(
    {},
  )
  const [error, setError] = useState<string | null>(null)

  async function toggleChecks(enabled: boolean) {
    setBusy(true)
    setError(null)
    try {
      publish(await setSmartBurnChecksEnabled(enabled))
    } catch (reason) {
      setError(
        providerErrorMessage(
          reason,
          "Could not update Model-based Checks. Check the Jev connection.",
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
      {refreshError && (
        <p role="alert" className="type-footnote text-system-red-text">
          {refreshError}
        </p>
      )}
      <SectionGroup title="Local Checks">
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

      <SectionGroup title="Model-based Checks">
        <Card>
          <SettingsToggleRow
            searchId="smartChecksEnabled"
            description="Run installed checks on new session activity."
            checked={state.yamlEnabled}
            disabled={busy}
            aria-busy={busy}
            onChange={(enabled) => void toggleChecks(enabled)}
          />
        </Card>
        <p className="mt-2 type-footnote text-label-secondary">
          {state.installedChecks?.length ?? 0} installed{" "}
          {state.installedChecks?.length === 1 ? "check" : "checks"}. To install a check, add
          its YAML file to <code>.antiburn/checks/</code> in a project or{" "}
          <code>~/.antiburn/checks/</code>.
        </p>
      </SectionGroup>
      {(error || state.error) && (
        <p role="alert" className="type-footnote text-system-red-text">
          {error || state.error}
        </p>
      )}

      <CheckProviderSettings
        control={control}
        targetRevision={targetRevision}
        legacyKeySaved={state.savedKey}
        usage={state.usage}
      />
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
