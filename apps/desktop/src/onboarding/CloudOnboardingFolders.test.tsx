import { fireEvent, render, screen } from "@testing-library/react"
import { expect, it, vi } from "vitest"
import type { CloudProjectGroup } from "../lib/cloud"
import { CloudProjectFolders, CloudProjectFolderSummary } from "./CloudOnboardingFolders"

const group: CloudProjectGroup = {
  id: "studio",
  name: "Studio",
  path: "/Users/avery/dev/Studio",
  memberPaths: [
    "/Users/avery/dev/Studio",
    "/Users/avery/dev/worktrees/studio-documentation-preview/feature-cloud-folder-layout",
    "/Users/jordan/shared/studio-review",
  ],
  sessionCount: 47,
  worktreeCount: 2,
  captureSupported: true,
}

it("keeps path and counts below the repository name with exact raw path descriptions", () => {
  const select = vi.fn()
  render(
    <CloudProjectFolders
      groups={[group]}
      choices={{}}
      homeDirectory="/Users/avery"
      onSelect={select}
    />,
  )
  const name = screen.getByText("Studio")
  const path = screen.getByText("~/dev/Studio")
  const summary = path.parentElement!
  expect(summary.parentElement).toContainElement(name)
  expect(summary).toHaveTextContent("~/dev/Studio")
  expect(summary).toHaveTextContent("47 sessions")
  expect(summary).toHaveTextContent("2 worktrees")
  expect(path).toHaveAttribute("title", group.path)
  expect(summary).toHaveAccessibleDescription(`${group.path} · 47 sessions · 2 worktrees`)
  expect(screen.getByRole("switch")).toHaveAccessibleDescription(
    expect.stringContaining("47 sessions"),
  )
  expect(document.querySelector("details, summary")).toBeNull()
  expect(
    screen.queryByText(
      "~/dev/worktrees/studio-documentation-preview/feature-cloud-folder-layout",
    ),
  ).not.toBeInTheDocument()
  expect(screen.queryByText("/Users/jordan/shared/studio-review")).not.toBeInTheDocument()
  expect(select).not.toHaveBeenCalled()
  fireEvent.click(screen.getByRole("switch"))
  expect(select).toHaveBeenCalledExactlyOnceWith("studio", false, undefined)
})

it("does not infer the current home from a provider path", () => {
  render(<CloudProjectFolders groups={[group]} choices={{}} onSelect={vi.fn()} />)
  expect(screen.getByText(group.path)).toBeVisible()
  expect(screen.queryByText("~/dev/Studio")).not.toBeInTheDocument()
})

it("can wrap the path above the counts without changing its accessible description", () => {
  render(<CloudProjectFolderSummary group={group} stackPath />)
  const path = screen.getByText(group.path)
  expect(path).toHaveClass("basis-full", "wrap-anywhere")
  expect(path).not.toHaveClass("truncate")
  expect(path.parentElement).toHaveAccessibleDescription(
    `${group.path} · 47 sessions · 2 worktrees`,
  )
  expect(screen.getByText("47 sessions")).toBeVisible()
  expect(screen.getByText("2 worktrees")).toBeVisible()
})
