import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { entry, project, report } from "./memoriesFixtures"
import { MemoriesSession, type MemoriesAdapter } from "./MemoriesSession"
import { MemoriesView } from "./MemoriesView"
import type { AgentMemoriesReport } from "../../../lib/memoriesIpc"
import type * as IpcModule from "../../../lib/ipc"

const noteInteraction = vi.hoisted(() => vi.fn())
vi.mock("../../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  noteInteraction,
}))

const NOW = 1_000_000 + 3 * 60 * 1000
const HOUR = 3_600_000

function setup(overrides: Partial<MemoriesAdapter> = {}) {
  const adapter: MemoriesAdapter = {
    listMemories: vi.fn().mockResolvedValue(report()),
    reveal: vi.fn().mockResolvedValue(undefined),
    archive: vi.fn().mockResolvedValue({
      outcome: "archived",
      archiveId: "1-a.md",
      indexLineRemoved: true,
    }),
    restore: vi.fn().mockResolvedValue({ outcome: "restored", indexLineRestored: true }),
    removeIndexLine: vi.fn().mockResolvedValue({ outcome: "indexLineRemoved" }),
    noteInteraction,
    now: vi.fn(() => NOW),
    ...overrides,
  }
  const session = new MemoriesSession(adapter)
  const view = render(<MemoriesView active session={session} />)
  return { adapter, session, view }
}

afterEach(() => {
  noteInteraction.mockClear()
  localStorage.clear()
  cleanup()
})

