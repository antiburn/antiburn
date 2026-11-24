import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import { emptyCloudSnapshot, type CloudOngoingStatus } from "../../../../lib/cloud"
import { DEFAULT_SETTINGS } from "../../../../lib/ipc"
import { CloudSession } from "../../../../onboarding/CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
  localToolsStatus,
} from "../../../../onboarding/__tests__/cloudProjectFixtures"
import { SessionsStepSettings } from "./SessionsStepSettings"

const invoke = vi.hoisted(() => vi.fn())
const uploadLabel = "Upload sessions to Antiburn Cloud"
const studioLabel = "Sync Studio: /Example/Studio"
const personalBinding = { accountId: "synthetic-account", destinationId: "synthetic-personal" }
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }))
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn(), open: vi.fn() }))

function mockCloud(
  consented: boolean,
  legacyPreference: boolean,
  statusPatch: Partial<CloudOngoingStatus> = {},
  currentTeamRevision?: string,
  groups = [projectGroup()],
) {
  let status = ongoingStatus({
    deviceUploadEnabled: true,
    ...(consented
      ? {
          status: "ready",
          consentVersion: "desktop-project-sync-v1",
          projects: projectGroup().projects,
          projectChoices: { studio: true },
        }
      : {}),
    ...statusPatch,
  })
  invoke.mockImplementation(
    async (
      command: string,
      args?: {
        enabled?: boolean
        binding?: { accountId: string; destinationId: string }
        expectedPolicyRevision?: string
        confirmation?: {
          accountId: string
          destinationId: string
          expectedPolicyRevision: string
          folderPaths: string[]
        }
      },
    ) => {
      switch (command) {
        case "app_info":
          return {
            appVersion: "0.1.0",
            cloudEnabled: true,
            indexedSessions: 0,
            databaseBytes: 0,
          }
        case "get_settings":
          return {
            ...DEFAULT_SETTINGS,
            cloudUploadEnabled: legacyPreference,
            includeNonRepoFolders: false,
          }
        case "cloud_status":
          return {
            ...emptyCloudSnapshot,
            authEnabled: true,
            captureEnabled: true,
            syncEnabled: consented,
            status: "signed_in",
            identity: { id: "synthetic-account", login: "Example" },
            destination: { id: "synthetic-personal", name: "Personal", kind: "personal" },
          }
        case "get_local_tools_status":
          return localToolsStatus()
        case "cloud_sync_status":
          return status
        case "cloud_workspaces":
          return [
            { id: "synthetic-personal", kind: "personal", name: "Personal" },
            ...(status.teamStatuses?.map((team) => ({
              id: team.destinationId,
              kind: "team",
              name: team.workspaceName,
            })) ?? []),
          ]
        case "cloud_project_groups":
          return {
            groups,
            sourceRoots: [
              { sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" },
            ],
            nextCursor: null,
            partial: false,
          }
        case "cloud_set_sync_enabled": {
          const enabled = args?.enabled
          if (enabled === undefined) throw new Error("Expected a device upload setting")
          if (
            enabled &&
            (args?.binding?.accountId !== personalBinding.accountId ||
              args?.binding?.destinationId !== personalBinding.destinationId)
          )
            throw new Error("Expected the Personal account binding to resume uploads")
          status = { ...status, deviceUploadEnabled: enabled }
          return status
        }
        case "cloud_reapprove_team_sync": {
          const confirmation = args?.confirmation
          const teams = status.teamStatuses ?? []
          const team = teams.find((item) => item.destinationId === confirmation?.destinationId)
          if (
            !team ||
            confirmation?.expectedPolicyRevision !==
              (currentTeamRevision ?? team.policyRevision)
          )
            throw new Error("Synthetic policy revision changed.")
          const saved = {
            ...team,
            status: "ready" as const,
            storageKnown: true,
            folderConfirmationRequired: false,
            policyRevision: "renewed-policy",
          }
          status = {
            ...status,
            teamStatuses: teams.map((item) => (item === team ? saved : item)),
          }
          return saved
        }
        case "cloud_backfill_sync":
          return (
            status.teamStatuses?.find(
              (team) => team.destinationId === args?.binding?.destinationId,
            ) ?? status
          )
        case "cloud_retry_sync": {
          const teams = status.teamStatuses ?? []
          const team = teams.find((item) => item.destinationId === args?.binding?.destinationId)
          if (team) {
            if (args?.expectedPolicyRevision !== (currentTeamRevision ?? team.policyRevision))
              throw new Error(
                "Cloud folder choices changed. Reload before retrying saved work.",
              )
            const retried = { ...team, status: "ready" as const, retryAvailable: false }
            status = {
              ...status,
              teamStatuses: teams.map((item) => (item === team ? retried : item)),
            }
            return retried
          }
          status = { ...status, status: "ready", paused: false }
          return status
        }
        case "get_scan_status":
          return {
            running: false,
            completedAgents: 0,
            totalAgents: 0,
            sessions: 0,
            finishedAt: null,
            cancelled: false,
            error: null,
            agents: [],
          }
        case "get_folder_permissions":
          return { supported: false, deferred: [], granted: [] }
        case "list_scan_roots":
        case "list_repositories":
        case "refresh_repositories":
        case "get_agent_integrations":
          return []
        default:
          return null
      }
    },
  )
}

beforeEach(() => {
  vi.clearAllMocks()
})

it("shows placeholders instead of upload controls while checking Cloud availability", async () => {
  mockCloud(true, false)
  const invokeCloud = invoke.getMockImplementation()!
  let finishCloudRead!: () => void
  invoke.mockImplementation((...args) => {
    if (args[0] !== "cloud_status") return invokeCloud(...args)
    return new Promise((resolve) => {
      finishCloudRead = () => resolve(invokeCloud(...args))
    })
  })
  render(<SessionsStepSettings />)
  expect(await screen.findByText("Loading Cloud settings…")).toBeVisible()
  expect(screen.queryByRole("switch", { name: uploadLabel })).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Choose folders" })).not.toBeInTheDocument()

  await act(async () => finishCloudRead())
  expect(await screen.findByRole("switch", { name: uploadLabel })).toBeChecked()
})

it("focuses the first folder with an upload choice without saving or enabling uploads", async () => {
  mockCloud(false, true, {}, undefined, [
    projectGroup({
      id: "inaccessible",
      label: "Inaccessible",
      path: "/Example/Inaccessible",
      folderPaths: [],
      projects: [],
      captureSupported: false,
      groupingComplete: false,
    }),
    projectGroup({
      id: "local-only",
      label: "Local only",
      path: "/Example/Local",
      folderPaths: ["/Example/Local"],
      projects: [],
      captureSupported: false,
    }),
    projectGroup(),
    projectGroup({
      id: "website",
      label: "Website",
      path: "/Example/Website",
      folderPaths: ["/Example/Website"],
      projects: [
        {
          sourceKind: "claude_jsonl",
          sourcePath: "/Example/claude/projects/studio",
          projectPath: "/Example/Website",
        },
      ],
    }),
  ])
  render(<SessionsStepSettings />)
  const folder = await screen.findByRole("switch", { name: studioLabel })
  const folders = within(screen.getByRole("group", { name: "Cloud folders" }))
  expect(folders.queryByText("Inaccessible")).not.toBeInTheDocument()
  expect(folders.queryByText("/Example/Inaccessible")).not.toBeInTheDocument()
  expect(folders.getByText("No sessions ready to upload")).toBeVisible()
  expect(folders.getAllByRole("switch")).toEqual([
    folder,
    folders.getByRole("switch", { name: "Sync Website: /Example/Website" }),
  ])
  expect(screen.queryByRole("switch", { name: uploadLabel })).not.toBeInTheDocument()
  expect(screen.getByText("Uploads have not started on this device.")).toBeVisible()
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Choose folders" })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeEnabled()
  expect(screen.queryByRole("switch", { name: uploadLabel })).not.toBeInTheDocument()
  expect(
    invoke.mock.calls.filter(([command]) =>
      [
        "cloud_enable_sync",
        "cloud_set_sync_enabled",
        "cloud_set_sync_project",
        "set_settings",
      ].includes(command),
    ),
  ).toEqual([])
  expect(screen.getByRole("switch", { name: "Include folders without Git" })).not.toBeChecked()
})

it.each(["pause", "scan"])(
  "keeps pause available during manual sync when %s settles first",
  async (first) => {
    const saved = ongoingStatus({
      status: "ready",
      deviceUploadEnabled: true,
      consentVersion: "desktop-project-sync-v1",
      projects: projectGroup().projects,
      projectChoices: { studio: true },
    })
    mockCloud(true, false, saved)
    const invokeCloud = invoke.getMockImplementation()!
    let finishBackfill!: () => void
    let finishPause!: () => void
    invoke.mockImplementation((...args) => {
      const [command, options] = args
      if (command === "cloud_backfill_sync")
        return new Promise((resolve, reject) => {
          finishBackfill = () =>
            first === "pause"
              ? resolve({ ...saved, backfillInProgress: true })
              : reject(new Error("Cloud account changed. Saved work is kept."))
        })
      if (command === "cloud_set_sync_enabled" && options?.enabled === false)
        return new Promise((resolve) => {
          finishPause = () => resolve(invokeCloud(...args))
        })
      return invokeCloud(...args)
    })

    render(<SessionsStepSettings />)
    await screen.findByRole("switch", { name: uploadLabel })
    const sync = await screen.findByRole("button", { name: "Sync now" })
    const upload = () => screen.getByRole("switch", { name: uploadLabel })
    const folder = screen.getByRole("switch", { name: studioLabel })
    const expectDisabledFolderSwitch = () => {
      expect(screen.getByRole("switch", { name: studioLabel })).toBe(folder)
      expect(folder).toBeChecked()
      expect(folder).toHaveAttribute("aria-disabled", "true")
    }
    fireEvent.click(sync)
    expect(screen.getByText("Starting sync", { exact: true })).toBeVisible()
    expect(screen.getByRole("button", { name: "Syncing" })).toBeDisabled()
    expect(upload()).toBeChecked()
    expect(upload()).toBeEnabled()
    expectDisabledFolderSwitch()
    expect(screen.queryByText("Updating…")).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Sign out" })).toBeDisabled()

    fireEvent.click(upload())
    expect(invoke).toHaveBeenCalledWith("cloud_set_sync_enabled", { enabled: false })
    expect(screen.queryByRole("switch", { name: uploadLabel })).not.toBeInTheDocument()
    expect(screen.getByText("Updating uploads…")).toBeVisible()
    const [finishFirst, finishLast] =
      first === "pause" ? [finishPause, finishBackfill] : [finishBackfill, finishPause]
    await act(async () => finishFirst())
    expect(screen.queryByRole("switch", { name: uploadLabel })).not.toBeInTheDocument()
    expectDisabledFolderSwitch()
    await act(async () => finishLast())

    expect(upload()).not.toBeChecked()
    expect(upload()).toBeEnabled()
    expect(screen.getByRole("switch", { name: studioLabel })).toBe(folder)
    expect(folder).toBeChecked()
    expect(folder).not.toHaveAttribute("aria-disabled")
    expect(screen.queryByText("Scanning folders…")).not.toBeInTheDocument()
    expect(screen.queryByText(/Could not start sync/)).not.toBeInTheDocument()
    expect(screen.getByText("Uploads are paused on this device.")).toBeVisible()
  },
)

it("pauses and resumes Team uploads without requiring Personal folders", async () => {
  mockCloud(false, false, {
    teamStatuses: [
      ongoingStatus({
        destinationId: "synthetic-team",
        workspaceName: "Example Team",
        consentVersion: "desktop-project-sync-v1",
        projects: projectGroup().projects,
        status: "ready",
      }),
    ],
  })
  const { container } = render(<SessionsStepSettings />)
  const upload = () => screen.getByRole("switch", { name: uploadLabel })
  const teamFolder = await screen.findByRole("combobox", {
    name: "Sync Studio: /Example/Studio",
  })
  const sync = screen.getByRole("button", { name: "Sync now" })
  await waitFor(() => expect(upload()).toBeChecked())
  expect(teamFolder).toHaveValue("synthetic-team")
  expect(sync).toBeEnabled()
  expect(screen.getByText("2 workspaces")).toBeVisible()
  expect(screen.queryByRole("button", { name: "Start syncing" })).not.toBeInTheDocument()
  expect(
    screen.queryByText(
      "Upload existing and new sessions from the approved projects. Pausing keeps saved uploads.",
    ),
  ).not.toBeInTheDocument()
  expect(
    screen.queryByText(
      "Cloud sync is optional. Local sessions and Checks use their own settings.",
    ),
  ).not.toBeInTheDocument()
  fireEvent.click(upload())
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("cloud_set_sync_enabled", { enabled: false }),
  )
  await waitFor(() => expect(upload()).not.toBeChecked())
  expect(screen.getByText("Uploads are paused on this device.")).toBeVisible()
  expect(teamFolder).toHaveValue("synthetic-team")
  expect(sync).toBeVisible()
  expect(sync).toBeDisabled()
  await waitFor(() => expect(upload()).not.toHaveAttribute("aria-disabled", "true"))
  fireEvent.click(upload())
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("cloud_set_sync_enabled", {
      enabled: true,
      binding: personalBinding,
    }),
  )
  await waitFor(() => expect(upload()).toBeChecked())
  expect(teamFolder).toHaveValue("synthetic-team")
  expect(sync).toBeEnabled()
  expect(screen.queryByRole("button", { name: "Start syncing" })).not.toBeInTheDocument()
  expect(
    invoke.mock.calls.some(
      ([command]) =>
        command === "set_settings" ||
        command === "cloud_enable_sync" ||
        command === "cloud_retry_sync",
    ),
  ).toBe(false)
  const controls = [...container.querySelectorAll("[data-settings-control]")].map((node) =>
    node.getAttribute("data-settings-control"),
  )
  expect(controls.indexOf("cloudUpload")).toBeLessThan(controls.indexOf("sourceRepositories"))
})

