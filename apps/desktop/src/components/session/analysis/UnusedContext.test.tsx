import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { SharedTooltipOwnerContext } from "../../presentation/Tooltip"
import type { UnusedContextRow } from "../../../lib/presentation/unusedContext"
import { UnusedContext } from "./UnusedContext"
import { unusedContextExpandedStore } from "./unusedContextExpandedStore"

afterEach(() => {
  cleanup()
  // The expanded flag is app-wide, so reset it between tests to keep them independent.
  unusedContextExpandedStore.set(false)
})

const TITLE = "Unused skills, MCP servers, and built-in tools"

describe("UnusedContext", () => {
  it("renders nothing for an empty row list", () => {
    const { container } = render(<UnusedContext rows={[]} sessionTotalUsd={null} />)
    expect(container).toBeEmptyDOMElement()
  })

  it("is collapsed by default: rows are not in the DOM and the header reports closed", () => {
    const rows: UnusedContextRow[] = [{ name: "playwright", kind: "MCP server", costUsd: 0.42 }]
    render(<UnusedContext rows={rows} sessionTotalUsd={7} />)

    expect(screen.queryByText("playwright")).toBeNull()
    expect(screen.getByRole("button", { name: new RegExp(TITLE) })).toHaveAttribute(
      "aria-expanded",
      "false",
    )
  })

  it("shows the title, total replay cost, and share of the session total in the header", () => {
    const rows: UnusedContextRow[] = [
      { name: "first", kind: "Skill", costUsd: 1.5 },
      { name: "second", kind: "MCP server", costUsd: 0.25 },
    ]
    render(<UnusedContext rows={rows} sessionTotalUsd={7} />)

    expect(screen.getByText(TITLE)).toBeTruthy()
    expect(screen.getByText("$1.75")).toBeTruthy()
    expect(screen.getByText("25%")).toBeTruthy()
  })

  it("hides the share when the session total is null", () => {
    const rows: UnusedContextRow[] = [{ name: "first", kind: "Skill", costUsd: 1.5 }]
    render(<UnusedContext rows={rows} sessionTotalUsd={null} />)

    expect(screen.getByText("$1.50")).toBeTruthy()
    expect(screen.queryByText(/%/)).toBeNull()
  })

  it("clicking the header reveals the rows, each with its own share cell", () => {
    const rows: UnusedContextRow[] = [
      { name: "playwright", kind: "MCP server", costUsd: 0.42 },
      { name: "bash", kind: "Built-in tool", costUsd: 0.28 },
    ]
    render(<UnusedContext rows={rows} sessionTotalUsd={0.7} />)

    fireEvent.click(screen.getByRole("button", { name: new RegExp(TITLE) }))

    expect(screen.getByRole("button", { name: new RegExp(TITLE) })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
    expect(screen.getByText("playwright")).toBeTruthy()
    expect(screen.getByText("MCP server")).toBeTruthy()
    expect(screen.getByText("$0.42")).toBeTruthy()
    expect(screen.getByText("60%")).toBeTruthy() // 0.42 / 0.7
    expect(screen.getByText("bash")).toBeTruthy()
    expect(screen.getByText("$0.28")).toBeTruthy()
    expect(screen.getByText("40%")).toBeTruthy() // 0.28 / 0.7
  })

  it("shows Not priced and a dash share for a row with no resolved cost", () => {
    const rows: UnusedContextRow[] = [{ name: "code-search", kind: "Skill", costUsd: null }]
    render(<UnusedContext rows={rows} sessionTotalUsd={5} />)

    fireEvent.click(screen.getByRole("button", { name: new RegExp(TITLE) }))

    expect(screen.getByText("code-search")).toBeTruthy()
    expect(screen.getByText("Not priced")).toBeTruthy()
    expect(screen.getByText("—")).toBeTruthy()
  })

  it("rolls up several small-cost rows of the same kind into one row, with the names in a tooltip", () => {
    const register = vi.fn(() => () => {})
    const rows: UnusedContextRow[] = [
      { name: "apply_patch", kind: "Built-in tool", costUsd: 0.001 },
      { name: "request_user_input", kind: "Built-in tool", costUsd: 0.0008 },
      { name: "tool_search", kind: "Built-in tool", costUsd: 0.0006 },
      { name: "update_plan", kind: "Built-in tool", costUsd: 0.0009 },
      { name: "view_image", kind: "Built-in tool", costUsd: 0.0007 },
      { name: "write_stdin", kind: "Built-in tool", costUsd: 0.0005 },
    ]
    render(
      <SharedTooltipOwnerContext.Provider value={{ register }}>
        <UnusedContext rows={rows} sessionTotalUsd={1} />
      </SharedTooltipOwnerContext.Provider>,
    )

    fireEvent.click(screen.getByRole("button", { name: new RegExp(TITLE) }))

    const rollupLabel = screen.getByText("6 built-in tools")
    expect(rollupLabel).toBeTruthy()
    const rollupRow = rollupLabel.closest('[tabindex="0"]')
    expect(rollupRow).not.toBeNull()
    expect(register).toHaveBeenCalledWith(
      rollupRow,
      expect.objectContaining({
        label:
          "apply_patch, request_user_input, tool_search, update_plan, view_image, write_stdin",
      }),
    )
  })
})
