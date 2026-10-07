import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { entry, project, report } from "./memoriesFixtures"
import { MemoriesSession, type MemoriesAdapter } from "./MemoriesSession"
import { MemoriesView } from "./MemoriesView"
import type { AgentMemoriesReport } from "../../../lib/memoriesIpc"
import type * as IpcModule from "../../../lib/ipc"

vi.mock("../../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  noteInteraction: vi.fn(),
}))

const NOW = 1_000_000 + 3 * 60 * 1000
const HOUR = 3_600_000

function setup(overrides: Partial<MemoriesAdapter> = {}) {
  const adapter: MemoriesAdapter = {
    listMemories: vi.fn().mockResolvedValue(report()),
    reveal: vi.fn().mockResolvedValue(undefined),
    now: vi.fn(() => NOW),
    ...overrides,
  }
  const session = new MemoriesSession(adapter)
  const view = render(<MemoriesView active session={session} />)
  return { adapter, session, view }
}

afterEach(() => {
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

  it("hides a collapsed project's rows", async () => {
    setup()
    fireEvent.click(await screen.findByRole("button", { name: /~\/p/ }))
    expect(screen.queryByRole("button", { name: /Alpha/ })).toBeNull()
  })
})
