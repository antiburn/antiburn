/** Isolated synthetic Cloud views for browser evidence, with no desktop credentials or source I/O. */
import { createCloudDevPort, isCloudDevScenario } from "../../src/onboarding/cloudDevPort"
import { CloudOnboarding } from "../../src/onboarding/CloudOnboarding"
import { useState, useSyncExternalStore } from "react"
import { CloudSession, cloudUploadStatusUnknown } from "../../src/onboarding/CloudSession"
import { CloudAccount } from "../../src/views/settings/CloudAccount"
import { SettingsRow, SettingsToggleRow } from "../../src/views/settings/SettingsSearchRows"
import { Card } from "../../src/components/ui/Card"
import { SectionGroup } from "../../src/components/ui/SectionGroup"
import {
  emptyCloudSnapshot,
  type CloudPort,
  type CloudSnapshot,
  type CloudOngoingStatus,
  type DesktopCloudProjectGroup,
} from "../../src/lib/cloud"
import type { LocalToolsStatus } from "../../src/lib/ipcPayloads"

const teamName = "Example Team for Design, Documentation, and Developer Experience"

/** Build preview-only account/request state; simulated success cannot establish product support. */
export function cloudFixture(name: string, refreshFolders?: () => void): CloudPort {
  const params = new URLSearchParams(window.location.search)
  const deferFolders = params.has("deferFolders")
  const rollingProgress = params.has("rollingProgress")
  const backgroundProgress = params.has("backgroundProgress")
  const partialSettingsFolders = params.has("partialSettingsFolders")
  const pagedSettingsFolders = params.has("pagedSettingsFolders")
  const joinedTeam = name === "settings-folders" && params.has("joinedTeam")
  const browserSetup = params.has("browserSetup")
  const hideGroupedFolderOnRefresh = params.has("reverseFolderFocus")
  const firstPersonalSaveFailure =
    name === "settings-folders" && params.has("personalSaveFailure")
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
    "settings-team-confirmation",
    "settings-missing-sources",
  ].includes(name)
  let value: CloudSnapshot = {
    ...emptyCloudSnapshot,
    authEnabled: !["marketing", "marketing_failed"].includes(name),
    syncEnabled:
      name === "settings-progress" ||
      name === "settings-reopened" ||
      (settingsFolders && !firstPersonalSaveFailure),
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
  let folderReadCount = 0
  let refreshFailures = 0
  let folderReleasePending = false
  let folderWatcher: (() => void) | undefined
  const pendingFolderReads = new Set<() => void>()
  const releasePendingFolderReads = () => {
    for (const resolve of pendingFolderReads) resolve()
    pendingFolderReads.clear()
  }
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
  if (deferFolders) groups.splice(1)
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
  if (name === "settings-team-confirmation") {
    const saved = groups[0]!
    ongoing = {
      ...ongoing,
      storageKnown: true,
      deviceUploadEnabled: true,
      teamStatuses: [
        {
          ...ongoing,
          status: "reauth_required",
          accountId: "synthetic",
          destinationId: "synthetic-team",
          workspaceName: teamName,
          consentVersion: "desktop-project-sync-v1",
          policyRevision: "fixture-stale-team",
          folderConfirmationRequired: true,
          projects: saved.projects,
          savedFolders: saved.folderPaths.map((path) => ({ groupId: `saved-${path}`, path })),
        },
      ],
    }
  }
  if (name === "settings-progress" || name === "settings-storage-error")
    ongoing = {
      ...ongoing,
      storageKnown: true,
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      deviceUploadEnabled: true,
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
  if (rollingProgress) ongoing = { ...ongoing, queuedCount: 0, acceptedCount: 0 }
  if (backgroundProgress) {
    const folder = { groupId: "saved-studio", path: groups[0]!.path }
    ongoing = {
      ...ongoing,
      backgroundSyncInProgress: true,
      queuedCount: 1,
      savedFolders: [folder],
      backgroundProgress: {
        uploaded: 2,
        total: null,
        found: 5,
        folderFound: { [folder.groupId]: 5 },
        transfer: { sentBytes: 12_000_000, totalBytes: null },
      },
    }
  }
  if (params.has("savedRecovery"))
    ongoing = { ...ongoing, automaticRecoveryCount: 1, retryAvailable: false }
  if (params.has("savedFolderFocus")) {
    const path = groups[0]!.folderPaths[1]!
    ongoing = { ...ongoing, savedFolders: [{ path, groupId: "saved-worktree" }] }
  }
  if (name === "settings-reopened")
    ongoing = {
      ...ongoing,
      storageKnown: true,
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      deviceUploadEnabled: true,
      selectedSources: [{ sourceKind: "claude_jsonl", path: "/Example/Sources/Studio" }],
      projects: groups[0]!.projects,
      projectChoices: { Studio: true, Website: false },
    }
  if (settingsFolders && !firstPersonalSaveFailure) {
    const approved = groups.filter((group) => group.id !== "Website")
    ongoing = {
      ...ongoing,
      storageKnown: true,
      status: "ready",
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
      fencedCount: name === "settings-attention" || params.has("footerFailure") ? 1 : 0,
      retryAvailable: name === "settings-attention" || params.has("footerFailure"),
      deviceUploadEnabled: true,
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
  let failTeamSave = params.get("saveFailure") === "1"
  let personalSaveCalls = 0
  let projectWrites = 0
  let syncCalls = 0
  let hideStudio = false
  let showQueueProgress = false
  const ongoingStatus = (): CloudOngoingStatus =>
    showQueueProgress
      ? {
          ...ongoing,
          status: "syncing",
          fullSyncInProgress: true,
          queuedCount: 3,
          uploadProgress: {
            uploaded: 6,
            total: 96,
            transfer: { sentBytes: 1_000_000, totalBytes: 3_000_000 },
          },
        }
      : ongoing
  return {
    displayHome: async () => "/Users/avery",
    ...(browserSetup && {
      openSetup: async () => {
        document.body.dataset.browserSetupOpened = "true"
      },
    }),
    ...(browserSetup &&
      name === "identity" && {
        workspaces: async () => [
          { id: "synthetic-personal", name: "Personal", kind: "personal" as const },
          ...(document.body.dataset.browserSetupOpened === "true"
            ? [{ id: "synthetic-team", name: teamName, kind: "team" as const }]
            : []),
        ],
      }),
    ...(name === "settings-team-confirmation"
      ? {
          workspaces: async () => [
            { id: "synthetic-personal", name: "Personal", kind: "personal" as const },
            { id: "synthetic-team", name: teamName, kind: "team" as const },
          ],
        }
      : {}),
    ...(joinedTeam
      ? {
          workspaces: async () => [
            { id: "synthetic-personal", name: "Personal", kind: "personal" as const },
            ...(document.body.dataset.joinedTeam === "true"
              ? [{ id: "joined-team", name: "Joined Team", kind: "team" as const }]
              : []),
          ],
        }
      : {}),
    ongoing: {
      watch: async (listener) => {
        const finish = () => {
          ongoing = {
            ...ongoing,
            backfillInProgress: false,
            ...(name === "settings-attention" && {
              status: "syncing",
              uploadProgress: {
                uploaded: 3,
                total: 8,
                transfer: { sentBytes: 0, totalBytes: null },
              },
            }),
            ...(rollingProgress && {
              status: "ready",
              uploadProgress: {
                uploaded: 1,
                total: 5,
                transfer: { sentBytes: 12_000_000, totalBytes: 85_000_000 },
              },
            }),
          }
          listener(ongoingStatus())
        }
        const event = "cloud-fixture:finish-sync"
        const updateTransfer = () => {
          ongoing = {
            ...ongoing,
            status: "syncing",
            automaticRecoveryCount: 0,
            uploadProgress: {
              uploaded: rollingProgress ? 2 : 3,
              total: rollingProgress ? 5 : 8,
              transfer: {
                sentBytes: rollingProgress ? 17_000_000 : 12_000_000,
                totalBytes: 85_000_000,
              },
            },
            uploadTransfer: rollingProgress
              ? { sentBytes: 2_000_000, totalBytes: 5_000_000 }
              : { sentBytes: 12_000_000, totalBytes: 85_000_000 },
          }
          listener(ongoingStatus())
        }
        const finishTransfer = () => {
          if (ongoing.backgroundSyncInProgress) {
            delete ongoing.backgroundSyncInProgress
            delete ongoing.backgroundProgress
            ongoing = { ...ongoing, status: "ready", queuedCount: 0 }
            listener(ongoingStatus())
            return
          }
          ongoing = {
            ...ongoing,
            status: "ready",
            queuedCount: 0,
            automaticRecoveryCount: 0,
            fullSyncInProgress: false,
            uploadProgress: {
              uploaded: rollingProgress ? 5 : 8,
              total: rollingProgress ? 5 : 8,
              transfer: { sentBytes: 85_000_000, totalBytes: 85_000_000 },
            },
          }
          delete ongoing.uploadTransfer
          listener(ongoingStatus())
        }
        const recoverTransfer = () => {
          ongoing = {
            ...ongoing,
            status: "ready",
            queuedCount: 1,
            automaticRecoveryCount: 1,
            retryAvailable: false,
          }
          delete ongoing.uploadTransfer
          listener(ongoingStatus())
        }
        const failTransfer = () => {
          ongoing = { ...ongoing, status: "offline", queuedCount: 1 }
          listener(ongoingStatus())
        }
        const transferEvent = "cloud-fixture:update-transfer"
        const transferDoneEvent = "cloud-fixture:finish-transfer"
        const recoveryEvent = "cloud-fixture:recover-transfer"
        const failureEvent = "cloud-fixture:fail-transfer"
        const updateQueue = () => {
          showQueueProgress = !showQueueProgress
          listener(ongoingStatus())
        }
        const queueEvent = "cloud-fixture:update-queue"
        window.addEventListener(event, finish)
        window.addEventListener(queueEvent, updateQueue)
        window.addEventListener(transferEvent, updateTransfer)
        window.addEventListener(transferDoneEvent, finishTransfer)
        window.addEventListener(recoveryEvent, recoverTransfer)
        window.addEventListener(failureEvent, failTransfer)
        return () => {
          window.removeEventListener(event, finish)
          window.removeEventListener(queueEvent, updateQueue)
          window.removeEventListener(transferEvent, updateTransfer)
          window.removeEventListener(transferDoneEvent, finishTransfer)
          window.removeEventListener(recoveryEvent, recoverTransfer)
          window.removeEventListener(failureEvent, failTransfer)
        }
      },
      groups: async (_binding, after) => {
        folderReadCount++
        if (deferFolders || partialSettingsFolders || pagedSettingsFolders)
          document.body.dataset.folderReadCalls = String(folderReadCount)
        if (folderReadCount <= Number(params.get("folderDiscovery") ?? 0))
          throw new Error("Synthetic folder read failure")
        if (refreshFailures > 0) {
          refreshFailures--
          throw new Error("Synthetic refresh failure")
        }
        if (deferFolders) {
          if (folderReleasePending) folderReleasePending = false
          else await new Promise<void>((resolve) => pendingFolderReads.add(resolve))
        }
        const emptyPage =
          ["folders-empty-partial", "folders-empty-paged"].includes(name) && !after
        const partialSettingsPage = partialSettingsFolders && folderReadCount === 1
        return {
          groups: emptyPage
            ? []
            : hideStudio
              ? groups.filter(({ id }) => id !== "Studio")
              : pagedSettingsFolders
                ? after
                  ? groups.slice(1)
                  : groups.slice(0, 1)
                : groups,
          sourceRoots:
            emptyPage ||
            (name === "settings-missing-sources" &&
              (!partialSettingsFolders || partialSettingsPage))
              ? []
              : groups.flatMap((group) =>
                  group.projects.map((project) => ({
                    sourceKind: project.sourceKind,
                    path: project.sourcePath,
                  })),
                ),
          nextCursor:
            (name === "folders-empty-paged" || pagedSettingsFolders) && !after
              ? { environment: "native", cwd: "/Example/Next" }
              : null,
          partial: name === "folders-empty-partial" || partialSettingsPage,
        }
      },
      status: async () => {
        if (params.has("deferStatus"))
          await new Promise<void>((resolve) =>
            window.addEventListener("cloud-fixture:finish-status", () => resolve(), {
              once: true,
            }),
          )
        return ongoingStatus()
      },
      confirmTeamFolders: async ({
        accountId,
        destinationId,
        expectedPolicyRevision,
        folderPaths,
        additions,
      }) => {
        const team = ongoing.teamStatuses?.[0]
        if (
          !team ||
          team.accountId !== accountId ||
          team.destinationId !== destinationId ||
          team.policyRevision !== expectedPolicyRevision ||
          JSON.stringify(folderPaths) !==
            JSON.stringify(team.savedFolders?.map(({ path }) => path))
        )
          throw new Error("Unexpected synthetic Team confirmation.")
        if (failTeamSave) {
          failTeamSave = false
          if (params.has("deferSaveFailure"))
            await new Promise<void>((resolve) => {
              window.addEventListener("cloud-fixture:finish-save", () => resolve(), {
                once: true,
              })
            })
          throw new Error("Synthetic save failure.")
        }
        const added = Object.values(additions ?? {}).flat()
        if (
          additions &&
          JSON.stringify(added) !==
            JSON.stringify(groups.find(({ id }) => id === "Website")?.projects)
        )
          throw new Error("Unexpected synthetic Team additions.")
        const saved = {
          ...team,
          status: "ready" as const,
          storageKnown: true,
          folderConfirmationRequired: false,
          policyRevision: "fixture-renewed-team",
          projects: [...team.projects, ...added],
          savedFolders: [
            ...(team.savedFolders ?? []),
            ...added.map(({ projectPath: path }) => ({ groupId: `saved-${path}`, path })),
          ],
        }
        ongoing = { ...ongoing, teamStatuses: [saved] }
        return saved
      },
      enable: async (consent) => {
        if (firstPersonalSaveFailure) {
          personalSaveCalls += 1
          document.body.dataset.personalSaveCalls = String(personalSaveCalls)
          if (personalSaveCalls === 1) throw new Error("Synthetic Personal save failure.")
        }
        if (name === "ambiguous") throw new Error("Synthetic unknown result")
        ongoing = {
          ...ongoing,
          storageKnown: true,
          consentVersion: consent.version,
          policyRevision: `fixture-approved-${++projectWrites}`,
          deviceUploadEnabled: ongoing.deviceUploadEnabled ?? true,
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
      setProject: async ({ destinationId, groupId, selected, projects }) => {
        const paths = new Set(groups.find((group) => group.id === groupId)?.folderPaths)
        if (name === "settings-team-confirmation" && destinationId === "synthetic-team") {
          const team = ongoing.teamStatuses?.[0]
          if (!team || selected) throw new Error("Unexpected synthetic Team folder change.")
          const saved = {
            ...team,
            policyRevision: "fixture-moved-team",
            projects: team.projects.filter(({ projectPath }) => !paths.has(projectPath)),
            projectChoices: { ...team.projectChoices, [groupId]: false },
            savedFolders: (team.savedFolders ?? []).filter(({ path }) => !paths.has(path)),
          }
          ongoing = { ...ongoing, teamStatuses: [saved] }
          return saved
        }
        const next = ongoing.projects.filter((project) => !paths.has(project.projectPath))
        ongoing = {
          ...ongoing,
          policyRevision: `fixture-approved-${++projectWrites}`,
          projects: selected ? [...next, ...projects] : next,
          projectChoices: { ...ongoing.projectChoices, [groupId]: selected },
        }
        return ongoing
      },
      setEnabled: async (enabled) => {
        ongoing = {
          ...ongoing,
          deviceUploadEnabled: enabled,
          paused: !enabled,
          status: enabled ? "ready" : "paused",
        }
        return ongoing
      },
      retry: async () => {
        ongoing = { ...ongoing, paused: false, status: "ready" }
        return ongoing
      },
      backfill: async ({ accountId, destinationId }, expectedPolicyRevision) => {
        const policy =
          destinationId === ongoing.destinationId
            ? ongoing
            : ongoing.teamStatuses?.find((team) => team.destinationId === destinationId)
        if (policy?.accountId !== accountId || policy.policyRevision !== expectedPolicyRevision)
          throw new Error("Unexpected synthetic sync policy.")
        document.body.dataset.syncCalls = String(++syncCalls)
        if (rollingProgress) {
          ongoing = {
            ...ongoing,
            status: "syncing",
            queuedCount: 1,
            backfillInProgress: true,
            fullSyncInProgress: true,
            uploadProgress: {
              uploaded: 1,
              found: 5,
              total: null,
              transfer: { sentBytes: 12_000_000, totalBytes: null },
            },
            uploadTransfer: { sentBytes: 7_000_000, totalBytes: 10_000_000 },
          }
          return ongoing
        }
        if (name !== "settings-attention") return policy
        ongoing = {
          ...ongoing,
          backfillInProgress: true,
          uploadProgress: {
            uploaded: 2,
            found: 436,
            total: null,
            transfer: { sentBytes: 0, totalBytes: null },
          },
        }
        return ongoing
      },
    },
    read: async () => {
      if (params.has("deferAccount"))
        await new Promise<void>((resolve) =>
          window.addEventListener("cloud-fixture:finish-account", () => resolve(), {
            once: true,
          }),
        )
      return value
    },
    watch: async () => {
      const releaseFolders = () => {
        // Keep an early release for the next read. A remount can replace this watcher.
        if (folderWatcher !== releaseFolders) return
        if (pendingFolderReads.size === 0) folderReleasePending = true
        releasePendingFolderReads()
      }
      const recheckFolders = () => {
        hideStudio = hideGroupedFolderOnRefresh
        refreshFailures = Number(params.get("refreshFailures") ?? 0)
        refreshFolders?.()
      }
      if (deferFolders) {
        folderWatcher = releaseFolders
        window.addEventListener("cloud-fixture:recheck-folders", recheckFolders)
        window.addEventListener("cloud-fixture:finish-folders", releaseFolders)
      }
      return () => {
        window.removeEventListener("cloud-fixture:recheck-folders", recheckFolders)
        window.removeEventListener("cloud-fixture:finish-folders", releaseFolders)
        if (folderWatcher !== releaseFolders) return
        folderWatcher = undefined
        releasePendingFolderReads()
        folderReleasePending = false
      }
    },
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
    name === "settings-team-confirmation" ||
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
  const [session] = useState(() => {
    const session = new CloudSession(
      cloudFixture(name, () => void session.loadProjects()),
      false,
      true,
      "automatic",
    )
    return session
  })
  const value = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const storageUnknown = cloudUploadStatusUnknown(value.ongoing)
  const hasSavedFolders = [value.ongoing, ...Object.values(value.teamStatuses ?? {})].some(
    (policy) =>
      policy?.consentVersion === "desktop-project-sync-v1" && policy.projects.length > 0,
  )
  const canChangeUploads =
    hasSavedFolders &&
    !storageUnknown &&
    value.deviceUploadEnabled !== undefined &&
    (!value.busy || (value.canPauseBackfill && value.deviceUploadEnabled))
  return (
    <main
      className="h-full overflow-y-auto bg-surface-window p-6 text-label"
      aria-label="Cloud Settings preview"
    >
      <div className="mx-auto max-w-3xl">
        <SectionGroup title="Antiburn Cloud">
          <Card divided={false}>
            <CloudAccount session={session} value={value}>
              {canChangeUploads ? (
                <SettingsToggleRow
                  searchId="cloudUpload"
                  description={
                    value.deviceUploadEnabled === false
                      ? "Uploads are paused on this device."
                      : undefined
                  }
                  checked={value.deviceUploadEnabled === true}
                  onChange={(enabled) => void session.controlSync(!enabled, value)}
                />
              ) : (
                <SettingsRow searchId="cloudUpload" />
              )}
            </CloudAccount>
          </Card>
        </SectionGroup>
      </div>
    </main>
  )
}