it("can pause and revoke during a setup error but cannot resume or approve new folders", async () => {
  mockCloud(false, false)
  const { port, ongoing, emit } = cloudProjectFixture()
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
  })
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", name: "Personal", kind: "personal" as const },
    { id: "synthetic-team", name: "Example Team", kind: "team" as const },
  ])
  ongoing.status.mockResolvedValue(saved)
  ongoing.setEnabled.mockResolvedValue({
    ...saved,
    status: "paused",
    paused: true,
    deviceUploadEnabled: false,
  })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(() => {})
  const { unmount } = render(<SessionsStepSettings sharedCloudSession={session} />)
  await screen.findByRole("combobox", { name: studioLabel })
  act(() => emit({ setupError: "Could not read Cloud setup." }))

  const pause = screen.getByRole("switch", { name: uploadLabel })
  expect(pause).toBeChecked()
  expect(screen.queryByRole("combobox", { name: studioLabel })).not.toBeInTheDocument()
  expect(screen.getByRole("switch", { name: /Sync Studio: .* to Personal/ })).toBeChecked()
  const sync = screen.getByRole("button", { name: "Sync now" })
  expect(sync.matches(":disabled") || sync.getAttribute("aria-disabled") === "true").toBe(true)
  fireEvent.click(pause)
  await waitFor(() => expect(session.getSnapshot().deviceUploadEnabled).toBe(false))
  expect(screen.queryByRole("switch", { name: uploadLabel })).not.toBeInTheDocument()
  expect(ongoing.setEnabled).toHaveBeenCalledExactlyOnceWith(false)

  fireEvent.click(screen.getByRole("switch", { name: /Sync Studio: .* to Personal/ }))
  await waitFor(() => expect(ongoing.setProject).toHaveBeenCalledOnce())
  expect(ongoing.setProject).toHaveBeenCalledWith(expect.objectContaining({ selected: false }))
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(ongoing.backfill).not.toHaveBeenCalled()
  unmount()
  stop()
})

