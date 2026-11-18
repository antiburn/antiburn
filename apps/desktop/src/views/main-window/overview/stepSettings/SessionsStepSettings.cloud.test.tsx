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
) {
  let status = ongoingStatus({
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
  invoke.mockImplementation(async (command: string) => {
    switch (command) {
      case "app_info":
        return { appVersion: "0.1.0", cloudEnabled: true, indexedSessions: 0, databaseBytes: 0 }
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
      case "cloud_project_groups":
        return {
          groups: [projectGroup()],
          sourceRoots: [
            { sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" },
          ],
          nextCursor: null,
          partial: false,
        }
      case "cloud_pause_sync":
        status = { ...status, status: "paused", paused: true }
        return status
      case "cloud_retry_sync":
        status = { ...status, status: "ready", paused: false }
        return status
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
  })
}

beforeEach(() => {
  vi.clearAllMocks()
})

it("does not turn an old true upload preference into ongoing content consent", async () => {
  mockCloud(false, true)
  render(<SessionsStepSettings />)
  await screen.findByRole("switch", { name: "Sync Studio: /Example/Studio" })
  const upload = screen.getByRole("switch", { name: "Upload sessions" })
  expect(upload).not.toBeChecked()
  expect(upload).toHaveAttribute("aria-disabled", "true")
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
  expect(
    invoke.mock.calls.some(
      ([command]) => command === "cloud_enable_sync" || command === "set_settings",
    ),
  ).toBe(false)
  expect(screen.getByRole("switch", { name: "Include folders without Git" })).not.toBeChecked()
})

it("uses the existing upload switch to pause and resume the saved desktop policy", async () => {
  mockCloud(true, false)
  const { container } = render(<SessionsStepSettings />)
  const upload = () => screen.getByRole("switch", { name: "Upload sessions" })
  await waitFor(() => expect(upload()).toBeChecked())
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
    expect(invoke).toHaveBeenCalledWith("cloud_pause_sync", {
      binding: { accountId: "synthetic-account", destinationId: "synthetic-personal" },
    }),
  )
  await waitFor(() => expect(upload()).not.toBeChecked())
  await waitFor(() => expect(upload()).not.toHaveAttribute("aria-disabled", "true"))
  fireEvent.click(upload())
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("cloud_retry_sync", {
      binding: { accountId: "synthetic-account", destinationId: "synthetic-personal" },
    }),
  )
  await waitFor(() => expect(upload()).toBeChecked())
  expect(
    invoke.mock.calls.some(
      ([command]) => command === "set_settings" || command === "cloud_enable_sync",
    ),
  ).toBe(false)
  const controls = [...container.querySelectorAll("[data-settings-control]")].map((node) =>
    node.getAttribute("data-settings-control"),
  )
  expect(controls.indexOf("cloudUpload")).toBeLessThan(controls.indexOf("sourceRepositories"))
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
  const upload = screen.getByRole("switch", { name: "Upload sessions" })
  expect(upload).toBeChecked()
  expect(upload).toHaveAttribute("aria-disabled", "true")
  expect(screen.getByRole("button", { name: "Sign out" })).toBeEnabled()
  expect(screen.getByRole("button", { name: "Retry saved uploads" })).toBeEnabled()
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
  mockCloud(true, false, { status: "syncing", fencedCount: 2 })
  render(<SessionsStepSettings />)
  expect(await screen.findByText("Could not resume saved uploads.")).toBeVisible()
  await screen.findByRole("switch", { name: "Sync Studio: /Example/Studio" })
  fireEvent.click(screen.getByRole("button", { name: "Retry saved uploads" }))
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("cloud_retry_sync", {
      binding: { accountId: "synthetic-account", destinationId: "synthetic-personal" },
    }),
  )
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Retry saved uploads" })).toBeEnabled(),
  )
  expect(screen.getAllByText("Could not resume saved uploads.")).toHaveLength(1)
  expect(
    screen.queryByText(/uploaded ·|Transferred|All sessions uploaded/),
  ).not.toBeInTheDocument()
  expect(
    invoke.mock.calls.some(
      ([command]) => command === "cloud_enable_sync" || command === "set_settings",
    ),
  ).toBe(false)
})
