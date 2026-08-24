// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { SubagentBadge } from "./SubagentBadge"

afterEach(cleanup)

describe("SubagentBadge", () => {
  it("marks the view as an autonomous sub-agent", () => {
    render(<SubagentBadge parentAgent="claude-code" onOpenOrchestrator={() => {}} />)
    expect(screen.getByText("Autonomous sub-agent")).toBeTruthy()
  })

  it("hides the launching orchestrator until expanded, then labels it", () => {
    render(
      <SubagentBadge
        parentAgent="claude-code"
        parentTitle="add-ci-workflow"
        onOpenOrchestrator={() => {}}
      />,
    )
    expect(screen.queryByText("add-ci-workflow")).toBeNull()

    fireEvent.click(screen.getByText("Autonomous sub-agent"))
    expect(screen.getByText("Launched by")).toBeTruthy()
    expect(screen.getByText("add-ci-workflow")).toBeTruthy()
  })

  it("opens the orchestrator's analysis when its row is clicked", () => {
    const onOpen = vi.fn()
    render(
      <SubagentBadge
        parentAgent="claude-code"
        parentTitle="add-ci-workflow"
        onOpenOrchestrator={onOpen}
      />,
    )
    fireEvent.click(screen.getByText("Autonomous sub-agent"))
    fireEvent.click(screen.getByText("add-ci-workflow"))
    expect(onOpen).toHaveBeenCalledOnce()
  })

  it("names the orchestrator generically when its title is unknown", () => {
    render(
      <SubagentBadge parentAgent="codex" parentTitle="   " onOpenOrchestrator={() => {}} />,
    )
    fireEvent.click(screen.getByText("Autonomous sub-agent"))
    expect(screen.getByText("Orchestrator session")).toBeTruthy()
  })
})