it.each([
  { fails: false, enabled: true },
  { fails: true, enabled: true },
  { fails: false, enabled: false },
])(
  "confirms all saved Team folders in one action (save fails: $fails, uploads on: $enabled)",
  async ({ fails, enabled }) => {
    const folderPaths = ["/Example/Studio", "/Example/Unavailable"]
    const team = ongoingStatus({
      destinationId: "synthetic-team",
      workspaceName: "Example Team",
      consentVersion: "desktop-project-sync-v1",
      folderConfirmationRequired: true,
      policyRevision: "saved-revision",
      storageKnown: false,
      status: "reauth_required",
      savedFolders: folderPaths.map((path, index) => ({
        groupId: `saved-folder-${index}`,
        path,
      })),
      projects: folderPaths.map((projectPath) => ({
        ...projectGroup().projects[0]!,
        projectPath,
      })),
    })
    mockCloud(
      false,
      false,
      { teamStatuses: [team], deviceUploadEnabled: enabled },
      fails ? "new-revision" : undefined,
    )
    render(<SessionsStepSettings />)
    expect(await screen.findByText("Sync now approves the listed Team folders.")).toBeVisible()
    const confirmation = await screen.findByRole("button", {
      name: "Sync now",
    })
    const upload = () => screen.getByRole("switch", { name: uploadLabel })
    expect(upload()).toHaveAttribute("aria-checked", String(enabled))
    const manageSaved = screen.getByText("Manage other saved folders")
    const disclosure = manageSaved.closest("details")
    expect(disclosure).not.toHaveAttribute("open")
    for (const path of folderPaths)
      expect(within(disclosure!).getByText(path)).not.toBeVisible()
    fireEvent.click(manageSaved)
    expect(disclosure).toHaveAttribute("open")
    for (const path of folderPaths)
      expect(
        within(disclosure!).getByRole("switch", { name: `Sync ${path} to Example Team` }),
      ).toBeChecked()
    const writes = () =>
      invoke.mock.calls.filter(([command]) =>
        [
          "cloud_enable_sync",
          "cloud_backfill_sync",
          "cloud_reapprove_team_sync",
          "cloud_retry_sync",
          "cloud_set_sync_enabled",
          "cloud_set_sync_project",
        ].includes(command),
      )
    expect(writes()).toEqual([])

    confirmation.focus()
    fireEvent.click(confirmation)
    await waitFor(() =>
      expect(writes()).toEqual([
        [
          "cloud_reapprove_team_sync",
          {
            confirmation: {
              accountId: "synthetic-account",
              destinationId: team.destinationId,
              expectedPolicyRevision: team.policyRevision,
              folderPaths,
            },
          },
        ],
        ...(!fails && enabled
          ? [
              [
                "cloud_backfill_sync",
                {
                  binding: {
                    accountId: "synthetic-account",
                    destinationId: team.destinationId,
                  },
                  expectedPolicyRevision: "renewed-policy",
                },
              ],
            ]
          : []),
      ]),
    )
    if (fails) {
      expect(await screen.findByRole("alert")).toHaveTextContent(
        "Could not save folders. Try again.",
      )
      expect(confirmation).toHaveFocus()
      expect(confirmation).toBeEnabled()
      for (const path of folderPaths)
        expect(
          screen.getByRole("switch", { name: `Sync ${path} to Example Team` }),
        ).toBeChecked()
    } else {
      if (enabled) {
        await waitFor(() => expect(confirmation).toBeEnabled())
        expect(confirmation).toHaveFocus()
      } else {
        await waitFor(() => expect(confirmation).toBeDisabled())
        await waitFor(() =>
          expect(screen.getByRole("group", { name: "Cloud folders" })).toHaveFocus(),
        )
      }
    }
    expect(upload()).toHaveAttribute("aria-checked", String(enabled))
    expect(screen.getByText("Signed in as Example").closest("div")).not.toHaveTextContent(
      /could not/i,
    )
  },
)

