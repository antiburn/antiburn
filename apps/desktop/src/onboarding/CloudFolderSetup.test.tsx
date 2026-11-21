import { fireEvent, render, screen, within } from "@testing-library/react"
import { useState } from "react"
import { expect, it, vi } from "vitest"
import { CloudFolderSetup, type CloudFolderSetupProps } from "./CloudFolderSetup"

const personal = { id: "example-personal", name: "My workspace", kind: "personal" } as const
const team = { id: "example-team", name: "Example Studio", kind: "team" } as const
const folders = [
  {
    id: "studio",
    path: "/Users/tester/dev/Studio",
    name: "Studio group",
    sessionCount: 12,
    worktreeCount: 2,
  },
  { id: "website", path: "/Users/tester-other/dev/Website", sessionCount: null },
]
const start = () => screen.getByRole("button", { name: "Start syncing" })
const finish = () => screen.getByRole("button", { name: "Finish without syncing" })
const destinationFor = (name: RegExp) => screen.getByRole("combobox", { name })

function fixture(patch: Partial<CloudFolderSetupProps> = {}) {
  return {
    destinations: [personal],
    folders,
    choices: {},
    homeDirectory: "/Users/tester",
    onChoiceChange: vi.fn<CloudFolderSetupProps["onChoiceChange"]>(),
    onStart: vi.fn<CloudFolderSetupProps["onStart"]>(),
    onFinish: vi.fn<CloudFolderSetupProps["onFinish"]>(),
    ...patch,
  }
}

function Draft(props: CloudFolderSetupProps) {
  const [choices, setChoices] = useState(props.choices)
  return (
    <CloudFolderSetup
      {...props}
      choices={choices}
      onChoiceChange={(id, choice) => {
        setChoices((current) => ({ ...current, [id]: choice }))
        props.onChoiceChange(id, choice)
      }}
    />
  )
}

it("starts new groups off and records explicit edits using one Personal switch per row", () => {
  const props = fixture()
  render(<Draft {...props} />)
  const studio = screen.getByRole("switch", { name: /Studio group.*Personal/ })
  const website = screen.getByRole("switch", { name: /Website.*Personal/ })
  expect(studio).not.toBeChecked()
  expect(website).not.toBeChecked()
  expect(start()).toBeDisabled()
  expect(props.onChoiceChange).not.toHaveBeenCalled()
  expect(screen.getByText("~/dev/Studio")).toBeVisible()
  expect(screen.getByText("/Users/tester-other/dev/Website")).toBeVisible()
  expect(screen.getByText("12 sessions")).toBeVisible()
  expect(screen.getByText("2 worktrees")).toBeVisible()
  expect(screen.getByText("Session count unavailable")).toBeVisible()

  fireEvent.click(studio)
  expect(studio).toBeChecked()
  expect(website).not.toBeChecked()
  expect(props.onChoiceChange).toHaveBeenLastCalledWith("studio", personal.id)
  expect(start()).toBeEnabled()
  fireEvent.click(studio)
  expect(props.onChoiceChange).toHaveBeenLastCalledWith("studio", null)
  expect(start()).toBeDisabled()
  expect(props.onStart).not.toHaveBeenCalled()
})

it("confirms only visible assignments with full-session scope and named routes", () => {
  const props = fixture({ destinations: [team, personal], choices: { unseen: team.id } })
  render(<Draft {...props} />)
  const studio = destinationFor(/Studio group/)
  const website = destinationFor(/Website/)
  expect(screen.queryByRole("switch")).not.toBeInTheDocument()
  expect(
    within(studio)
      .getAllByRole("option")
      .map((option) => option.textContent),
  ).toEqual(["Don’t sync", "Personal", "Example Studio"])
  expect(studio).toHaveDisplayValue("Don’t sync")
  expect(start()).not.toHaveAccessibleDescription(/Team owners and admins can read/)
  fireEvent.change(studio, { target: { value: team.id } })
  fireEvent.change(website, { target: { value: personal.id } })
  expect(studio).toHaveDisplayValue("Example Studio")
  expect(website).toHaveDisplayValue("Personal")
  expect(start()).toHaveAccessibleDescription(
    /full sessions available now and new sessions.*selected folders and their worktrees.*Team owners and admins can read.*2 folders selected/,
  )
  expect(props.onStart).not.toHaveBeenCalled()
  fireEvent.click(start())
  expect(props.onStart).toHaveBeenCalledExactlyOnceWith([
    { folderId: "studio", destinationId: team.id },
    { folderId: "website", destinationId: personal.id },
  ])
})

