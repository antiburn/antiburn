import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"

import { hasShell } from "./ipc"

type CheckUsageSummary = {
  inputTokens: number
  outputTokens: number
  confirmedCalls: number
  cacheHits: number
  unknownOutcomes: number
  estimatedUsd: string | null
  lastUsedAtEpoch: number | null
}

export type CheckAvailability = {
  configured: boolean
  savedKey: boolean
  error: string | null
  usage: CheckUsageSummary
  historyDays: 0 | 7 | 30
  backfill: {
    total: number
    waitingForData: number
    waitingForIdle: number
    ready: number
    queued: number
    running: number
    completed: number
    skipped: number
    failed: number
  }
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
  configured: false,
  savedKey: false,
  error: null,
  usage: emptyUsage,
  historyDays: 0,
  backfill: {
    total: 0,
    waitingForData: 0,
    waitingForIdle: 0,
    ready: 0,
    queued: 0,
    running: 0,
    completed: 0,
    skipped: 0,
    failed: 0,
  },
}

export async function getCheckAvailability(): Promise<CheckAvailability> {
  if (!hasShell()) return emptyCheckAvailability
  const value = await invoke<CheckAvailability | null>("get_check_availability")
  if (!value || typeof value.configured !== "boolean") {
    throw new Error("Could not read check status.")
  }
  return value
}

export async function setTypeSafeApiKey(key?: string): Promise<CheckAvailability> {
  return invoke<CheckAvailability>("set_typesafe_api_key", { key: key || null })
}

export async function setSmartBurnChecksEnabled(enabled: boolean): Promise<CheckAvailability> {
  return invoke<CheckAvailability>("set_smart_burn_checks_enabled", { enabled })
}

export async function removeTypeSafeApiKey(): Promise<CheckAvailability> {
  return invoke<CheckAvailability>("remove_typesafe_api_key")
}

export async function setCheckHistoryDays(days: 0 | 7 | 30): Promise<CheckAvailability> {
  return invoke<CheckAvailability>("set_check_history_days", { days })
}

export async function runCheckBackfill(): Promise<{
  queued: number
  availability: CheckAvailability
}> {
  return invoke<{ queued: number; availability: CheckAvailability }>("run_check_backfill")
}

export function onCheckAvailabilityChanged(
  callback: (event: CheckAvailabilityEvent) => void,
): Promise<() => void> {
  if (!hasShell()) return Promise.resolve(() => undefined)
  return listen<CheckAvailabilityEvent>("checks:availability-changed", (event) =>
    callback(event.payload),
  )
}