it("retries only the chosen verified Team and only on explicit activation", async () => {
  mockCloud(false, false, {
    teamStatuses: [
      ongoingStatus({
        destinationId: "synthetic-team",
        workspaceName: "Example Team",
        consentVersion: "desktop-project-sync-v1",
        projects: projectGroup().projects,
        status: "interrupted",
        retryAvailable: true,
      }),
      ongoingStatus({
        destinationId: "other-team",
        workspaceName: "Other Team",
        consentVersion: "desktop-project-sync-v1",
        projects: [{ ...projectGroup().projects[0]!, projectPath: "/Example/Other" }],
        status: "fenced",
        retryAvailable: true,
      }),
    ],
  })
  render(<SessionsStepSettings />)
  const retry = await screen.findByRole("button", {
    name: "Retry saved uploads to Example Team",
  })
  const retryCalls = () =>
    invoke.mock.calls.filter(([command]) => command === "cloud_retry_sync")
  expect(retryCalls()).toEqual([])
  retry.focus()
  fireEvent.click(retry)
  await waitFor(() => expect(retry).not.toBeInTheDocument())
  await waitFor(() =>
    expect(screen.getByRole("group", { name: "Cloud folders" })).toHaveFocus(),
  )
  expect(
    screen.getByRole("button", { name: "Retry saved uploads to Other Team" }),
  ).toBeEnabled()
  expect(retryCalls()).toEqual([
    [
      "cloud_retry_sync",
      {
        binding: { accountId: "synthetic-account", destinationId: "synthetic-team" },
        expectedPolicyRevision: "synthetic-policy",
      },
    ],
  ])
  expect(invoke.mock.calls.some(([command]) => command === "cloud_set_sync_enabled")).toBe(
    false,
  )
})

