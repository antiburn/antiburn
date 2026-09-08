import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { CollectionDetailPane, type CollectionItem } from "./CollectionDetailPane"

const items = [
  { id: "one", label: "First item", description: "First description" },
  { id: "two", label: "Second item", description: "Second description" },
  { id: "three", label: "Third item" },
]
function Workspace({ entries = items }: { entries?: CollectionItem[] }) {
  return (
    <CollectionDetailPane
      title="Collection"
      items={entries}
      emptyMessage="Nothing here"
      detailEmptyMessage="Select an item"
      renderDetail={(item) => <p>Content for {item.label}</p>}
    />
  )
}

describe("CollectionDetailPane", () => {
  it("reserves the detail region without auto-selecting or rendering detail", () => {
    const renderDetail = vi.fn()
    render(
      <CollectionDetailPane
        title="Collection"
        items={items}
        emptyMessage="Nothing here"
        detailEmptyMessage="Select an item"
        renderDetail={renderDetail}
      />,
    )
    expect(screen.getByRole("region", { name: "Details" })).toBeVisible()
    expect(screen.getByText("Select an item")).toBeVisible()
    expect(
      screen
        .getAllByRole("option")
        .filter((row) => row.getAttribute("aria-selected") === "true"),
    ).toHaveLength(0)
    expect(renderDetail).not.toHaveBeenCalled()
  })

  it("selects on click while keeping focus and the collection viewport", () => {
    const { container } = render(<Workspace />)
    const viewport = container.querySelector(".main-window-collection-scroll")
    const row = screen.getByRole("option", { name: "Second item" })
    fireEvent.click(row)
    expect(row).toHaveFocus()
    expect(row).toHaveAttribute("aria-selected", "true")
    expect(screen.getByText("Content for Second item")).toBeVisible()
    expect(container.querySelector(".main-window-collection-scroll")).toBe(viewport)
  })

  it("keeps arrow selection in the list and moves focus to detail with Enter", async () => {
    render(<Workspace />)
    const first = screen.getAllByRole("option")[0]!
    first.focus()
    fireEvent.keyDown(first, { key: "ArrowDown" })
    const second = screen.getAllByRole("option")[1]!
    expect(second).toHaveFocus()
    expect(second).toHaveAttribute("aria-selected", "true")
    fireEvent.keyDown(second, { key: "End" })
    const last = screen.getAllByRole("option")[2]!
    expect(last).toHaveFocus()
    fireEvent.keyDown(last, { key: "Home" })
    expect(first).toHaveFocus()
    fireEvent.keyDown(first, { key: "Enter" })
    await waitFor(() =>
      expect(screen.getByRole("region", { name: "First item" })).toHaveFocus(),
    )
  })

  it("tracks identity across reordering and preserves the detail when filtered out", () => {
    const { rerender } = render(<Workspace />)
    fireEvent.click(screen.getAllByRole("option")[1]!)
    rerender(
      <Workspace entries={[items[2]!, { ...items[1]!, label: "Updated item" }, items[0]!]} />,
    )
    expect(screen.getByRole("heading", { name: "Updated item" })).toBeVisible()
    expect(screen.getByRole("option", { name: "Updated item" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    rerender(<Workspace entries={[items[0]!]} />)
    expect(screen.getByRole("status")).toHaveTextContent("outside the current list")
    expect(screen.getByRole("heading", { name: "Second item" })).toBeVisible()
  })

  it("lets a feature supply a collection without nesting another viewport", () => {
    const { container } = render(
      <CollectionDetailPane
        title="Custom"
        items={items}
        emptyMessage="Empty"
        detailEmptyMessage="Select"
        renderCollection={({ select }) => (
          <button onClick={() => select(items[0]!)}>Custom item</button>
        )}
        renderDetail={(item) => item.label}
      />,
    )
    expect(container.querySelector(".main-window-collection-scroll")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Custom item" }))
    expect(screen.getByRole("heading", { name: "First item" })).toBeVisible()
  })

  it("shows an empty collection beside the unselected detail region", () => {
    render(<Workspace entries={[]} />)
    expect(screen.getByText("Nothing here")).toBeVisible()
    expect(screen.getByText("Select an item")).toBeVisible()
    expect(screen.queryByRole("listbox")).toBeNull()
  })
})
