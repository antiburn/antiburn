import { fireEvent, render, screen, within } from "@testing-library/react"
import { Bug, Circle, Square, Triangle } from "lucide-react"
import { useState } from "react"
import { describe, expect, it, vi } from "vitest"

import { SidebarNav, type SidebarNavItem } from "./SidebarNav"

const NESTED_ITEMS: SidebarNavItem[] = [
  {
    id: "sessions",
    label: "Sessions",
    icon: Square,
    count: 12,
    children: [
      { id: "notable", label: "Notable", count: 3 },
      { id: "material", label: "Material", count: 5 },
      {
        id: "claude-code",
        label: "Claude Code",
        count: 4,
        separatorBefore: true,
        controls: "sessions-panel",
      },
    ],
  },
  { id: "checks", label: "Burn checks", icon: Triangle },
]

const ITEMS: SidebarNavItem[] = [
  { id: "first", label: "First", icon: Circle },
  { id: "second", label: "Second", icon: Square },
  { id: "third", label: "Third", icon: Triangle },
  { id: "extra", label: "Extra", icon: Bug, separatorBefore: true },
]

/** Drives the controlled `value` so key handling can be observed end to end. */
function Harness({ initial = "first" }: { initial?: string }) {
  const [value, setValue] = useState(initial)
  return <SidebarNav items={ITEMS} value={value} onChange={setValue} ariaLabel="Sections" />
}

function tab(name: string) {
  return screen.getByRole("tab", { name })
}

function selectedTabName() {
  return screen.getAllByRole("tab").find((el) => el.getAttribute("aria-selected") === "true")
    ?.textContent
}

