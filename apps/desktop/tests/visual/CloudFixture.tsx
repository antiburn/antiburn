/** Isolated synthetic Cloud views for browser evidence, with no desktop credentials or source I/O. */
import { createCloudDevPort, isCloudDevScenario } from "../../src/onboarding/cloudDevPort"
import { CloudOnboarding } from "../../src/onboarding/CloudOnboarding"
import { useState, useSyncExternalStore } from "react"
import { CloudSession, cloudUploadStatusUnknown } from "../../src/onboarding/CloudSession"
import { CloudAccount } from "../../src/views/settings/CloudAccount"
import { SettingsToggleRow } from "../../src/views/settings/SettingsSearchRows"
import { Card } from "../../src/components/ui/Card"
import { SectionGroup } from "../../src/components/ui/SectionGroup"
import {
  emptyCloudSnapshot,
  type CloudPort,
  type CloudSnapshot,
  type SyncResult,
  type CloudOngoingStatus,
  type DesktopCloudProjectGroup,
} from "../../src/lib/cloud"
import type { LocalToolsStatus } from "../../src/lib/ipcPayloads"

/** Build preview-only account/request state; simulated success cannot establish product support. */
export function cloudFixture(name: string): CloudPort {
  const dev = name.startsWith("dev-") ? name.slice(4) : null
  if (isCloudDevScenario(dev)) return createCloudDevPort(dev)
  const settingsFolders = ["settings-folders", "settings-attention"].includes(name)
  const homePaths =
    name === "folders-long-paths" || settingsFolders || name === "settings-missing-sources"
  const authenticated = [
    "identity",
    "identity_error",
    "folders",
    "folders-unchecked",
    "folders-many",
    "folders-long-paths",
    "folders-empty-partial",
    "folders-empty-paged",
    "empty",
    "complete",
    "tools-ready",
    "tools-pending",
    "tools-failed",
    "tools-unavailable",
    "tools-live",
    "accepted",
    "partial",
    "failed",
    "ambiguous",
    "syncing",
    "unavailable",
    "cancelled",
    "settings-progress",
    "settings-empty",
    "settings-storage-error",
    "settings-reopened",
    "settings-folders",
    "settings-attention",
    "settings-missing-sources",
  ].includes(name)
  let value: CloudSnapshot = {
    ...emptyCloudSnapshot,
    authEnabled: !["marketing", "returned", "marketing_failed"].includes(name),
    syncEnabled:
      name === "settings-progress" || name === "settings-reopened" || settingsFolders,
    captureEnabled: authenticated && name !== "unavailable",
    status: authenticated
      ? "signed_in"
      : name === "marketing" || name === "auth"
        ? "idle"
        : name === "marketing_failed"
          ? "browser_failed"
          : (name as CloudSnapshot["status"]),
    identity: authenticated ? { id: "synthetic", login: "example" } : null,
    destination: authenticated
      ? { id: "synthetic-personal", name: "Personal", kind: "personal" }
      : null,
    draft: {
      screen: ["identity", "identity_error"].includes(name)
        ? "identity"
        : name === "complete" || name.startsWith("tools-")
          ? "complete"
          : authenticated
            ? "folders"
            : "handoff",
      folders: [],
    },
  }
  let saveFailed = false
  const groups: DesktopCloudProjectGroup[] =
    name === "empty" || name === "settings-empty"
      ? []
      : [
          { label: "Studio", count: 128 },
          { label: "Website", count: 64 },
          ...(homePaths
            ? [
                { label: "Tools", count: 18 },
                { label: "Desktop", count: 42 },
                { label: "Examples", count: 3 },
                { label: "Integrations", count: 17 },
                { label: "Docs", count: 9 },
              ]
            : []),
          ...(name === "folders-long-paths" ? [{ label: "API", count: 23 }] : []),
          ...(name === "folders-many"
            ? [
                { label: "Desktop", count: 42 },
                { label: "Documentation preview and integrations", count: 9 },
                { label: "Tools", count: 18 },
                { label: "Examples", count: 3 },
              ]
            : []),
        ].map(({ label, count }) => {
          const path = homePaths ? `/Users/avery/dev/${label}` : `/Example/${label}`
          const folderPaths =
            label !== "Studio"
              ? [path]
              : homePaths
                ? [
                    path,
                    "/Users/avery/dev/worktrees/studio-documentation-preview/feature-cloud-folder-layout-and-readable-paths",
                    "/Users/avery/dev/worktrees/studio-integration-tests/feature-account-bound-sync-selection",
                  ]
                : [path, "/Example/worktrees/studio-menu-fix"]
          return {
            id: label,
            environment: "native",
            path,
            label,
            folderPaths,
            worktreeCount: folderPaths.length - 1,
            agents: ["claude"],
            sessionCount: count,
            captureSessionCount: count,
            captureSupported: name !== "folders-unchecked",
            groupingComplete: true,
            projects: folderPaths.map((projectPath) => ({
              sourceKind: "claude_jsonl" as const,
              sourcePath: `/Example/Sources/${label}`,
              projectPath,
            })),
          }
        })
  if (name === "settings-reopened")
    groups.push({
      id: "NewProject",
      environment: "native",
      label: "New project",
      path: "/Example/NewProject",
      folderPaths: ["/Example/NewProject"],
      worktreeCount: 0,
      agents: ["claude"],
      sessionCount: 2,
      captureSessionCount: 2,
      captureSupported: true,
      groupingComplete: true,
      projects: [
        {
          sourceKind: "claude_jsonl",
          sourcePath: "/Example/Sources/NewProject",
          projectPath: "/Example/NewProject",
        },
      ],
    })
  let ongoing: CloudOngoingStatus = {
    storageKnown: false,
    status: "consent_required",
    accountId: "synthetic",
    destinationId: "synthetic-personal",
    consentVersion: null,
    policyRevision: "fixture-policy",
    paused: false,
    projects: [],
    projectChoices: {},
    selectedSources: [],
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
  if (name === "settings-progress" || name === "settings-storage-error")
    ongoing = {
      ...ongoing,
      storageKnown: true,
      status: "syncing",
      consentVersion: "desktop-project-sync-v1",
      selectedSources: groups.map((group) => ({
        sourceKind: "claude_jsonl",
        path: group.projects[0]!.sourcePath,
      })),
      projects: groups.flatMap((group) => group.projects),
      projectChoices: Object.fromEntries(groups.map((group) => [group.id, true])),
      acceptedCount: 41,
      queuedCount: 3,
      progress: {
        negotiated: true,
        totalCompressedBytes: 8_388_608,
        requiredTransferBytes: 3_145_728,
        reusedBytes: 5_242_880,
        confirmedUploadBytes: 1_048_576,
      },
    }
  if (name === "settings-storage-error")
    ongoing = {
      ...ongoing,
      status: "storage_unavailable",
      storageKnown: false,
    }
  if (name === "settings-reopened")
    ongoing = {
      ...ongoing,
      storageKnown: true,
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      selectedSources: [{ sourceKind: "claude_jsonl", path: "/Example/Sources/Studio" }],
      projects: groups[0]!.projects,
      projectChoices: { Studio: true, Website: false },
    }
  if (settingsFolders) {
    const approved = groups.filter((group) => group.id !== "Website")
    ongoing = {
      ...ongoing,
      storageKnown: true,
      status: "syncing",
      consentVersion: "desktop-project-sync-v1",
      selectedSources: approved.map((group) => ({
        sourceKind: "claude_jsonl",
        path: group.projects[0]!.sourcePath,
      })),
      projects: approved.flatMap((group) => group.projects),
      projectChoices: Object.fromEntries(
        groups.map((group) => [group.id, group.id !== "Website"]),
      ),
      queuedCount: 2,
      acceptedCount: 3,
      fencedCount: name === "settings-attention" ? 1 : 0,
    }
  }
  const toolsStatus = (phase: LocalToolsStatus["phase"], revision = 0): LocalToolsStatus => ({
    revision,
    phase,
    cliInstalled: phase === "ready" || phase === "failed",
    cliTested: phase === "ready",
    skillInstalled: phase === "ready",
    reason: phase === "failed" ? "cli_test_failed" : null,
  })
  let localTools = toolsStatus(
    name === "tools-ready"
      ? "ready"
      : name === "complete" || name === "tools-pending" || name === "tools-live"
        ? "pending"
        : name === "tools-failed"
          ? "failed"
          : "unavailable",
  )
  return {
    displayHome: async () => "/Users/avery",
    ongoing: {
      groups: async (_binding, after) => {
        const emptyPage =
          ["folders-empty-partial", "folders-empty-paged"].includes(name) && !after
        return {
          groups: emptyPage ? [] : groups,
          sourceRoots:
            emptyPage || name === "settings-missing-sources"
              ? []
              : groups.flatMap((group) =>
                  group.projects.map((project) => ({
                    sourceKind: project.sourceKind,
                    path: project.sourcePath,
                  })),
                ),
          nextCursor:
            name === "folders-empty-paged" && !after
              ? { environment: "native", cwd: "/Example/Next" }
              : null,
          partial: name === "folders-empty-partial",
        }
      },
      status: async () => ongoing,
      watch: async () => () => {},
      enable: async (consent) => {
        if (name === "ambiguous") throw new Error("Synthetic unknown result")
        ongoing = {
          ...ongoing,
          storageKnown: true,
          consentVersion: consent.version,
          projects: consent.projects,
          projectChoices: consent.projectChoices,
          selectedSources: consent.selectedSources,
          status:
            name === "failed"
              ? "upload_unavailable"
              : name === "partial"
                ? "fenced"
                : name === "syncing"
                  ? "syncing"
                  : "ready",
          queuedCount: name === "syncing" ? 2 : 0,
          acceptedCount: name === "partial" ? 1 : 0,
          fencedCount: name === "partial" ? 1 : 0,
        }
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
        return ongoing
      },
      pause: async () => {
        ongoing = { ...ongoing, paused: true, status: "paused" }
        return ongoing
      },
      retry: async () => {
        ongoing = { ...ongoing, paused: false, status: "ready" }
        return ongoing
      },
    },
    read: async () => value,
    watch: async () => () => {},
    begin: async () => {
      value = { ...value, status: value.authEnabled ? "waiting" : "idle" }
      return value
    },
    cancel: async () => {
      value = { ...value, status: "cancelled" }
      return value
    },
    save: async (draft) => {
      if (name === "identity_error" && !saveFailed) {
        saveFailed = true
        throw new Error("Synthetic save failure")
      }
      value = { ...value, draft }
      return value
    },
    signOut: async () => {
      value = { ...emptyCloudSnapshot, authEnabled: value.authEnabled, status: "signed_out" }
      return value
    },
    resetLocal: async () => {
      value = { ...emptyCloudSnapshot, status: "signed_out", remoteRevocationUnconfirmed: true }
      return value
    },
    pick: async () => ["/Example/New folder"],
    localTools: {
      read: async () => localTools,
      watch: async (listener) => {
        if (name !== "tools-live") return () => {}
        const stops = (["pending", "ready", "failed"] as const).map((phase) => {
          const event = `fixture:local-tools-${phase}`
          const update = () => {
            localTools = toolsStatus(phase, localTools.revision + 1)
            listener(localTools)
          }
          window.addEventListener(event, update)
          return () => window.removeEventListener(event, update)
        })
        return () => stops.forEach((stop) => stop())
      },
    },
    prepareSync: async () => {},
    start: async () =>
      name === "ambiguous"
        ? Promise.reject(new Error("Synthetic network ambiguity"))
        : name === "failed"
          ? { status: "failed" }
          : {
              status: name as "accepted" | "partial" | "syncing",
              operationId: "synthetic-operation",
              completed: name === "partial" ? 1 : 0,
              total: 2,
            },
    reconcile: async (): Promise<SyncResult> => ({
      status: "accepted",
      operationId: "synthetic-operation",
      completed: 0,
      total: 2,
    }),
    cancelSync: async () => ({ status: "cancelled" }),
  }
}
/** Render the selected synthetic view without analytics or persisted onboarding changes. */
export function CloudFixture() {
  const name = new URLSearchParams(window.location.search).get("cloud") ?? "marketing"
  if (
    name === "settings-progress" ||
    name === "settings-empty" ||
    name === "settings-storage-error" ||
    name === "settings-reopened" ||
    name === "settings-folders" ||
    name === "settings-attention" ||
    name === "settings-missing-sources"
  )
    return <CloudSettingsFixture name={name} />
  return (
    <CloudOnboarding
      port={cloudFixture(name)}
      analytics={false}
      holdIdentity={name === "identity"}
      onBack={() => {}}
      onComplete={() => {
        document.body.dataset.completed = "true"
      }}
    />
  )
}

/** Use current Settings components with controlled IPC and synthetic transfer totals. */
function CloudSettingsFixture({ name }: { name: string }) {
  const [session] = useState(() => new CloudSession(cloudFixture(name), false, true))
  const value = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const storageUnknown = cloudUploadStatusUnknown(value.ongoing)
  return (
    <main
      className="h-full overflow-y-auto bg-surface-window p-6 text-label"
      aria-label="Cloud Settings preview"
    >
      <div className="mx-auto max-w-3xl">
        <SectionGroup title="Antiburn Cloud">
          <Card divided={false}>
            <CloudAccount session={session} value={value}>
              <SettingsToggleRow
                searchId="cloudUpload"
                checked={
                  value.ongoing?.consentVersion === "desktop-project-sync-v1" &&
                  !value.ongoing.paused
                }
                disabled={value.busy || storageUnknown || !value.ongoing?.consentVersion}
                disabledTooltip={
                  storageUnknown
                    ? "Could not read saved uploads."
                    : !value.ongoing?.consentVersion
                      ? "Choose folders, then Start syncing."
                      : undefined
                }
                onChange={(enabled) => void session.controlSync(!enabled)}
              />
            </CloudAccount>
          </Card>
        </SectionGroup>
      </div>
    </main>
  )
}