it("shows a stale Team retry error without retrying the new revision", async () => {
  mockCloud(
    false,
    false,
    {
      teamStatuses: [
        ongoingStatus({
          destinationId: "synthetic-team",
          workspaceName: "Example Team",
          consentVersion: "desktop-project-sync-v1",
          projects: projectGroup().projects,
          policyRevision: "shown-team-policy",
          status: "interrupted",
          retryAvailable: true,
        }),
      ],
    },
    "new-team-policy",
  )
  render(<SessionsStepSettings />)
  fireEvent.click(
    await screen.findByRole("button", { name: "Retry saved uploads to Example Team" }),
  )
  const error = await screen.findByText(
    "Cloud folder choices changed. Reopen Settings before retrying.",
  )
  expect(error.closest('[role="alert"]')).not.toBeNull()
  expect(invoke.mock.calls.filter(([command]) => command === "cloud_retry_sync")).toEqual([
    [
      "cloud_retry_sync",
      {
        binding: { accountId: "synthetic-account", destinationId: "synthetic-team" },
        expectedPolicyRevision: "shown-team-policy",
      },
    ],
  ])
})

it.each([
  { retryAvailable: false, paused: false, deviceUploadEnabled: true },
  { retryAvailable: true, paused: true, deviceUploadEnabled: true },
  { retryAvailable: true, paused: false, deviceUploadEnabled: false },
  { retryAvailable: true, paused: false, deviceUploadEnabled: true, policyRevision: null },
])("does not offer blocked Team retry: %o", async ({ deviceUploadEnabled, ...team }) => {
  mockCloud(false, false, {
    deviceUploadEnabled,
    teamStatuses: [
      ongoingStatus({
        destinationId: "synthetic-team",
        workspaceName: "Example Team",
        consentVersion: "desktop-project-sync-v1",
        projects: projectGroup().projects,
        status: "interrupted",
        ...team,
      }),
    ],
  })
  render(<SessionsStepSettings />)
  expect(await screen.findByText("2 workspaces")).toBeVisible()
  expect(screen.queryByRole("button", { name: /Retry saved uploads/ })).not.toBeInTheDocument()
  expect(invoke.mock.calls.some(([command]) => command === "cloud_retry_sync")).toBe(false)
})