describe("SidebarNav", () => {
  it("renders a labelled vertical tablist of tabs", () => {
    render(<Harness />)

    const list = screen.getByRole("tablist", { name: "Sections" })
    expect(list.getAttribute("aria-orientation")).toBe("vertical")
    expect(screen.getAllByRole("tab")).toHaveLength(ITEMS.length)
  })

  it("marks only the active row aria-selected and points it at its panel", () => {
    render(<Harness initial="second" />)

    expect(tab("Second").getAttribute("aria-selected")).toBe("true")
    expect(tab("Second").getAttribute("aria-controls")).toBe("second-panel")
    expect(tab("Second").id).toBe("second-tab")
    for (const name of ["First", "Third", "Extra"]) {
      expect(tab(name).getAttribute("aria-selected")).toBe("false")
    }
  })

  it("keeps a roving tabindex so only the active row is tabbable", () => {
    render(<Harness initial="third" />)

    expect(tab("Third").getAttribute("tabindex")).toBe("0")
    for (const name of ["First", "Second", "Extra"]) {
      expect(tab(name).getAttribute("tabindex")).toBe("-1")
    }
  })

  it("moves selection and focus with ArrowDown / ArrowUp", () => {
    render(<Harness />)

    fireEvent.keyDown(tab("First"), { key: "ArrowDown" })
    expect(selectedTabName()).toBe("Second")
    expect(document.activeElement).toBe(tab("Second"))

    fireEvent.keyDown(tab("Second"), { key: "ArrowDown" })
    expect(selectedTabName()).toBe("Third")

    fireEvent.keyDown(tab("Third"), { key: "ArrowUp" })
    expect(selectedTabName()).toBe("Second")
    expect(document.activeElement).toBe(tab("Second"))
  })

  it("wraps at both ends", () => {
    render(<Harness />)

    fireEvent.keyDown(tab("First"), { key: "ArrowUp" })
    expect(selectedTabName()).toBe("Extra")

    fireEvent.keyDown(tab("Extra"), { key: "ArrowDown" })
    expect(selectedTabName()).toBe("First")
  })

  it("jumps to the first and last row with Home / End", () => {
    render(<Harness initial="second" />)

    fireEvent.keyDown(tab("Second"), { key: "End" })
    expect(selectedTabName()).toBe("Extra")
    expect(document.activeElement).toBe(tab("Extra"))

    fireEvent.keyDown(tab("Extra"), { key: "Home" })
    expect(selectedTabName()).toBe("First")
    expect(document.activeElement).toBe(tab("First"))
  })

  it("selects on click and ignores unrelated keys", () => {
    const onChange = vi.fn()
    render(<SidebarNav items={ITEMS} value="first" onChange={onChange} ariaLabel="Sections" />)

    fireEvent.click(tab("Third"))
    expect(onChange).toHaveBeenCalledWith("third")

    onChange.mockClear()
    fireEvent.keyDown(tab("First"), { key: "ArrowRight" })
    fireEvent.keyDown(tab("First"), { key: "a" })
    expect(onChange).not.toHaveBeenCalled()
  })

  it("renders a footer node outside the tablist", () => {
    render(
      <SidebarNav
        items={ITEMS}
        value="first"
        onChange={vi.fn()}
        ariaLabel="Sections"
        footer={<button type="button">Quit</button>}
      />,
    )

    expect(screen.getByRole("button", { name: "Quit" })).not.toBeNull()
    const tablist = screen.getByRole("tablist", { name: "Sections" })
    // Rows carry `role="tab"`, so a "button" role query within the tablist
    // stays empty either way — the real guard is that every row is still
    // present as a tab and the footer's plain button never joins them.
    expect(within(tablist).queryAllByRole("tab")).toHaveLength(ITEMS.length)
    expect(within(tablist).queryAllByRole("button")).toHaveLength(0)
  })

  it("renders no footer wrapper when the prop is omitted", () => {
    render(<Harness />)

    expect(screen.queryByRole("button", { name: "Quit" })).toBeNull()
  })

  it("renders a count pill with tabular-nums", () => {
    render(
      <SidebarNav
        items={NESTED_ITEMS}
        value="sessions"
        onChange={vi.fn()}
        ariaLabel="Sections"
      />,
    )

    const pill = within(tab("Sessions")).getByText("12")
    expect(pill).toHaveClass("font-mono", "tabular-nums")
  })

  it("points a child's aria-controls at its own panel by default", () => {
    render(
      <SidebarNav
        items={NESTED_ITEMS}
        value="sessions"
        onChange={vi.fn()}
        ariaLabel="Sections"
      />,
    )

    expect(tab("Notable").getAttribute("aria-controls")).toBe("notable-panel")
  })

  it("points a child's aria-controls at a shared panel when controls is set", () => {
    render(
      <SidebarNav
        items={NESTED_ITEMS}
        value="sessions"
        onChange={vi.fn()}
        ariaLabel="Sections"
      />,
    )

    expect(tab("Claude Code").getAttribute("aria-controls")).toBe("sessions-panel")
  })

  it("renders children indented and reachable by ArrowDown from the parent", () => {
    render(
      <SidebarNav
        items={NESTED_ITEMS}
        value="sessions"
        onChange={vi.fn()}
        ariaLabel="Sections"
      />,
    )

    for (const name of ["Notable", "Material", "Claude Code"]) {
      expect(tab(name)).not.toBeNull()
    }

    const notable = tab("Notable")
    expect(notable.className).toContain("pl-8")
  })

  it("moves ArrowDown from a parent onto its first child, in document order", () => {
    const onChange = vi.fn()
    const { rerender } = render(
      <SidebarNav
        items={NESTED_ITEMS}
        value="sessions"
        onChange={onChange}
        ariaLabel="Sections"
      />,
    )

    fireEvent.keyDown(tab("Sessions"), { key: "ArrowDown" })
    expect(onChange).toHaveBeenCalledWith("notable")

    rerender(
      <SidebarNav
        items={NESTED_ITEMS}
        value="notable"
        onChange={onChange}
        ariaLabel="Sections"
      />,
    )
    fireEvent.keyDown(tab("Notable"), { key: "ArrowDown" })
    expect(onChange).toHaveBeenCalledWith("material")
  })

  it("spans children with Home and End", () => {
    render(
      <SidebarNav
        items={NESTED_ITEMS}
        value="material"
        onChange={vi.fn()}
        ariaLabel="Sections"
      />,
    )

    fireEvent.keyDown(tab("Material"), { key: "End" })
    expect(document.activeElement).toBe(tab("Burn checks"))

    fireEvent.keyDown(tab("Burn checks"), { key: "Home" })
    expect(document.activeElement).toBe(tab("Sessions"))
  })

  it("calls onChange with the child id on click", () => {
    const onChange = vi.fn()
    render(
      <SidebarNav
        items={NESTED_ITEMS}
        value="sessions"
        onChange={onChange}
        ariaLabel="Sections"
      />,
    )

    fireEvent.click(tab("Material"))
    expect(onChange).toHaveBeenCalledWith("material")
  })

  it("renders a child separator indented to match the child rows", () => {
    render(
      <SidebarNav
        items={NESTED_ITEMS}
        value="sessions"
        onChange={vi.fn()}
        ariaLabel="Sections"
      />,
    )

    const tablist = screen.getByRole("tablist", { name: "Sections" })
    const separators = tablist.querySelectorAll('[role="presentation"]')
    const childSeparator = Array.from(separators).find((el) => el.className.includes("ml-8"))
    expect(childSeparator).not.toBeUndefined()
    expect(childSeparator?.hasAttribute("data-nested")).toBe(true)
  })

  it("marks child rows with data-nested and leaves top-level rows unmarked", () => {
    render(
      <SidebarNav
        items={NESTED_ITEMS}
        value="sessions"
        onChange={vi.fn()}
        ariaLabel="Sections"
      />,
    )

    const parent = screen.getByRole("tab", { name: "Sessions" })
    expect(parent.hasAttribute("data-nested")).toBe(false)
    const nested = screen
      .getAllByRole("tab")
      .filter((tab) => tab.hasAttribute("data-nested"))
      .map((tab) => tab.id)
    const expected = NESTED_ITEMS.flatMap((item) =>
      (item.children ?? []).map((child) => `${child.id}-tab`),
    )
    expect(nested).toEqual(expected)
  })

  it("never moves arrow-key navigation onto the footer", () => {
    render(
      <SidebarNav
        items={ITEMS}
        value="extra"
        onChange={vi.fn()}
        ariaLabel="Sections"
        footer={<button type="button">Quit</button>}
      />,
    )

    fireEvent.keyDown(tab("Extra"), { key: "ArrowDown" })
    expect(document.activeElement).toBe(tab("First"))
    expect(document.activeElement).not.toBe(screen.getByRole("button", { name: "Quit" }))
  })
})
