import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { SessionMemoryTouch } from "../../../lib/memoriesIpc"
import { SessionMemories } from "./SessionMemories"

afterEach(cleanup)

function touch(over: Partial<SessionMemoryTouch> = {}): SessionMemoryTouch {
  return {
    slug: "-work",
    path: "/h/.claude/projects/-work/memory/a.md",
    fileName: "a.md",
    title: "Alpha",
    action: "referenced",
    count: 1,
    lastMs: null,
    exists: true,
    ...over,
  }
}

describe("SessionMemories", () => {
  it("renders nothing without entries", () => {
    const { container, rerender } = render(<SessionMemories sessionMemories={null} />)
    expect(container).toBeEmptyDOMElement()
    rerender(<SessionMemories sessionMemories={{ entries: [] }} />)
    expect(container).toBeEmptyDOMElement()
  })

  it("labels actions and counts and shows a dash for a missing time", () => {
    render(
      <SessionMemories
        sessionMemories={{
          entries: [
            touch(),
            touch({
              path: "/b.md",
              fileName: "b.md",
              title: "Beta",
              action: "written",
              count: 3,
            }),
          ],
        }}
      />,
    )
    expect(screen.getByText("Memories touched")).toBeTruthy()
    expect(screen.getByText("read")).toBeTruthy()
    expect(screen.getByText("written ×3")).toBeTruthy()
    expect(screen.getAllByText("—")).toHaveLength(2)
  })

  it("opens a memory that still exists and marks a deleted one", () => {
    const onOpenMemory = vi.fn()
    render(
      <SessionMemories
        sessionMemories={{
          entries: [touch(), touch({ path: "/gone.md", fileName: "gone.md", exists: false })],
        }}
        onOpenMemory={onOpenMemory}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Show in Memories" }))
    expect(onOpenMemory).toHaveBeenCalledWith({
      slug: "-work",
      path: "/h/.claude/projects/-work/memory/a.md",
    })
    expect(screen.getByText("deleted")).toBeTruthy()
    expect(screen.getAllByRole("button")).toHaveLength(1)
  })

  it("shows no open button without a handler", () => {
    render(<SessionMemories sessionMemories={{ entries: [touch()] }} />)
    expect(screen.queryByRole("button")).toBeNull()
  })
})
