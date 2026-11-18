import { fireEvent, render, screen } from "@testing-library/react"
import { expect, it } from "vitest"
import { CloudProjectPaths } from "./CloudProjectPaths"

it("provides a focusable disclosure containing every approved path", () => {
  render(
    <CloudProjectPaths
      name="Studio"
      paths={["/Example/Studio", "/Example/external/worktree"]}
    />,
  )
  const summary = screen.getByText("Included folders", { selector: "summary" })
  expect(summary).toHaveAttribute("aria-label", "Included folders for Studio")
  summary.focus()
  expect(summary).toHaveFocus()
  fireEvent.click(summary)
  expect(summary.closest("details")).toHaveAttribute("open")
  expect(screen.getByText("/Example/Studio")).toBeVisible()
  expect(screen.getByText("/Example/external/worktree")).toBeVisible()
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  expect(screen.queryByRole("switch")).not.toBeInTheDocument()
})
