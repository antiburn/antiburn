import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { expect, it, vi } from "vitest"
import {
  emptyCloudSnapshot,
  CLOUD_BUSY_MESSAGE,
  desktopCloudPort,
  type CloudPort,
  type CloudProjectGroup,
  type CloudSnapshot,
} from "../lib/cloud"
import { CloudOnboarding, CloudProjectFolders } from "./CloudOnboarding"
import { localToolsFixture } from "./__tests__/cloudProjectFixtures"

const analytics = vi.hoisted(() => ({ action: vi.fn(), view: vi.fn() }))
vi.mock("../lib/onboardingAnalytics", () => ({
  noteCloudAction: analytics.action,
  noteCloudScreenViewed: analytics.view,
}))

function fixture(patch: Partial<CloudSnapshot> = {}) {
  let notify: (next: CloudSnapshot) => void = () => {}
  let value: CloudSnapshot = {
    ...emptyCloudSnapshot,
    authEnabled: true,
    captureEnabled: true,
    status: "signed_in",
    identity: { id: "synthetic", login: "Example" },
    destination: { id: "personal", name: "Personal", kind: "personal" },
    draft: {
      screen: "folders",
      folders: [{ path: "/Example/claude/projects/project", selected: true }],
    },
    ...patch,
  }
  const start = vi.fn<CloudPort["start"]>(async () => ({ status: "upload_unavailable" }))
  const port: CloudPort = {
    ...desktopCloudPort,
    ongoing: undefined,
    read: async () => value,
    watch: async (listener) => {
      notify = listener
      return () => {}
    },
    localTools: localToolsFixture().port,
    prepareSync: vi.fn(async () => {}),
    start,
    reconcile: async () => value.pendingSync?.result ?? { status: "upload_unavailable" },
    save: async (draft) => {
      value = { ...value, draft }
      return value
    },
  }
  return {
    port,
    start,
    emit: (patch: Partial<CloudSnapshot> = {}) => {
      value = { ...value, ...patch }
      notify(value)
    },
  }
}

it("saves desktop capture requests with truthful completion copy", async () => {
  const { port, start } = fixture()
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  const save = await screen.findByRole("button", { name: "Save capture request" })
  expect(save).toBeEnabled()
  expect(screen.getByText(/Choose the folders where Claude saves sessions/)).toBeVisible()
  expect(screen.queryByText(/existing and future coding sessions/)).not.toBeInTheDocument()
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  fireEvent.click(save)
  expect(await screen.findByRole("heading", { name: "Capture request saved" })).toBeVisible()
  expect(start).toHaveBeenCalledOnce()
  expect(screen.getByText("Upload unavailable")).toBeVisible()
  expect(screen.queryByText(/Sessions syncing/)).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Done" }))
  expect(done).toHaveBeenCalledOnce()
})

it("does not claim a desktop request was saved when its result is unknown", async () => {
  const { port } = fixture()
  vi.mocked(port.prepareSync).mockRejectedValue(new Error("save response lost"))
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const save = await screen.findByRole("button", { name: "Save capture request" })
  fireEvent.click(save)
  await waitFor(() => expect(screen.getByText("Capture request not confirmed")).toBeVisible())
  expect(
    screen.queryByRole("heading", { name: "Capture request saved" }),
  ).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled()
})

it("keeps source-listing recovery out of onboarding", async () => {
  const { port, start } = fixture()
  start.mockRejectedValueOnce(new Error("listing failed"))
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const save = await screen.findByRole("button", { name: "Save capture request" })
  fireEvent.click(save)
  expect(await screen.findByRole("heading", { name: "Capture request saved" })).toBeVisible()
  expect(screen.queryByRole("button", { name: /Retry/ })).not.toBeInTheDocument()
  expect(screen.queryByText(/Could not finish listing sources/)).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled()
  expect(start).toHaveBeenCalledOnce()
  expect(vi.mocked(port.prepareSync)).toHaveBeenCalledOnce()
})

it("keeps Done available after a background busy rejection", async () => {
  const { port, start, emit } = fixture()
  port.prepareSync = vi.fn(async (_folders, requestId) => {
    emit({ pendingSync: { requestId, result: { status: "upload_unavailable" } } })
  })
  start.mockRejectedValueOnce(CLOUD_BUSY_MESSAGE)
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const save = await screen.findByRole("button", { name: "Save capture request" })
  fireEvent.click(save)
  await waitFor(() => expect(start).toHaveBeenCalledOnce())
  expect(screen.queryByText(CLOUD_BUSY_MESSAGE)).not.toBeInTheDocument()
  act(() => emit())
  expect(screen.queryByText(CLOUD_BUSY_MESSAGE)).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled()
  expect(port.prepareSync).toHaveBeenCalledOnce()
  expect(screen.queryByRole("button", { name: /Retry/ })).not.toBeInTheDocument()
})