it("keeps unreadable saved-upload state visible without extra status paragraphs", async () => {
  mockCloud(true, false, {
    status: "storage_unavailable",
    storageKnown: false,
    acceptedCount: 5,
    fencedCount: 2,
  })
  render(<SessionsStepSettings />)
  expect(
    await screen.findByText("Could not read saved uploads. Saved files are kept."),
  ).toBeVisible()
  expect(screen.getByText("Could not check saved uploads on this device.")).toBeVisible()
  const folders = within(screen.getByRole("group", { name: "Cloud folders" }))
  expect(folders.getByText("Sync status unavailable")).toBeVisible()
  expect(folders.queryByRole("switch")).not.toBeInTheDocument()
  expect(screen.queryByRole("switch", { name: uploadLabel })).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Choose folders" })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Sign out" })).toBeEnabled()
  expect(screen.queryByRole("button", { name: "Check saved uploads" })).not.toBeInTheDocument()
  expect(screen.queryByText("Last known setting.")).not.toBeInTheDocument()
  expect(
    screen.queryByText(
      "Cloud sync is optional. Local sessions and Checks use their own settings.",
    ),
  ).not.toBeInTheDocument()
  expect(screen.queryByText(/uploaded ·|Transferred/)).not.toBeInTheDocument()
  expect(
    invoke.mock.calls.some(
      ([command]) =>
        command === "cloud_enable_sync" ||
        command === "cloud_set_sync_enabled" ||
        command === "cloud_set_sync_project" ||
        command === "cloud_retry_sync" ||
        command === "set_settings",
    ),
  ).toBe(false)
})

