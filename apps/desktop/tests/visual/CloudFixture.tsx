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

/** Build preview-only account/request state; simulated success cannot establish product support. */
export function cloudFixture(name: string): CloudPort {
  const dev = name.startsWith("dev-") ? name.slice(4) : null
  if (isCloudDevScenario(dev)) return createCloudDevPort(dev)
  const authenticated = [
    "identity",
    "identity_error",
    "folders",
    "empty",
    "complete",
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
  ].includes(name)
  let value: CloudSnapshot = {
    ...emptyCloudSnapshot,
    authEnabled: !["marketing", "returned", "marketing_failed"].includes(name),
    syncEnabled: name === "settings-progress" || name === "settings-reopened",
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
        : name === "complete"
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
        ].map(({ label, count }) => ({
          id: label,
          environment: "native",
          path: `/Example/${label}`,
          label,
          folderPaths:
            label === "Studio"
              ? [`/Example/${label}`, "/Example/worktrees/studio-menu-fix"]
              : [`/Example/${label}`],
          worktreeCount: label === "Studio" ? 1 : 0,
          agents: ["claude"],
          sessionCount: count,
          captureSessionCount: count,
          captureSupported: true,
          groupingComplete: true,
          projects: (label === "Studio"
            ? [`/Example/${label}`, "/Example/worktrees/studio-menu-fix"]
            : [`/Example/${label}`]
          ).map((projectPath) => ({
            sourceKind: "claude_jsonl",
            sourcePath: `/Example/Sources/${label}`,
            projectPath,
          })),
        }))
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
      interruptedCount: 1,
      fencedCount: 1,
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
      value = { ...emptyCloudSnapshot, status: "signed_out" }
      return value
    },
    resetLocal: async () => {
      value = { ...emptyCloudSnapshot, status: "signed_out", remoteRevocationUnconfirmed: true }
      return value
    },
    pick: async () => ["/Example/New folder"],
    agent: async () => (name === "complete" ? "pending" : "unavailable"),
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
    name === "settings-reopened"
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
          <Card>
            <CloudAccount session={session} value={value}>
              <SettingsToggleRow
                searchId="cloudUpload"
                description={
                  storageUnknown
                    ? "Last known setting."
                    : value.ongoing?.consentVersion === "desktop-project-sync-v1"
                      ? "Upload existing and new sessions from the approved projects. Pausing keeps saved uploads."
                      : "Choose projects and confirm full-content sync before enabling uploads."
                }
                checked={
                  value.ongoing?.consentVersion === "desktop-project-sync-v1" &&
                  !value.ongoing.paused
                }
                disabled={value.busy || storageUnknown || !value.ongoing?.consentVersion}
                disabledTooltip={
                  storageUnknown
                    ? "Check disk space and file permissions, then retry saved uploads."
                    : !value.ongoing?.consentVersion
                      ? "Choose projects and confirm full-content sync below."
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
