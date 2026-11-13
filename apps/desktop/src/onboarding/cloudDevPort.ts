import avatar from "../assets/cloud-dev-avatar.jpg"
import {
  emptyCloudSnapshot,
  type AgentReadiness,
  type CloudPort,
  type CloudSnapshot,
  type CloudStatus,
  type SyncResult,
} from "../lib/cloud"
export const cloudDevScenarios = [
  "walkthrough",
  "marketing",
  "auth",
  "waiting",
  "cancelled",
  "expired",
  "browser_failed",
  "offline",
  "rejected",
  "invalid_response",
  "credential_failed",
  "identity",
  "folders",
  "empty",
  "unavailable",
  "complete-ready",
  "complete-pending",
  "complete-unavailable",
  "complete-failed",
] as const
export type CloudDevScenario = (typeof cloudDevScenarios)[number]
export function isCloudDevScenario(value: unknown): value is CloudDevScenario {
  return typeof value === "string" && cloudDevScenarios.some((scenario) => scenario === value)
}
export function createCloudDevPort(scenario: CloudDevScenario): CloudPort {
  const completion = scenario.startsWith("complete-")
  const signedIn =
    completion || ["identity", "folders", "empty", "unavailable"].includes(scenario)
  const identity = { id: "cloud-dev-account", login: "example", avatar }
  const result = (status: string): SyncResult => {
    if (["accepted", "syncing", "complete", "partial"].includes(status))
      return {
        status: status as "accepted" | "syncing" | "complete" | "partial",
        operationId: "cloud-dev-operation",
        completed: status === "complete" ? 2 : status === "accepted" ? 0 : 1,
        total: 2,
      }
    return { status: status as "failed" | "ambiguous" | "cancelled" }
  }
  let value: CloudSnapshot = {
    ...emptyCloudSnapshot,
    authEnabled: scenario !== "marketing",
    syncEnabled: !["unavailable", "marketing"].includes(scenario),
    status: signedIn
      ? "signed_in"
      : ["walkthrough", "auth", "marketing"].includes(scenario)
        ? "idle"
        : (scenario as CloudStatus),
    identity: signedIn ? identity : null,
    draft: {
      screen: completion
        ? "complete"
        : signedIn
          ? scenario === "identity"
            ? "identity"
            : "folders"
          : "handoff",
      folders:
        scenario === "empty"
          ? []
          : [
              { path: "/Example/Studio", selected: true, sessionCount: 128 },
              { path: "/Example/Website", selected: true, sessionCount: 64 },
            ],
    },
  }
  const listeners = new Set<(snapshot: CloudSnapshot) => void>()
  const timers = new Set<ReturnType<typeof setTimeout>>()
  const clear = () => {
    timers.forEach(clearTimeout)
    timers.clear()
  }
  const publish = () => listeners.forEach((listener) => listener(value))
  const later = (action: () => void, delay: number) => {
    const timer = setTimeout(() => {
      timers.delete(timer)
      action()
    }, delay)
    timers.add(timer)
  }
  const progress = (requestId: string) => {
    for (const [delay, status] of [
      [1500, "syncing"],
      [3500, "complete"],
    ] as const) {
      if (scenario === "walkthrough" && status === "complete") continue
      later(() => {
        value = { ...value, pendingSync: { requestId, result: result(status) } }
        publish()
      }, delay)
    }
  }
  return {
    read: async () => value,
    watch: async (listener) => {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
        if (!listeners.size) clear()
      }
    },
    begin: async () => {
      clear()
      if (scenario === "marketing") return value
      value = { ...value, status: "waiting", identity: null, pendingSync: null }
      later(() => {
        value = {
          ...value,
          status: "signed_in",
          identity,
          draft: { ...value.draft, screen: "identity" },
        }
        publish()
      }, 1800)
      return value
    },
    cancel: async () => {
      clear()
      value = { ...value, status: "cancelled" }
      return value
    },
    save: async (draft) => {
      value = { ...value, draft, pendingSync: null }
      return value
    },
    signOut: async () => {
      clear()
      value = { ...value, status: "signed_out", identity: null, pendingSync: null }
      return value
    },
    pick: async () => ["/Example/New folder"],
    agent: async () => (completion ? (scenario.slice(9) as AgentReadiness) : "ready"),
    prepareSync: async () => {},
    start: async (_folders, requestId) => {
      clear()
      const accepted = result("accepted")
      value = { ...value, pendingSync: { requestId, result: accepted } }
      progress(requestId)
      return accepted
    },
    reconcile: async (requestId) => {
      clear()
      value = { ...value, pendingSync: { requestId, result: result("accepted") } }
      progress(requestId)
      return value.pendingSync!.result
    },
    cancelSync: async (requestId) => {
      clear()
      const cancelled = { status: "cancelled" } as const
      value = { ...value, pendingSync: { requestId, result: cancelled } }
      return cancelled
    },
  }
}