describe("MemoriesView", () => {
  it("shows a loading status first", () => {
    setup({ listMemories: vi.fn(() => new Promise<AgentMemoriesReport>(() => undefined)) })
    expect(screen.getByRole("status")).toHaveTextContent("Loading Memories.")
  })

  it("shows an error with a Retry that loads again", async () => {
    const { adapter } = setup({ listMemories: vi.fn().mockRejectedValue(new Error("no")) })
    expect(await screen.findByRole("alert")).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    expect(adapter.listMemories).toHaveBeenCalledTimes(2)
  })

  it("shows the empty state", async () => {
    setup({ listMemories: vi.fn().mockResolvedValue(report([])) })
    expect(await screen.findByText("No agent memories found")).toBeVisible()
  })

  it("summarises the report in a status line", async () => {
    setup()
    expect(await screen.findByText(/1 memory in 1 project · updated 3m ago/)).toBeVisible()
  })

  it("shows the project path, count, and attention count", async () => {
    setup({
      listMemories: vi.fn().mockResolvedValue(
        report([
          project({
            dangling: [{ title: "Gone", target: "gone.md", lineNumber: 4 }],
            memories: [entry({ inIndex: false })],
          }),
        ]),
      ),
    })
    const header = await screen.findByRole("button", { name: /~\/p/ })
    expect(header).toHaveTextContent("2 need attention")
    expect(header).toHaveTextContent("3 sessions")
  })

  it("lists dangling index entries and a missing index", async () => {
    setup({
      listMemories: vi
        .fn()
        .mockResolvedValue(
          report([
            project({ dangling: [{ title: "Gone", target: "gone.md", lineNumber: 4 }] }),
            project({ slug: "-q", displayPath: "~/q", indexPath: null }),
          ]),
        ),
    })
    expect(await screen.findByText("1 entry points to a missing file")).toBeVisible()
    expect(screen.getByText("Gone → gone.md")).toBeVisible()
    expect(screen.getByText(/No MEMORY.md index/)).toBeVisible()
  })

  it("renders a row with its title, hook, kind, and facts", async () => {
    setup({
      listMemories: vi.fn().mockResolvedValue(
        report([
          project({
            memories: [
              entry({
                facts: {
                  ...entry().facts,
                  lastReferencedMs: NOW - 2 * HOUR,
                  lastWrittenMs: NOW - 48 * HOUR,
                  sessionsSinceWritten: 7,
                },
              }),
              entry({
                path: "/p/memory/b.md",
                title: "Beta",
                hook: "Second hook",
                kind: null,
                facts: { ...entry().facts, hasHistory: false },
              }),
            ],
          }),
        ]),
      ),
    })
    const row = (await screen.findByRole("button", { name: /Alpha/ })) as HTMLElement
    expect(row).toHaveTextContent("Alpha hook")
    expect(row).toHaveTextContent("feedback")
    expect(row).toHaveTextContent("2h")
    expect(row).toHaveTextContent("2d")
    expect(row).toHaveTextContent("7")
    const noHistory = screen.getByRole("button", { name: /Beta/ })
    expect(noHistory).toHaveAttribute("title", "No session history recorded for this memory")
    expect(noHistory.textContent?.match(/—/g)).toHaveLength(3)
  })

  it("opens a row to its body and flags a memory missing from the index", async () => {
    setup({
      listMemories: vi
        .fn()
        .mockResolvedValue(
          report([project({ memories: [entry({ inIndex: false, hookSource: "body" })] })]),
        ),
    })
    const row = await screen.findByRole("button", { name: /Alpha/ })
    fireEvent.click(row)
    expect(row).toHaveAttribute("aria-expanded", "true")
    expect(screen.getByText("Alpha body").tagName).toBe("PRE")
    expect(screen.getByText(/Not in the index/)).toBeVisible()
    expect(screen.getByText("Hook from first line")).toBeVisible()
    expect(screen.getByText(/a\.md · 2\.0 KB · frontmatter yes/)).toBeVisible()
  })

  it("reveals the memory path", async () => {
    const { adapter } = setup()
    fireEvent.click(await screen.findByRole("button", { name: /Alpha/ }))
    const panel = screen.getByText("Alpha body").parentElement!
    fireEvent.click(
      within(panel).getByRole("button", { name: /Finder|File Explorer|file manager/ }),
    )
    expect(adapter.reveal).toHaveBeenCalledWith("/p/memory/a.md")
  })

  it("keeps several rows open at once", async () => {
    setup({
      listMemories: vi.fn().mockResolvedValue(
        report([
          project({
            memories: [
              entry(),
              entry({
                path: "/p/memory/b.md",
                title: "Beta",
                hook: "Second hook",
                body: "Beta body",
              }),
            ],
          }),
        ]),
      ),
    })
    fireEvent.click(await screen.findByRole("button", { name: /Alpha/ }))
    fireEvent.click(screen.getByRole("button", { name: /Beta/ }))
    expect(screen.getByText("Alpha body")).toBeVisible()
    expect(screen.getByText("Beta body")).toBeVisible()
  })

  it("focuses the requested row once and clears the request", async () => {
    const scroll = vi.fn()
    Element.prototype.scrollIntoView = scroll
    const { session } = setup()
    await screen.findByRole("button", { name: /Alpha/ })
    act(() => session.focus("-p", "/p/memory/a.md"))
    const row = screen.getByRole("button", { name: /Alpha/ })
    expect(row).toHaveFocus()
    expect(row).toHaveAttribute("aria-expanded", "true")
    expect(scroll).toHaveBeenCalledTimes(1)
    expect(scroll).toHaveBeenCalledWith({ block: "center" })
    expect(session.getSnapshot().focusRequest).toBeNull()
  })

  it("hides a collapsed project's rows", async () => {
    setup()
    fireEvent.click(await screen.findByRole("button", { name: /~\/p/ }))
    expect(screen.queryByRole("button", { name: /Alpha/ })).toBeNull()
  })

  it("measures a reveal", async () => {
    setup()
    fireEvent.click(await screen.findByRole("button", { name: /Alpha/ }))
    const panel = screen.getByText("Alpha body").parentElement!
    fireEvent.click(
      within(panel).getByRole("button", { name: /Finder|File Explorer|file manager/ }),
    )
    await waitFor(() =>
      expect(noteInteraction).toHaveBeenCalledWith({
        kind: "memoryAction",
        action: "reveal",
        outcome: "succeeded",
      }),
    )
  })
})

