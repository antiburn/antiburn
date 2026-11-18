import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import { emptyCloudSnapshot } from "../../../../lib/cloud"
import { DEFAULT_SETTINGS } from "../../../../lib/ipc"
import {
  ongoingStatus,
  projectGroup,
} from "../../../../onboarding/__tests__/cloudProjectFixtures"
import { SessionsStepSettings } from "./SessionsStepSettings"

const invoke = vi.hoisted(() => vi.fn())
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }))
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn(), open: vi.fn() }))

function mockCloud(consented: boolean, legacyPreference: boolean) {
  let status = ongoingStatus(
    consented ? { status: "ready", consentVersion: "desktop-project-sync-v1" } : {},
  )
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
  expect(screen.getByRole("checkbox")).not.toBeChecked()
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