it("keeps sync wording for simulated accepted operations", async () => {
  const { port, start } = fixture({ captureEnabled: false, syncEnabled: true })
  start.mockResolvedValue({
    status: "accepted",
    operationId: "simulated",
    completed: 0,
    total: 1,
  })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const sync = await screen.findByRole("button", { name: "Start syncing" })
  fireEvent.click(sync)
  expect(
    await screen.findByRole("heading", { name: "Sessions sync in the background" }),
  ).toBeVisible()
  expect(screen.getByText("Syncing 1 folder")).toBeVisible()
  expect(screen.queryByText("Upload unavailable")).not.toBeInTheDocument()
  expect(screen.queryByText(/not repositories/)).not.toBeInTheDocument()
  expect(screen.queryByText(/Ask your agent/)).not.toBeInTheDocument()
  expect(screen.getByText("Local tools unavailable")).toBeVisible()
})

it("keeps Done available while desktop preparation is pending", async () => {
  const { port, start } = fixture()
  port.prepareSync = vi.fn(() => new Promise<void>(() => {}))
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  const save = await screen.findByRole("button", { name: "Save capture request" })
  fireEvent.click(save)
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled()
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: /Retry|Remove|Cancel sync/ }),
  ).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Done" }))
  expect(done).toHaveBeenCalledOnce()
  expect(start).not.toHaveBeenCalled()
})

it("does not claim syncing when desktop capture is unavailable on the platform", async () => {
  const { port } = fixture({
    captureEnabled: false,
    syncEnabled: false,
    setupError: "Desktop capture is not supported on this platform.",
    pendingSync: { requestId: "saved", result: { status: "upload_unavailable" } },
  })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(
    await screen.findByRole("heading", { name: "Continue using Antiburn locally" }),
  ).toBeVisible()
  expect(screen.getByText("Desktop capture is not supported on this platform.")).toBeVisible()
  expect(screen.queryByText(/Sessions syncing/)).not.toBeInTheDocument()
})

it.each([{ draft: { screen: "folders" as const, folders: [] } }, {}])(
  "allows local completion without source or content consent: %j",
  async (patch) => {
    const { port, start } = fixture(patch)
    const done = vi.fn()
    render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
    const local = await screen.findByRole("button", { name: "Continue locally" })
    fireEvent.click(local)
    expect(done).toHaveBeenCalledOnce()
    expect(port.prepareSync).not.toHaveBeenCalled()
    expect(start).not.toHaveBeenCalled()
  },
)

it("names unsupported capture after a fresh connection without trying to save navigation", async () => {
  const { port, start } = fixture({
    captureEnabled: false,
    setupError: "Desktop capture is not supported on this platform.",
    draft: { screen: "handoff", folders: [] },
  })
  port.save = vi.fn()
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  expect(
    await screen.findByText(/Desktop capture is not supported on this platform/),
  ).toBeVisible()
  fireEvent.click(screen.getByRole("button", { name: "Continue locally" }))
  expect(done).toHaveBeenCalledOnce()
  expect(port.save).not.toHaveBeenCalled()
  expect(start).not.toHaveBeenCalled()
})

it("allows local completion while configured Cloud is offline", async () => {
  const { port } = fixture({ status: "offline", identity: null, destination: null })
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  fireEvent.click(await screen.findByRole("button", { name: "Continue locally" }))
  expect(done).toHaveBeenCalledOnce()
  expect(port.prepareSync).not.toHaveBeenCalled()
})

it("records a local choice without reporting request or upload success", async () => {
  const { port } = fixture()
  render(<CloudOnboarding port={port} onBack={vi.fn()} onComplete={vi.fn()} />)
  fireEvent.click(await screen.findByRole("button", { name: "Continue locally" }))
  expect(analytics.action).toHaveBeenCalledExactlyOnceWith("folders", "use_locally")
  expect(port.prepareSync).not.toHaveBeenCalled()
})

it("cancels only the pending browser attempt before local completion", async () => {
  const { port } = fixture({ status: "waiting", identity: null, destination: null })
  port.cancel = vi.fn(async () => ({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "revocation_unconfirmed" as const,
    revocationPending: true,
  }))
  port.cancelSync = vi.fn()
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  fireEvent.click(await screen.findByRole("button", { name: "Continue locally" }))
  await waitFor(() => expect(done).toHaveBeenCalledOnce())
  expect(port.cancel).toHaveBeenCalledOnce()
  expect(port.cancelSync).not.toHaveBeenCalled()
  expect(port.prepareSync).not.toHaveBeenCalled()
})