it("keeps blocked saved work visible after Retry without claiming acceptance", async () => {
  mockCloud(true, false, { status: "syncing", fencedCount: 2, retryAvailable: true })
  render(<SessionsStepSettings />)
  expect(await screen.findByText("Some saved uploads are blocked.")).toBeVisible()
  await screen.findByRole("switch", { name: "Sync Studio: /Example/Studio" })
  fireEvent.click(screen.getByRole("button", { name: "Retry saved uploads" }))
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("cloud_retry_sync", {
      binding: { accountId: "synthetic-account", destinationId: "synthetic-personal" },
      expectedPolicyRevision: "synthetic-policy",
    }),
  )
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Retry saved uploads" })).toBeEnabled(),
  )
  expect(screen.getAllByText("Some saved uploads are blocked.")).toHaveLength(1)
  expect(
    screen.queryByText(/uploaded ·|Transferred|All sessions uploaded/),
  ).not.toBeInTheDocument()
  expect(
    invoke.mock.calls.some(
      ([command]) => command === "cloud_enable_sync" || command === "set_settings",
    ),
  ).toBe(false)
})

it("focuses the paused upload switch and resumes only when activated", async () => {
  mockCloud(true, false, {
    status: "paused",
    paused: true,
    fencedCount: 2,
    retryAvailable: false,
    deviceUploadEnabled: false,
  })
  const invokeCloud = invoke.getMockImplementation()!
  let finishResume!: () => void
  invoke.mockImplementation((...args) => {
    if (args[0] !== "cloud_set_sync_enabled") return invokeCloud(...args)
    return new Promise((resolve) => {
      finishResume = () => resolve(invokeCloud(...args))
    })
  })
  render(<SessionsStepSettings />)
  fireEvent.click(await screen.findByRole("button", { name: "Go to upload switch" }))
  const upload = screen.getByRole("switch", { name: uploadLabel })
  expect(upload).toHaveFocus()
  expect(upload).not.toBeChecked()
  expect(screen.queryByRole("button", { name: "Retry saved uploads" })).not.toBeInTheDocument()
  expect(
    invoke.mock.calls.some(
      ([command]) => command === "cloud_retry_sync" || command === "cloud_set_sync_enabled",
    ),
  ).toBe(false)
  fireEvent.click(upload)
  expect(screen.getByText("Updating uploads…")).toBeVisible()
  expect(screen.queryByRole("switch", { name: uploadLabel })).not.toBeInTheDocument()
  await act(async () => finishResume())
  await waitFor(() => {
    const resumed = screen.getByRole("switch", { name: uploadLabel })
    expect(resumed).toBeChecked()
    expect(resumed).toHaveFocus()
  })
  expect(invoke).toHaveBeenCalledWith("cloud_set_sync_enabled", {
    enabled: true,
    binding: personalBinding,
  })
  expect(invoke.mock.calls.some(([command]) => command === "cloud_retry_sync")).toBe(false)
})
