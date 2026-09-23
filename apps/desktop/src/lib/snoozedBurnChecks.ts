import { invoke } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"
import { useSyncExternalStore } from "react"

import type {
  BurnCheckDetectorId,
  ChecksCategoryPayload,
  ChecksReportPayload,
} from "./insightsIpc"
import { hasShell } from "./ipc"
import type { SessionHygieneCheck } from "./presentation/sessionHygiene"
import type { UnusedContextRow } from "./presentation/unusedContext"

const SNOOZES_CHANGED_EVENT = "checks:snoozes-changed"

export type SnoozeDuration = "week" | "month" | "forever"

export interface SnoozedBurnCheck {
  detector: BurnCheckDetectorId
  scope: "check"
  until: number | null
}

export interface SnoozedBurnChecksState {
  status: "loading" | "ready" | "error"
  records: readonly SnoozedBurnCheck[]
}

const sessionDetectorIds: Partial<Record<SessionHygieneCheck["id"], BurnCheckDetectorId>> = {
  sessionOverdepth: "sessionsOverDepth",
  modelOverthinking: "modelOverthinking",
  overpoweredSubagents: "overpoweredSubagents",
  obsoleteModel: "oldModelUsage",
  fastModeOveruse: "overuseOfFastMode",
  excessCacheRehydration: "cacheChurn",
}

const detectorIds: readonly BurnCheckDetectorId[] = [
  "sessionsOverDepth",
  "modelOverthinking",
  "overpoweredSubagents",
  "unusedMcpServers",
  "unusedBuiltInTools",
  "unusedSkills",
  "oldModelUsage",
  "overuseOfFastMode",
  "cacheChurn",
]

/** The bit mask of `detectors` that indexes
 *  `estimatedTokenBurnBasisPointsByDetectorMask`. */