describe("MemoriesView editing", () => {
  const twoMemories = report([
    project({
      memories: [
        entry(),
        entry({
          path: "/p/memory/b.md",
          fileName: "b.md",
          title: "Beta",
          hook: "Second hook",
          body: "Beta body",
        }),
      ],
    }),
  ])

  async function openAlpha() {
    fireEvent.click(await screen.findByRole("button", { name: /Alpha/ }))
  }

  it("deletes a memory, strikes the row, and offers Undo", async () => {
    const { adapter } = setup({ listMemories: vi.fn().mockResolvedValue(twoMemories) })
    const header = await screen.findByRole("button", { name: /~\/p/ })
    expect(header).toHaveTextContent("~/p2")
    await openAlpha()
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(adapter.archive).toHaveBeenCalledWith("-p", "a.md", 2048, null)
    expect(await screen.findByText("Deleted · moved to antiburn's archive")).toBeVisible()
    expect(screen.getByRole("button", { name: "Undo" })).toBeVisible()
    expect(screen.getByText("Alpha")).toHaveClass("line-through")
    expect(screen.queryByText("Alpha body")).toBeNull()
    expect(header).toHaveTextContent("~/p1")
    expect(screen.getByText(/1 memory in 1 project/)).toBeVisible()
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "memoryAction",
      action: "archive",
      outcome: "succeeded",
    })
  })

  it("undoes a delete by restoring and reloading", async () => {
    const { adapter } = setup({ listMemories: vi.fn().mockResolvedValue(twoMemories) })
    await openAlpha()
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    fireEvent.click(await screen.findByRole("button", { name: "Undo" }))
    await waitFor(() => expect(adapter.restore).toHaveBeenCalledWith("-p", "1-a.md"))
    await waitFor(() => expect(adapter.listMemories).toHaveBeenCalledTimes(2))
    await waitFor(() => expect(screen.queryByRole("button", { name: "Undo" })).toBeNull())
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "memoryAction",
      action: "restore",
      outcome: "succeeded",
    })
  })

  it("shows a change on disk with a Reload and does not strike the row", async () => {
    const { adapter } = setup({
      archive: vi.fn().mockResolvedValue({ outcome: "changedOnDisk" }),
    })
    await openAlpha()
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    const alert = await screen.findByRole("alert")
    expect(alert).toHaveTextContent(
      "This memory changed on disk. Reload to see the current state.",
    )
    expect(screen.getByText("Alpha")).not.toHaveClass("line-through")
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "memoryAction",
      action: "archive",
      outcome: "changed_on_disk",
    })
    fireEvent.click(within(alert).getByRole("button", { name: "Reload" }))
    expect(adapter.listMemories).toHaveBeenCalledTimes(2)
  })

  it("shows a failure when the delete is rejected", async () => {
    setup({ archive: vi.fn().mockRejectedValue(new Error("boom")) })
    await openAlpha()
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not delete this memory.")
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "memoryAction",
      action: "archive",
      outcome: "failed",
    })
    expect(noteInteraction).not.toHaveBeenCalledWith(
      expect.objectContaining({ action: "archive", outcome: "succeeded" }),
    )
  })

  it("offers no Delete or Remove buttons where writes are unsupported", async () => {
    setup({
      listMemories: vi.fn().mockResolvedValue({
        ...report([
          project({ dangling: [{ title: "Gone", target: "gone.md", lineNumber: 4 }] }),
        ]),
        writesSupported: false,
      }),
    })
    await openAlpha()
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Remove line" })).toBeNull()
    expect(screen.getByText("Editing memories is not supported on Windows yet.")).toBeVisible()
  })

  it("removes a dangling line and notes the backup", async () => {
    const { adapter } = setup({
      listMemories: vi
        .fn()
        .mockResolvedValue(
          report([
            project({ dangling: [{ title: "Gone", target: "gone.md", lineNumber: 4 }] }),
          ]),
        ),
    })
    fireEvent.click(await screen.findByRole("button", { name: "Remove line" }))
    expect(adapter.removeIndexLine).toHaveBeenCalledWith("-p", 4, "gone.md")
    await waitFor(() => expect(screen.queryByText("Gone → gone.md")).toBeNull())
    expect(screen.getByText("Backup written to MEMORY.md.antiburn-bak")).toBeVisible()
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "memoryAction",
      action: "remove_index_line",
      outcome: "succeeded",
    })
  })

  it("shows fresh rows after the view is left and entered again", async () => {
    const { session, view } = setup({ listMemories: vi.fn().mockResolvedValue(twoMemories) })
    await openAlpha()
    fireEvent.click(screen.getByRole("button", { name: "Delete" }))
    await screen.findByRole("button", { name: "Undo" })
    view.rerender(<MemoriesView active={false} session={session} />)
    view.rerender(<MemoriesView active session={session} />)
    await waitFor(() => expect(screen.queryByRole("button", { name: "Undo" })).toBeNull())
    expect(screen.getByText("Alpha")).not.toHaveClass("line-through")
  })
})