it("offers explicit OpenCode database selection and stored-content consent without upload", async () => {
  const { port, start } = fixture({ draft: { screen: "folders", folders: [] } })
  port.pick = vi.fn(async () => ["/Example/opencode/opencode.db"])
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  fireEvent.click(await screen.findByRole("button", { name: "Add OpenCode databases…" }))
  await waitFor(() =>
    expect(screen.getByText("OpenCode V2: /Example/opencode/opencode.db")).toBeVisible(),
  )
  expect(port.pick).toHaveBeenCalledWith("opencode_sqlite_v2")
  const save = screen.getByRole("button", { name: "Save capture request" })
  expect(save).toBeEnabled()
  fireEvent.click(save)
  expect(await screen.findByRole("heading", { name: "Capture request saved" })).toBeVisible()
  expect(port.prepareSync).toHaveBeenCalledWith(expect.any(Array), expect.any(String), {
    version: "native-full-content-v2",
    accountId: "synthetic",
    destinationId: "personal",
    fullContent: true,
    attachments: false,
    selectedSources: [
      { sourceKind: "opencode_sqlite_v2", path: "/Example/opencode/opencode.db" },
    ],
  })
  expect(start).toHaveBeenCalledOnce()
  expect(screen.getByText("Upload unavailable")).toBeVisible()
})

const projectGroups: readonly CloudProjectGroup[] = [
  {
    id: "studio",
    name: "Studio",
    path: "/Example/Studio",
    memberPaths: [
      "/Example/Studio",
      "/Example/worktrees/studio-fix",
      "/Example/external/studio-review",
    ],
    sessionCount: 12,
    worktreeCount: 2,
  },
  {
    id: "website",
    name: "Website",
    path: "/Example/Website",
    memberPaths: ["/Example/Website"],
    sessionCount: 1,
    worktreeCount: 0,
  },
]

it("keeps one existing-style row and selection control per project group", () => {
  const select = vi.fn()
  render(<CloudProjectFolders groups={projectGroups} choices={{}} onSelect={select} />)
  expect(screen.getByText("Studio")).toBeVisible()
  expect(screen.getByText("/Example/Studio")).toBeVisible()
  expect(screen.getByText(/12 sessions/)).toBeVisible()
  expect(screen.getByText("2 worktrees")).toBeVisible()
  expect(screen.getByText("1 session")).toBeVisible()
  expect(screen.getAllByRole("switch")).toHaveLength(2)
  for (const control of screen.getAllByRole("switch")) expect(control).toBeChecked()
  expect(select).not.toHaveBeenCalled()
  expect(screen.queryByText(/database|Claude source|OpenCode/i)).not.toBeInTheDocument()
  expect(screen.getByText("/Example/Studio")).toHaveAttribute("title", projectGroups[0]!.path)
  fireEvent.click(screen.getByRole("switch", { name: "Sync Studio: /Example/Studio" }))
  expect(select).toHaveBeenCalledExactlyOnceWith("studio", false)
})

it("preserves an explicit group opt-out while new groups show selected without a write", () => {
  const select = vi.fn()
  const choices = { studio: false }
  const { rerender } = render(
    <CloudProjectFolders groups={[projectGroups[0]!]} choices={choices} onSelect={select} />,
  )
  rerender(<CloudProjectFolders groups={projectGroups} choices={choices} onSelect={select} />)
  expect(screen.getByRole("switch", { name: /Sync Studio:/ })).not.toBeChecked()
  expect(screen.getByRole("switch", { name: /Sync Website:/ })).toBeChecked()
  expect(select).not.toHaveBeenCalled()
})

it("does not create child controls when worktrees or recorded CWD members change", () => {
  const select = vi.fn()
  const { rerender } = render(
    <CloudProjectFolders groups={projectGroups} choices={{}} onSelect={select} />,
  )
  const updated = projectGroups.map((project) =>
    project.id === "studio"
      ? {
          ...project,
          memberPaths: [...project.memberPaths, "/Example/new-cwd"],
          sessionCount: 13,
        }
      : project,
  )
  rerender(<CloudProjectFolders groups={updated} choices={{}} onSelect={select} />)
  expect(screen.getAllByRole("switch")).toHaveLength(2)
  expect(screen.getByText("13 sessions")).toBeVisible()
  expect(screen.getByText("/Example/Studio")).toHaveAttribute("title", updated[0]!.path)
  expect(select).not.toHaveBeenCalled()
})
