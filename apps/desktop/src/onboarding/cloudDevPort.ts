/** Isolated preview data and timers. This module never uses desktop credentials or source I/O. */
import avatar from "../assets/cloud-dev-avatar.jpg"
import {
  emptyCloudSnapshot,
  type CloudPort,
  type CloudSnapshot,
  type CloudStatus,
  type SyncResult,
  type CloudOngoingStatus,
  type DesktopCloudProjectGroup,
} from "../lib/cloud"
/** Closed desktop-menu scenarios; unknown values must not select a preview. */
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
/** One synthetic scenario, not a service response or desktop account state. */
export type CloudDevScenario = (typeof cloudDevScenarios)[number]
/** Validate desktop preview selection before reading it as a scenario. */
export function isCloudDevScenario(value: unknown): value is CloudDevScenario {
  return typeof value === "string" && cloudDevScenarios.some((scenario) => scenario === value)
}
/** Create a separate in-memory preview. The last observer stops its timers. */
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
    syncEnabled: completion,
    captureEnabled: !["unavailable", "marketing"].includes(scenario),
    status: signedIn
      ? "signed_in"
      : ["walkthrough", "auth", "marketing"].includes(scenario)
        ? "idle"
        : (scenario as CloudStatus),
    identity: signedIn ? identity : null,
    destination: {
      id: "cloud-dev-personal",
      name: "Example personal workspace",
      kind: "personal",
    },
    draft: {
      screen: completion
        ? "complete"
        : signedIn
          ? scenario === "identity"
            ? "identity"
            : "folders"
          : "handoff",
      folders: [],
    },
  }
  const listeners = new Set<(snapshot: CloudSnapshot) => void>()
  const syncListeners = new Set<(status: CloudOngoingStatus) => void>()
  const groups: DesktopCloudProjectGroup[] =
    scenario === "empty"
      ? []
      : [
          ["Studio", 128],
          ["Website", 64],
        ].map(([name, count]) => ({
          id: `cloud-dev-${name}`,
          environment: "native",
          path: `/Example/${name}`,
          label: String(name),
          folderPaths: [`/Example/${name}`],
          worktreeCount: 0,
          agents: ["claude"],
          sessionCount: Number(count),
          captureSessionCount: Number(count),
          captureSupported: true,
          groupingComplete: true,
          projects: [
            {
              sourceKind: "claude_jsonl",
              sourcePath: `/Example/Sources/${name}`,
              projectPath: `/Example/${name}`,
            },
          ],
        }))
  let devicePreference: boolean | undefined
  let ongoing: CloudOngoingStatus = {
    storageKnown: completion,
    status: completion ? "ready" : "consent_required",
    accountId: identity.id,
    destinationId: "cloud-dev-personal",
    consentVersion: completion ? "desktop-project-sync-v1" : null,
    policyRevision: "preview-policy",
    deviceUploadEnabled: completion,
    paused: false,
    selectedSources: completion
      ? groups.flatMap((group) =>
          group.projects.map((project) => ({
            sourceKind: project.sourceKind,
            path: project.sourcePath,
          })),
        )
      : [],
    projects: completion ? groups.flatMap((group) => group.projects) : [],
    projectChoices: completion
      ? Object.fromEntries(groups.map((group) => [group.id, true]))
      : {},
    queuedCount: 0,
    interruptedCount: 0,
    fencedCount: 0,
    acceptedCount: 0,
    suppressedCount: 0,
    progress: {
      negotiated: false,
      totalCompressedBytes: 0,
      requiredTransferBytes: 0,
      reusedBytes: 0,
      confirmedUploadBytes: 0,
    },
  }
  const timers = new Set<ReturnType<typeof setTimeout>>()
  const clear = () => {
    timers.forEach(clearTimeout)
    timers.clear()
  }
  const publish = () => listeners.forEach((listener) => listener(value))
  const publishSync = () => syncListeners.forEach((listener) => listener(ongoing))
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
  const ongoingProgress = () => {
    later(() => {
      ongoing = { ...ongoing, status: "syncing", queuedCount: 2 }
      publishSync()
    }, 1500)
    if (scenario !== "walkthrough")
      later(() => {
        ongoing = { ...ongoing, status: "ready", queuedCount: 0, acceptedCount: 2 }
        publishSync()
      }, 3500)
  }
  return {
    ongoing: {
      groups: async () => ({
        groups,
        sourceRoots: groups.flatMap((group) =>
          group.projects.map((project) => ({
            sourceKind: project.sourceKind,
            path: project.sourcePath,
          })),
        ),
        nextCursor: null,
        partial: false,
      }),
      status: async () => ongoing,
      watch: async (listener) => {
        syncListeners.add(listener)
        return () => {
          syncListeners.delete(listener)
          if (!listeners.size && !syncListeners.size) clear()
        }
      },
      enable: async (consent) => {
        clear()
        const enabled = devicePreference ?? true
        ongoing = {
          ...ongoing,
          storageKnown: true,
          status: enabled ? "ready" : "paused",
          deviceUploadEnabled: enabled,
          consentVersion: consent.version,
          selectedSources: consent.selectedSources,
          projects: consent.projects,
          projectChoices: consent.projectChoices,
          paused: !enabled,
        }
        value = { ...value, syncEnabled: enabled }
        publish()
        publishSync()
        if (enabled) ongoingProgress()
        return ongoing
      },
      setProject: async ({ groupId, selected, projects }) => {
        const paths = new Set(groups.find((group) => group.id === groupId)?.folderPaths)
        const next = ongoing.projects.filter((project) => !paths.has(project.projectPath))
        ongoing = {
          ...ongoing,
          projects: selected ? [...next, ...projects] : next,
          projectChoices: { ...ongoing.projectChoices, [groupId]: selected },
        }
        publishSync()
        return ongoing
      },
      setEnabled: async (enabled) => {
        clear()
        devicePreference = enabled
        ongoing = {
          ...ongoing,
          deviceUploadEnabled: enabled,
          status: !ongoing.consentVersion ? "consent_required" : enabled ? "ready" : "paused",
          paused: !!ongoing.consentVersion && !enabled,
        }
        value = { ...value, syncEnabled: enabled && !!ongoing.consentVersion }
        publish()
        publishSync()
        if (value.syncEnabled) ongoingProgress()
        return ongoing
      },
      retry: async () => {
        if (!value.syncEnabled) throw new Error("Enable uploads before retrying.")
        ongoing = { ...ongoing, status: "ready", paused: false }
        value = { ...value, syncEnabled: true }
        publish()
        publishSync()
        ongoingProgress()
        return ongoing
      },
    },
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
    resetLocal: async () => {
      clear()
      value = {
        ...value,
        status: "signed_out",
        identity: null,
        pendingSync: null,
        revocationPending: false,
        localResetAvailable: false,
        remoteRevocationUnconfirmed: true,
      }
      return value
    },
    pick: async () => ["/Example/New folder"],
    localTools: {
      read: async () => {
        const phase =
          scenario === "complete-pending"
            ? "pending"
            : scenario === "complete-unavailable"
              ? "unavailable"
              : scenario === "complete-failed"
                ? "failed"
                : "ready"
        return {
          revision: 0,
          phase,
          cliInstalled: phase === "ready",
          cliTested: phase === "ready",
          skillInstalled: phase === "ready",
          reason: phase === "failed" ? "setup_unavailable" : null,
        }
      },
      watch: async () => () => {},
    },
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
