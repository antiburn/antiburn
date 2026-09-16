import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

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
  afterEach(() => vi.unstubAllGlobals())

  it("does not steal focus from a collection control after Back and a resize", async () => {
    vi.stubGlobal("innerWidth", 550)
    render(
      <CollectionDetailPane
        title="Collection"
        items={[]}
        selection={items[0]!}
        externalDetailRevealRevision={1}
        emptyMessage="Empty"
        detailEmptyMessage="Select"
        renderCollection={() => <button>Collection filter</button>}
        renderDetail={(item) => item.label}
      />,
    )
    await waitFor(() =>
      expect(screen.getByRole("region", { name: "First item" })).toHaveFocus(),
    )
    fireEvent.click(screen.getByRole("button", { name: "Back to collection" }))
    expect(screen.getByRole("region", { name: "Collection" })).toHaveFocus()
    const filter = screen.getByRole("button", { name: "Collection filter" })
    filter.focus()
    vi.stubGlobal("innerWidth", 1200)
    fireEvent(window, new Event("resize"))
    expect(filter).toHaveFocus()
  })

  it.each([false, true])(
    "restores a mounted virtual row unless focus moved: %s",
    async (focusMoved) => {
      vi.stubGlobal("innerWidth", 550)
      function VirtualWorkspace({ mounted }: { mounted: boolean }) {
        return (
          <>
            <button>Outside collection</button>
            <CollectionDetailPane
              title="Virtual collection"
              items={items}
              emptyMessage="Empty"
              detailEmptyMessage="Select"
              renderCollection={({ openDetail, selectedId }) =>
                mounted ? (
                  <button
                    aria-current={selectedId === "one" ? "true" : undefined}
                    onClick={() => openDetail(items[0]!)}
                  >
                    Virtual row
                  </button>
                ) : null
              }
              renderDetail={(item) => item.label}
            />
          </>
        )
      }
      const { rerender } = render(<VirtualWorkspace mounted />)
      fireEvent.click(screen.getByRole("button", { name: "Virtual row" }))
      await waitFor(() =>
        expect(screen.getByRole("region", { name: "First item" })).toHaveFocus(),
      )
      rerender(<VirtualWorkspace mounted={false} />)
      fireEvent.click(screen.getByRole("button", { name: "Back to virtual collection" }))
      expect(screen.getByRole("region", { name: "Virtual collection" })).toHaveFocus()
      const outside = screen.getByRole("button", { name: "Outside collection" })
      if (focusMoved) {
        outside.focus()
        vi.stubGlobal("innerWidth", 1200)
        fireEvent(window, new Event("resize"))
      }
      rerender(<VirtualWorkspace mounted />)
      await waitFor(() =>
        expect(
          focusMoved ? outside : screen.getByRole("button", { name: "Virtual row" }),
        ).toHaveFocus(),
      )
    },
  )

  it("opens one detail pane below 900 CSS pixels and restores the list focus and scroll", async () => {
    vi.stubGlobal("innerWidth", 550)
    const { container } = render(<Workspace />)
    const viewport = container.querySelector<HTMLElement>(".main-window-collection-scroll")!
    viewport.scrollTop = 42
    const second = screen.getByRole("option", { name: "Second item" })
    fireEvent.click(second)
    expect(screen.queryByRole("listbox")).toBeNull()
    expect(screen.getByText("Content for Second item")).toBeVisible()
    await waitFor(() =>
      expect(screen.getByRole("region", { name: "Second item" })).toHaveFocus(),
    )
    fireEvent.click(screen.getByRole("button", { name: "Back to collection" }))
    await waitFor(() => expect(second).toHaveFocus())
    expect(screen.getByRole("listbox")).toBeVisible()
    expect(viewport.scrollTop).toBe(42)
    expect(screen.queryByRole("region", { name: "Second item" })).toBeNull()
  })

  it("opens and focuses a repeated external detail request after Back", async () => {
    vi.stubGlobal("innerWidth", 550)
    function ExternalWorkspace({ revision }: { revision: number }) {
      return (
        <CollectionDetailPane
          title="Collection"
          items={items}
          emptyMessage="Nothing here"
          detailEmptyMessage="Select an item"
          selection={items[0]!}
          externalDetailRevealRevision={revision}
          renderDetail={(item) => <p>Content for {item.label}</p>}
        />
      )
    }
    const { rerender } = render(<ExternalWorkspace revision={0} />)
    expect(screen.getByRole("listbox")).toBeVisible()

    rerender(<ExternalWorkspace revision={1} />)
    await waitFor(() =>
      expect(screen.getByRole("region", { name: "First item" })).toHaveFocus(),
    )
    fireEvent.click(screen.getByRole("button", { name: "Back to collection" }))
    expect(screen.getByRole("listbox")).toBeVisible()

    rerender(<ExternalWorkspace revision={2} />)
    await waitFor(() =>
      expect(screen.getByRole("region", { name: "First item" })).toHaveFocus(),
    )
  })

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
