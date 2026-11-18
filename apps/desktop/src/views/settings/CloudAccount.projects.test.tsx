import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { expect, it } from "vitest"
import { CloudSession } from "../../onboarding/CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "../../onboarding/__tests__/cloudProjectFixtures"
import { CloudAccount } from "./CloudAccount"

function Account({ session }: { session: CloudSession }) {
  const value = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  return <CloudAccount session={session} value={value} />
}

it("starts desktop ongoing uploads from Settings without using legacy request discovery", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("switch", { name: "Sync Studio: /Example/Studio" })
  const start = screen.getByRole("button", { name: "Start syncing" })
  expect(start).toBeDisabled()
  fireEvent.click(screen.getByRole("checkbox"))
  fireEvent.click(start)
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
  expect(port.prepareSync).not.toHaveBeenCalled()
  expect(port.start).not.toHaveBeenCalled()
  act(() =>
    emitSync(
      ongoingStatus({
        status: "syncing",
        consentVersion: "desktop-project-sync-v1",
        acceptedCount: 1,
        queuedCount: 2,
        progress: {
          negotiated: true,
          totalCompressedBytes: 2000,
          requiredTransferBytes: 1000,
          reusedBytes: 1000,
          confirmedUploadBytes: 500,
        },
      }),
    ),
  )
  expect(screen.getByText("Sessions syncing")).toBeVisible()
  expect(screen.getByText(/1 uploaded · 2 waiting/)).toBeVisible()
  expect(screen.getByText(/Transferred.*Already saved in Cloud/)).toBeVisible()
})

it("retries only the saved desktop policy when a new external worktree awaits confirmation", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("switch")
  fireEvent.click(screen.getByRole("checkbox"))
  fireEvent.click(screen.getByRole("button", { name: "Start syncing" }))
  await screen.findByRole("button", { name: "Retry saved uploads" })
  ongoing.status.mockResolvedValue(
    ongoingStatus({ status: "ready", consentVersion: "desktop-project-sync-v1" }),
  )
  ongoing.groups.mockResolvedValue({
    groups: [
      projectGroup({
        folderPaths: [...projectGroup().folderPaths, "/Example/external/new-worktree"],
        projects: [
          ...projectGroup().projects,
          {
            sourceKind: "claude_jsonl",
            sourcePath: "/Example/claude/projects/studio",
            projectPath: "/Example/external/new-worktree",
          },
        ],
        worktreeCount: 2,
      }),
    ],
    sourceRoots: [{ sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" }],
    nextCursor: null,
    partial: false,
  })
  fireEvent.click(screen.getByRole("button", { name: "Refresh folders" }))
  await waitFor(() => expect(screen.getByRole("checkbox")).not.toBeChecked())
  expect(screen.getByRole("button", { name: "Update sync selection" })).toBeDisabled()
  fireEvent.click(screen.getByRole("button", { name: "Retry saved uploads" }))
  await waitFor(() => expect(ongoing.retry).toHaveBeenCalledOnce())
  expect(ongoing.enable).toHaveBeenCalledOnce()
})

it("hides cached totals and progress after saved upload storage becomes unknown", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("switch")
  const healthy = ongoingStatus({
    status: "ready",
    storageKnown: true,
    consentVersion: "desktop-project-sync-v1",
    projectChoices: { studio: true },
    acceptedCount: 7,
    queuedCount: 2,
    progress: {
      negotiated: true,
      totalCompressedBytes: 2000,
      requiredTransferBytes: 1000,
      reusedBytes: 1000,
      confirmedUploadBytes: 500,
    },
  })
  act(() => emitSync(healthy))
  expect(screen.getByText(/7 uploaded · 2 waiting/)).toBeVisible()
  expect(screen.getByText(/Transferred/)).toBeVisible()
  act(() => emitSync({ ...healthy, status: "storage_unavailable", storageKnown: false }))
  expect(screen.getByText("Saved upload status is unavailable")).toBeVisible()
  expect(screen.queryByText(/7 uploaded|0 uploaded|Transferred/)).not.toBeInTheDocument()
  expect(screen.queryByText("Background sync enabled")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Retry saved uploads" })).toBeEnabled()
  expect(screen.getByRole("checkbox")).toBeDisabled()
  expect(screen.getByRole("button", { name: "Refresh folders" })).toBeEnabled()
  ongoing.retry.mockResolvedValue(healthy)
  fireEvent.click(screen.getByRole("button", { name: "Retry saved uploads" }))
  expect(await screen.findByText(/7 uploaded · 2 waiting/)).toBeVisible()
})
