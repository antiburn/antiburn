import { invoke } from "@tauri-apps/api/core"
import { listen } from "./tauriEvents"

import { hasShell } from "./ipc"
import type {
  BurnCheckDetectorId,
  InstalledCheckAvailability,
  InstalledCheckId,
} from "./insightsIpc"
import { CHECK_DEFINITIONS } from "./presentation/checkDefinitions"

export type CheckUsageSummary = {
  inputTokens: number
  outputTokens: number
  confirmedCalls: number
  cacheHits: number
  unknownOutcomes: number
  estimatedUsd: string | null
  lastUsedAtEpoch: number | null
}

export interface InstalledCheckAvailabilityPayload {
  effectiveId: InstalledCheckId
  name: string
  aliases: string[]
  availability: InstalledCheckAvailability
}

export type CheckAvailability = {
  revision: number
  yamlEnabled: boolean
  checks: Array<{ id: BurnCheckDetectorId; enabled: boolean }>
  installedChecks?: InstalledCheckAvailabilityPayload[]
  configured: boolean
  savedKey: boolean
  error: string | null
  usage: CheckUsageSummary
}

export type CheckAvailabilityEvent =
  { status: "updated"; snapshot: CheckAvailability } | { status: "failed" }

const emptyUsage: CheckUsageSummary = {
  inputTokens: 0,
  outputTokens: 0,
  confirmedCalls: 0,
  cacheHits: 0,
  unknownOutcomes: 0,
  estimatedUsd: "$0.00",
  lastUsedAtEpoch: null,
}

export const emptyCheckAvailability: CheckAvailability = {
  revision: 0,
  yamlEnabled: true,
  installedChecks: [],
  checks: Object.keys(CHECK_DEFINITIONS).map((id) => ({
    id: id as BurnCheckDetectorId,
    enabled: false,
  })),
  configured: false,
  savedKey: false,
  error: null,
  usage: emptyUsage,
}

export async function getCheckAvailability(): Promise<CheckAvailability> {
  if (!hasShell()) return emptyCheckAvailability
  const value = await invoke<CheckAvailability | null>("get_check_availability")
  if (
    !value ||
    typeof value.configured !== "boolean" ||
    typeof value.yamlEnabled !== "boolean" ||
    typeof value.revision !== "number" ||
    !Array.isArray(value.checks)
  ) {
    throw new Error("Could not read check status.")
  }
  return value
}

export async function setSmartBurnChecksEnabled(enabled: boolean): Promise<CheckAvailability> {
  return invoke<CheckAvailability>("set_smart_burn_checks_enabled", { enabled })
}

export async function setCheckEnabled(
  detector: BurnCheckDetectorId,
  enabled: boolean,
): Promise<CheckAvailability> {
  return invoke<CheckAvailability>("set_check_enabled", { detector, enabled })
}

export async function removeTypeSafeApiKey(): Promise<CheckAvailability> {
  return invoke<CheckAvailability>("remove_typesafe_api_key")
}

export function onCheckAvailabilityChanged(
  callback: (event: CheckAvailabilityEvent) => void,
): Promise<() => void> {
  if (!hasShell()) return Promise.resolve(() => undefined)
  return listen<CheckAvailabilityEvent>("checks:availability-changed", (event) =>
    callback(event.payload),
  )
}
