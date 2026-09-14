import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import type { UnusedContextRow } from "../../../lib/presentation/unusedContext"
import { UnusedContext } from "./UnusedContext"

afterEach(cleanup)

describe("UnusedContext", () => {
  it("renders nothing for an empty row list", () => {
    const { container } = render(<UnusedContext rows={[]} />)
    expect(container).toBeEmptyDOMElement()
  })

  it("renders each row's name, kind, and figure", () => {
    const rows: UnusedContextRow[] = [
      { name: "playwright", kind: "MCP server", costUsd: 0.42 },
      { name: "bash", kind: "Built-in tool", costUsd: 0.1 },
    ]
    render(<UnusedContext rows={rows} />)

    expect(screen.getByText("playwright")).toBeTruthy()
    expect(screen.getByText("MCP server")).toBeTruthy()
    expect(screen.getByText("$0.42")).toBeTruthy()
    expect(screen.getByText("bash")).toBeTruthy()
    expect(screen.getByText("Built-in tool")).toBeTruthy()
    expect(screen.getByText("$0.10")).toBeTruthy()
  })

  it("shows Not priced for a row with no resolved cost", () => {
    const rows: UnusedContextRow[] = [{ name: "code-search", kind: "Skill", costUsd: null }]
    render(<UnusedContext rows={rows} />)

    expect(screen.getByText("code-search")).toBeTruthy()
    expect(screen.getByText("Not priced")).toBeTruthy()
  })

  it("omits the Total row when fewer than two rows are priced", () => {
    const rows: UnusedContextRow[] = [
      { name: "solo", kind: "Skill", costUsd: 1.5 },
      { name: "unpriced", kind: "MCP server", costUsd: null },
    ]
    render(<UnusedContext rows={rows} />)

    expect(screen.queryByText("Total")).toBeNull()
  })

  it("shows a Total row with the summed cost when two or more rows are priced", () => {
    const rows: UnusedContextRow[] = [
      { name: "first", kind: "Skill", costUsd: 1.5 },
      { name: "second", kind: "MCP server", costUsd: 0.25 },
    ]
    render(<UnusedContext rows={rows} />)

    expect(screen.getByText("Total")).toBeTruthy()
    expect(screen.getByText("$1.75")).toBeTruthy()
  })
})
