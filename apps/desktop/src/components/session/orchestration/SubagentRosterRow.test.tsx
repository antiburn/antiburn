import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { CollapsibleOrchestrationCard } from "./CollapsibleOrchestrationCard"
import { SubagentRosterRow } from "./SubagentRosterRow"

afterEach(cleanup)

describe("SubagentRosterRow", () => {
  it("renders the label as one activatable row", () => {
    const onClick = vi.fn()
    render(<SubagentRosterRow agent="codex" label="Refactor auth" onClick={onClick} />)
    const row = screen.getByRole("button", { name: /Refactor auth/ })
    fireEvent.click(row)
    expect(onClick).toHaveBeenCalledOnce()
  })

  it("renders no artwork of its own when no icon renderer is supplied", () => {
    const { container } = render(<SubagentRosterRow agent="codex" label="Refactor auth" />)
    // Only the hover chevron, which is decorative.
    expect(container.querySelectorAll("svg")).toHaveLength(1)
  })

  it("passes the agent slug and size to the injected icon renderer", () => {
    const renderAgentIcon = vi.fn(() => <span data-testid="slot" />)
    render(
      <SubagentRosterRow
        agent="claude-code"
        label="Investigate"
        renderAgentIcon={renderAgentIcon}
      />,
    )
    expect(renderAgentIcon).toHaveBeenCalledWith("claude-code", 14)
    expect(screen.getByTestId("slot")).toBeTruthy()
  })
})

describe("CollapsibleOrchestrationCard", () => {
  it("keeps its body collapsed until the header is activated", () => {
    render(
      <CollapsibleOrchestrationCard title="Orchestrated 3 agents">
        <p>roster</p>
      </CollapsibleOrchestrationCard>,
    )
    const header = screen.getByRole("button", { name: /Orchestrated 3 agents/ })
    expect(header.getAttribute("aria-expanded")).toBe("false")
    expect(screen.queryByText("roster")).toBeNull()

    fireEvent.click(header)
    expect(header.getAttribute("aria-expanded")).toBe("true")
    expect(screen.getByText("roster")).toBeTruthy()

    fireEvent.click(header)
    expect(screen.queryByText("roster")).toBeNull()
  })

  it("renders the leading glyph slot when one is given", () => {
    render(
      <CollapsibleOrchestrationCard title="Titled" icon={<span data-testid="glyph" />}>
        <p>body</p>
      </CollapsibleOrchestrationCard>,
    )
    expect(screen.getByTestId("glyph")).toBeTruthy()
  })

  it("applies the caller body classes only once expanded", () => {
    const { container } = render(
      <CollapsibleOrchestrationCard title="Titled" bodyClassName="space-y-0.5">
        <p>body</p>
      </CollapsibleOrchestrationCard>,
    )
    expect(container.querySelector(".space-y-0\\.5")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: /Titled/ }))
    expect(container.querySelector(".space-y-0\\.5")).toBeTruthy()
  })
})
