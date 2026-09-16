import { invoke } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"
import { useSyncExternalStore } from "react"

import type { BurnCheckDetectorId } from "./insightsIpc"
import { hasShell } from "./ipc"
import type { SessionHygieneCheck } from "./presentation/sessionHygiene"

const SNOOZES_CHANGED_EVENT = "checks:snoozes-changed"

export type SnoozeDuration = "week" | "month" | "forever"

export interface SnoozedBurnCheck {
  detector: BurnCheckDetectorId
  scope: "check"
  until: number | null
}

const sessionDetectorIds: Partial<Record<SessionHygieneCheck["id"], BurnCheckDetectorId>> = {
  sessionOverdepth: "sessionsOverDepth",
  modelOverthinking: "modelOverthinking",
  overpoweredSubagents: "overpoweredSubagents",
  obsoleteModel: "oldModelUsage",
  fastModeOveruse: "overuseOfFastMode",
  excessCacheRehydration: "cacheChurn",
}

export function snoozeUntil(duration: SnoozeDuration, from = new Date()): number | null {
  if (duration === "forever") return null
  const until = new Date(from)
  if (duration === "week") until.setDate(until.getDate() + 7)
  else {
    const day = until.getDate()
    until.setDate(1)
    until.setMonth(until.getMonth() + 1)
    const lastDay = new Date(until.getFullYear(), until.getMonth() + 1, 0).getDate()
    until.setDate(Math.min(day, lastDay))
  }
  return until.getTime()
}

let snapshot: SnoozedBurnCheck[] = []
const listeners = new Set<() => void>()
let expiryTimer: ReturnType<typeof setTimeout> | null = null

function scheduleExpiry(): void {
  if (expiryTimer) clearTimeout(expiryTimer)
  const next = snapshot
    .flatMap((snooze) => (snooze.until === null ? [] : [snooze.until]))
    .sort((left, right) => left - right)[0]
  if (next === undefined) return
  expiryTimer = setTimeout(() => void refresh(), Math.max(0, next - Date.now()))
}

function publish(next: SnoozedBurnCheck[]): void {
  snapshot = next
  scheduleExpiry()
  for (const listener of listeners) listener()
}

async function refresh(): Promise<void> {
  if (!hasShell()) return
  publish(await invoke<SnoozedBurnCheck[]>("list_burn_check_snoozes"))
}

export async function snoozeBurnCheck(
  detector: BurnCheckDetectorId,
  duration: SnoozeDuration,
): Promise<void> {
  const record: SnoozedBurnCheck = { detector, scope: "check", until: snoozeUntil(duration) }
  if (!hasShell()) return
  publish(await invoke<SnoozedBurnCheck[]>("set_burn_check_snooze", { snooze: record }))
}

export async function unsnoozeBurnCheck(detector: BurnCheckDetectorId): Promise<void> {
  if (!hasShell()) return
  publish(await invoke<SnoozedBurnCheck[]>("clear_burn_check_snooze", { detector }))
}

export function snoozedDetectorIds(
  records: readonly SnoozedBurnCheck[],
): ReadonlySet<BurnCheckDetectorId> {
  return new Set(records.map((record) => record.detector))
}

export function visibleSessionHygieneChecks(
  checks: SessionHygieneCheck[],
  snoozed: ReadonlySet<BurnCheckDetectorId>,
): SessionHygieneCheck[] {
  return checks.filter((check) => {
    const detector = sessionDetectorIds[check.id]
    return !detector || !snoozed.has(detector)
  })
}

export function formatSnoozeUntil(until: number | null): string {
  if (until === null) return "Snoozed forever"
  return `Snoozed until ${new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" }).format(until)}`
}

export function useSnoozedBurnChecks(): readonly SnoozedBurnCheck[] {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      void refresh()
      let stop: UnlistenFn | null = null
      if (hasShell())
        void listen<SnoozedBurnCheck[]>(SNOOZES_CHANGED_EVENT, (event) =>
          publish(event.payload),
        ).then((unlisten) => {
          stop = unlisten
        })
      return () => {
        listeners.delete(listener)
        stop?.()
        if (listeners.size === 0 && expiryTimer) {
          clearTimeout(expiryTimer)
          expiryTimer = null
        }
      }
    },
    () => snapshot,
    () => [],
  )
}
