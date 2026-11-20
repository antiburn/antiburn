import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import { emptyCloudSnapshot, type CloudOngoingStatus } from "../../../../lib/cloud"
import { DEFAULT_SETTINGS } from "../../../../lib/ipc"
import {
  ongoingStatus,
  projectGroup,
  localToolsStatus,
} from "../../../../onboarding/__tests__/cloudProjectFixtures"
import { SessionsStepSettings } from "./SessionsStepSettings"

const invoke = vi.hoisted(() => vi.fn())
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }))
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn(), open: vi.fn() }))

function mockCloud(
  consented: boolean,
  legacyPreference: boolean,
  statusPatch: Partial<CloudOngoingStatus> = {},
  currentTeamRevision?: string,
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
            groups: [projectGroup()],
            sourceRoots: [
              { sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" },
            ],
            nextCursor: null,
            partial: false,
          }
        case "cloud_set_sync_enabled": {
          const enabled = args?.enabled
          if (enabled === undefined) throw new Error("Expected a device upload setting")
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
            folderConfirmationRequired: false,
            policyRevision: "renewed-policy",
          }
          status = {
            ...status,
            teamStatuses: teams.map((item) => (item === team ? saved : item)),
          }
          return saved
        }
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

it("does not turn an old true upload preference into ongoing content consent", async () => {
  mockCloud(false, true)
  render(<SessionsStepSettings />)
  const folder = await screen.findByRole("switch", { name: "Sync Studio: /Example/Studio" })
  const upload = screen.getByRole("switch", { name: "Upload sessions to Antiburn Cloud" })
  expect(upload).not.toBeChecked()
  expect(upload).toBeEnabled()
  expect(upload).toHaveAccessibleDescription("Choose folders and confirm before uploading.")
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  expect(
    screen.queryByText(
      "Choose projects and confirm full-content sync before enabling uploads.",
    ),
  ).not.toBeInTheDocument()
  expect(
    screen.queryByText(
      "Cloud sync is optional. Local sessions and Checks use their own settings.",
    ),
  ).not.toBeInTheDocument()
  fireEvent.click(upload)
  expect(folder).toHaveFocus()
  expect(upload).not.toBeChecked()
  expect(
    invoke.mock.calls.some(
      ([command]) => command === "cloud_enable_sync" || command === "set_settings",
    ),
  ).toBe(false)
  expect(screen.getByRole("switch", { name: "Include folders without Git" })).not.toBeChecked()
})

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
  const upload = () => screen.getByRole("switch", { name: "Upload sessions to Antiburn Cloud" })
  const teamFolder = await screen.findByRole("switch", {
    name: "Sync Studio: /Example/Studio to Example Team",
  })
  await waitFor(() => expect(upload()).toBeChecked())
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
  expect(teamFolder).toBeChecked()
  await waitFor(() => expect(upload()).not.toHaveAttribute("aria-disabled", "true"))
  fireEvent.click(upload())
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("cloud_set_sync_enabled", { enabled: true }),
  )
  await waitFor(() => expect(upload()).toBeChecked())
  expect(teamFolder).toBeChecked()
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
    const confirmation = await screen.findByRole("button", {
      name: "Save",
    })
    const upload = screen.getByRole("switch", { name: "Upload sessions to Antiburn Cloud" })
    expect(upload).toHaveAttribute("aria-checked", String(enabled))
    expect(screen.getByText("Save the listed Team folders.")).toBeVisible()
    for (const path of folderPaths)
      expect(screen.getByRole("switch", { name: `Sync ${path} to Example Team` })).toBeChecked()
    const writes = () =>
      invoke.mock.calls.filter(([command]) =>
        [
          "cloud_enable_sync",
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
      await waitFor(() => expect(confirmation).toBeDisabled())
      await waitFor(() =>
        expect(screen.getByRole("group", { name: "Cloud folders" })).toHaveFocus(),
      )
    }
    expect(upload).toHaveAttribute("aria-checked", String(enabled))
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
  expect(await screen.findByText("Could not read saved uploads.")).toBeVisible()
  await screen.findByRole("switch", { name: "Sync Studio: /Example/Studio" })
  const upload = screen.getByRole("switch", { name: "Upload sessions to Antiburn Cloud" })
  expect(upload).toBeChecked()
  expect(upload).toHaveAttribute("aria-disabled", "true")
  expect(screen.getByRole("button", { name: "Sign out" })).toBeEnabled()
  expect(screen.getByRole("button", { name: "Check saved uploads" })).toBeEnabled()
  expect(screen.queryByText("Last known setting.")).not.toBeInTheDocument()
  expect(
    screen.queryByText(
      "Cloud sync is optional. Local sessions and Checks use their own settings.",
    ),
  ).not.toBeInTheDocument()
  expect(screen.queryByText(/uploaded ·|Transferred/)).not.toBeInTheDocument()
  fireEvent.click(upload)
  expect(
    invoke.mock.calls.some(
      ([command]) =>
        command === "cloud_enable_sync" ||
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

it("focuses the device upload switch instead of sending a Retry while uploads are off", async () => {
  mockCloud(true, false, {
    status: "paused",
    paused: true,
    fencedCount: 2,
    retryAvailable: false,
    deviceUploadEnabled: false,
  })
  render(<SessionsStepSettings />)
  fireEvent.click(await screen.findByRole("button", { name: "Go to upload switch" }))
  const upload = screen.getByRole("switch", { name: "Upload sessions to Antiburn Cloud" })
  expect(upload).toHaveFocus()
  expect(upload).not.toBeChecked()
  expect(screen.queryByRole("button", { name: "Retry saved uploads" })).not.toBeInTheDocument()
  expect(
    invoke.mock.calls.some(
      ([command]) => command === "cloud_retry_sync" || command === "cloud_set_sync_enabled",
    ),
  ).toBe(false)
})