export function detectorMask(detectors: ReadonlySet<BurnCheckDetectorId>): number {
  return detectorIds.reduce(
    (value, detector, index) => value | (detectors.has(detector) ? 1 << index : 0),
    0,
  )
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

let snapshot: SnoozedBurnChecksState = { status: "loading", records: [] }
const listeners = new Set<() => void>()
let expiryTimer: ReturnType<typeof setTimeout> | null = null
let eventListenerStarted = false
let stopEventListener: UnlistenFn | null = null
let revision = 0
let listenerGeneration = 0
const serverSnapshot: SnoozedBurnChecksState = { status: "loading", records: [] }

function scheduleExpiry(): void {
  if (expiryTimer) clearTimeout(expiryTimer)
  expiryTimer = null
  const next = snapshot.records
    .flatMap((snooze) => (snooze.until === null ? [] : [snooze.until]))
    .sort((left, right) => left - right)[0]
  if (next === undefined) return
  expiryTimer = setTimeout(
    () => {
      if (!hasShell()) {
        publish(snapshot.records)
        return
      }
      void refreshSnoozedBurnChecks()
    },
    Math.max(0, next - Date.now()),
  )
}

function publish(next: readonly SnoozedBurnCheck[]): void {
  snapshot = {
    status: "ready",
    records: (next ?? []).filter(
      (snooze) => snooze.until === null || snooze.until > Date.now(),
    ),
  }
  scheduleExpiry()
  for (const listener of listeners) listener()
}

function publishError(): void {
  snapshot = { status: "error", records: snapshot.records }
  for (const listener of listeners) listener()
}

export async function refreshSnoozedBurnChecks(): Promise<void> {
  const requestRevision = ++revision
  if (!hasShell()) {
    publish([])
    return
  }
  try {
    const records = await invoke<SnoozedBurnCheck[]>("list_burn_check_snoozes")
    if (requestRevision === revision) publish(records)
  } catch {
    if (requestRevision === revision) publishError()
  }
}

function startEventListener(): void {
  if (eventListenerStarted) return
  eventListenerStarted = true
  const generation = ++listenerGeneration
  void refreshSnoozedBurnChecks()
  if (!hasShell()) return
  void listen<SnoozedBurnCheck[]>(SNOOZES_CHANGED_EVENT, (event) => {
    if (generation !== listenerGeneration) return
    revision += 1
    publish(event.payload)
  })
    .then((unlisten) => {
      if (eventListenerStarted && generation === listenerGeneration)
        stopEventListener = unlisten
      else unlisten()
    })
    .catch(() => {
      if (eventListenerStarted && generation === listenerGeneration) publishError()
    })
}

function stopListening(): void {
  eventListenerStarted = false
  listenerGeneration += 1
  revision += 1
  stopEventListener?.()
  stopEventListener = null
}

export async function snoozeBurnCheck(
  detector: BurnCheckDetectorId,
  duration: SnoozeDuration,
): Promise<void> {
  const record: SnoozedBurnCheck = { detector, scope: "check", until: snoozeUntil(duration) }
  if (!hasShell()) return
  const requestRevision = ++revision
  try {
    const records = await invoke<SnoozedBurnCheck[]>("set_burn_check_snooze", {
      snooze: record,
    })
    if (requestRevision === revision) publish(records)
  } catch {
    if (requestRevision === revision) publishError()
  }
}

export async function unsnoozeBurnCheck(detector: BurnCheckDetectorId): Promise<void> {
  if (!hasShell()) return
  const requestRevision = ++revision
  try {
    const records = await invoke<SnoozedBurnCheck[]>("clear_burn_check_snooze", { detector })
    if (requestRevision === revision) publish(records)
  } catch {
    if (requestRevision === revision) publishError()
  }
}

export function snoozedDetectorIds(
  records: readonly SnoozedBurnCheck[] | null | undefined,
): ReadonlySet<BurnCheckDetectorId> {
  return new Set((records ?? []).map((record) => record.detector))
}

/** Remove detector-level snoozes before deriving any report state. */
export function visibleCheckCategories(
  categories: readonly ChecksCategoryPayload[],
  snoozed: ReadonlySet<BurnCheckDetectorId>,
): ChecksCategoryPayload[] {
  return categories.filter((category) => !snoozed.has(category.id))
}

export function activeChecksReport(
  report: ChecksReportPayload,
  snoozed: ReadonlySet<BurnCheckDetectorId>,
): ChecksReportPayload {
  if (snoozed.size === 0) return report
  const categories = visibleCheckCategories(report.categories, snoozed)
  if (categories.length === report.categories.length) return report
  const mask = detectorMask(new Set(categories.map((category) => category.id)))
  return {
    ...report,
    categories,
    estimatedTokenBurnBasisPoints:
      report.estimatedTokenBurnBasisPointsByDetectorMask?.[mask] ?? null,
  }
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

/** Remove unused-resource rows whose detector is snoozed. */
export function visibleUnusedContextRows(
  rows: readonly UnusedContextRow[],
  snoozed: ReadonlySet<BurnCheckDetectorId>,
): UnusedContextRow[] {
  return rows.filter((row) => {
    const detector =
      row.kind === "MCP server"
        ? "unusedMcpServers"
        : row.kind === "Built-in tool"
          ? "unusedBuiltInTools"
          : "unusedSkills"
    return !snoozed.has(detector)
  })
}

export function formatSnoozeUntil(until: number | null): string {
  if (until === null) return "Snoozed forever"
  return `Snoozed until ${new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" }).format(until)}`
}

function subscribeSnoozedBurnChecks(listener: () => void): () => void {
  listeners.add(listener)
  startEventListener()
  return () => {
    listeners.delete(listener)
    if (listeners.size === 0) {
      stopListening()
      if (expiryTimer) {
        clearTimeout(expiryTimer)
        expiryTimer = null
      }
    }
  }
}

export function useSnoozedBurnChecks(): SnoozedBurnChecksState {
  return useSyncExternalStore(
    subscribeSnoozedBurnChecks,
    () => snapshot,
    () => serverSnapshot,
  )
}