it("keeps a removed route visible and blocks Start until the user edits it", () => {
  const props = fixture({ choices: { studio: team.id, website: personal.id } })
  const { rerender } = render(<Draft {...props} destinations={[personal, team]} />)
  expect(destinationFor(/Studio group/)).toHaveDisplayValue("Example Studio")
  rerender(<Draft {...props} loading />)
  expect(screen.queryByText("Destination no longer available")).not.toBeInTheDocument()
  expect(start()).toBeDisabled()
  rerender(<Draft {...props} />)
  const studio = destinationFor(/Studio group/)
  expect(studio).toHaveDisplayValue("Unavailable destination")
  expect(within(studio).getByRole("option", { name: "Unavailable destination" })).toBeDisabled()
  expect(screen.getByText("Destination no longer available")).toBeVisible()
  expect(screen.getByRole("switch", { name: /Website.*Personal/ })).toBeChecked()
  expect(start()).toBeDisabled()
  expect(props.onChoiceChange).not.toHaveBeenCalled()

  fireEvent.change(studio, { target: { value: "" } })
  expect(props.onChoiceChange).toHaveBeenCalledExactlyOnceWith("studio", null)
  expect(screen.getByRole("switch", { name: /Studio group.*Personal/ })).not.toBeChecked()
  fireEvent.click(start())
  expect(props.onStart).toHaveBeenCalledExactlyOnceWith([
    { folderId: "website", destinationId: personal.id },
  ])
})

it("offers a local finish even when discovery or a destination blocks uploads", () => {
  const props = fixture({ destinations: [], folders: [], message: "Cloud is offline." })
  render(<CloudFolderSetup {...props} />)
  expect(screen.getByText("Cloud is offline.")).toBeVisible()
  expect(screen.getByText("No folders loaded.")).toBeVisible()
  expect(start()).toBeDisabled()
  fireEvent.click(finish())
  expect(props.onFinish).toHaveBeenCalledOnce()
  expect(props.onChoiceChange).not.toHaveBeenCalled()
  expect(props.onStart).not.toHaveBeenCalled()
})

it("does not submit an active draft when the caller blocks Start or the user finishes locally", () => {
  const props = fixture({ choices: { studio: personal.id }, startBlocked: true })
  const { rerender } = render(<CloudFolderSetup {...props} />)
  expect(screen.getByRole("heading", { name: "Choose folders to sync" })).toHaveFocus()
  expect(start()).toBeDisabled()
  rerender(<CloudFolderSetup {...props} busy />)
  expect(finish()).toBeDisabled()
  rerender(<CloudFolderSetup {...props} startBlocked={false} />)
  expect(start()).toBeEnabled()
  fireEvent.click(finish())
  expect(props.onFinish).toHaveBeenCalledOnce()
  expect(props.onStart).not.toHaveBeenCalled()
  expect(props.onChoiceChange).not.toHaveBeenCalled()
})

it("does not allow an unavailable folder to start or gain a new assignment", () => {
  const props = fixture({
    folders: [{ ...folders[0]!, captureSupported: false }],
    choices: { studio: personal.id },
  })
  render(<Draft {...props} />)
  const studio = destinationFor(/Studio group/)
  expect(within(studio).getByRole("option", { name: "Personal" })).toBeDisabled()
  expect(screen.getByText("Uploads unavailable for this folder")).toBeVisible()
  expect(start()).toBeDisabled()
  fireEvent.change(studio, { target: { value: "" } })
  expect(props.onChoiceChange).toHaveBeenCalledExactlyOnceWith("studio", null)
  expect(start()).toBeDisabled()
})
