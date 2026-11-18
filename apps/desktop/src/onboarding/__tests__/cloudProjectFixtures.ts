import { vi, type Mocked } from "vitest"
import {
  emptyCloudSnapshot,
  desktopCloudPort,
  type CloudOngoingConsent,
  type CloudOngoingStatus,
  type CloudOngoingPort,
  type CloudPort,
  type CloudSnapshot,
  type DesktopCloudProjectGroup,
} from "../../lib/cloud"
import type { LocalToolsStatus } from "../../lib/ipcPayloads"

/** Synthetic tool evidence. This helper never reads an installed CLI or skill. */
export function localToolsStatus(
  phase: LocalToolsStatus["phase"] = "unavailable",
  revision = 0,
): LocalToolsStatus {
  return {
    revision,
    phase,
    cliInstalled: phase === "ready",
    cliTested: phase === "ready",
    skillInstalled: phase === "ready",
    reason: phase === "failed" ? "setup_unavailable" : null,
  }
}

/** Model independent tool events and reads at the renderer's IPC boundary. */
export function localToolsFixture(initial = localToolsStatus()) {
  let value = initial
  let publish: (status: LocalToolsStatus) => void = () => {}
  const port: Mocked<CloudPort["localTools"]> = {
    read: vi.fn(async () => value),
    watch: vi.fn<CloudPort["localTools"]["watch"]>(async (listener) => {
      publish = listener
      return vi.fn<() => void>()
    }),
  }
  return {
    port,
    emit: (status: LocalToolsStatus) => {
      value = status
      publish(status)
    },
  }
}

/** Synthetic desktop metadata. No provider paths or credentials are read. */
export function projectGroup(
  patch: Partial<DesktopCloudProjectGroup> = {},
): DesktopCloudProjectGroup {
  return {
    id: "studio",
    environment: "native",
    path: "/Example/Studio",
    label: "Studio",
    folderPaths: ["/Example/Studio", "/Example/worktrees/studio-fix"],
    worktreeCount: 1,
    agents: ["claude"],
    sessionCount: 12,
    captureSessionCount: 12,
    captureSupported: true,
    groupingComplete: true,
    projects: ["/Example/Studio", "/Example/worktrees/studio-fix"].map((projectPath) => ({
      sourceKind: "claude_jsonl",
      sourcePath: "/Example/claude/projects/studio",
      projectPath,
    })),
    ...patch,
  }
}

/** Closed status fixture. Ready means a saved policy, not an accepted upload. */
export function ongoingStatus(patch: Partial<CloudOngoingStatus> = {}): CloudOngoingStatus {
  return {
    storageKnown: true,
    status: "consent_required",
    accountId: "synthetic-account",
    destinationId: "synthetic-personal",
    consentVersion: null,
    policyRevision: "synthetic-policy",
    paused: false,
    selectedSources: [],
    projects: [],
    projectChoices: {},
    acceptedCount: 0,
    queuedCount: 0,
    interruptedCount: 0,
    recoveryRequiredCount: 0,
    fencedCount: 0,
    suppressedCount: 0,
    progress: {
      negotiated: false,
      totalCompressedBytes: 0,
      requiredTransferBytes: 0,
      reusedBytes: 0,
      confirmedUploadBytes: 0,
    },
    ...patch,
  }
}

/** Mock desktop IPC at its renderer boundary. DOM tests do not prove service integration. */
export function cloudProjectFixture(): {
  port: CloudPort
  ongoing: Mocked<CloudOngoingPort>
  emit: (patch: Partial<CloudSnapshot>) => void
  emitSync: (status: CloudOngoingStatus) => void
  emitLocalTools: (status: LocalToolsStatus) => void
} {
  let value: CloudSnapshot = {
    ...emptyCloudSnapshot,
    authEnabled: true,
    captureEnabled: true,
    status: "signed_in",
    identity: { id: "synthetic-account", login: "Example" },
    destination: { id: "synthetic-personal", name: "Personal", kind: "personal" },
    draft: { screen: "folders", folders: [] },
  }
  let changed: (next: CloudSnapshot) => void = () => {}
  let syncChanged: (next: CloudOngoingStatus) => void = () => {}
  const root = { sourceKind: "claude_jsonl" as const, path: "/Example/claude/projects/studio" }
  const ongoing: Mocked<CloudOngoingPort> = {
    groups: vi.fn<CloudOngoingPort["groups"]>(async () => ({
      groups: [projectGroup()],
      sourceRoots: [root],
      nextCursor: null as { environment: string; cwd: string } | null,
      partial: false,
    })),
    status: vi.fn<CloudOngoingPort["status"]>(async () => ongoingStatus()),
    watch: vi.fn<CloudOngoingPort["watch"]>(async (listener) => {
      syncChanged = listener
      return vi.fn<() => void>()
    }),
    enable: vi.fn<CloudOngoingPort["enable"]>(async (consent: CloudOngoingConsent) =>
      ongoingStatus({
        status: "ready",
        consentVersion: consent.version,
        projects: consent.projects,
        selectedSources: consent.selectedSources,
        projectChoices: consent.projectChoices,
      }),
    ),
    setProject: vi.fn<CloudOngoingPort["setProject"]>(async (change) =>
      ongoingStatus({
        status: "ready",
        consentVersion: "desktop-project-sync-v1",
        projectChoices: { [change.groupId]: change.selected },
        projects: change.selected ? change.projects : [],
      }),
    ),
    pause: vi.fn<CloudOngoingPort["pause"]>(async () =>
      ongoingStatus({
        status: "paused",
        paused: true,
        consentVersion: "desktop-project-sync-v1",
      }),
    ),
    retry: vi.fn<CloudOngoingPort["retry"]>(async () =>
      ongoingStatus({ status: "ready", consentVersion: "desktop-project-sync-v1" }),
    ),
  }
  const localTools = localToolsFixture()
  const port: CloudPort = {
    ...desktopCloudPort,
    ongoing,
    read: vi.fn(async () => value),
    watch: vi.fn(async (listener) => {
      changed = listener
      return vi.fn<() => void>()
    }),
    save: vi.fn(async (draft) => {
      value = { ...value, draft }
      return value
    }),
    localTools: localTools.port,
    prepareSync: vi.fn(async () => {}),
    start: vi.fn(async () => ({ status: "upload_unavailable" as const })),
  }
  return {
    port,
    ongoing,
    emit: (patch: Partial<CloudSnapshot>) => {
      value = { ...value, ...patch }
      changed(value)
    },
    emitSync: (status: CloudOngoingStatus) => syncChanged(status),
    emitLocalTools: localTools.emit,
  }
}
