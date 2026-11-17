import { fireEvent, render, screen } from "@testing-library/react"
import { expect, it } from "vitest"
import { emptyCloudSnapshot, type CloudWorkspace } from "../lib/cloud"
import { CloudCaptureConsent } from "./CloudCaptureConsent"
import { CloudSession, type CloudViewSnapshot } from "./CloudSession"

function fixture(destination: CloudWorkspace | null) {
  const session = new CloudSession(undefined, false, true)
  const value: CloudViewSnapshot = {
    cloud: {
      ...emptyCloudSnapshot,
      identity: { id: "synthetic-account", login: "Example account" },
      destination,
      draft: {
        screen: "folders",
        folders: [{ path: "/Example/Project", selected: true }],
      },
    },
    screen: "folders",
    sync: { status: "unavailable" },
    agent: "unavailable",
    busy: false,
    error: null,
    loaded: true,
    fullContentConsent: false,
  }
  return { session, value }
}

it("names the account, destination, full native content, and separate attachments", () => {
  const { session, value } = fixture({
    id: "personal",
    name: "Personal workspace",
    kind: "personal",
  })
  render(<CloudCaptureConsent session={session} value={value} />)
  expect(screen.getByText("Example account")).toBeVisible()
  expect(screen.getByText("Personal workspace")).toBeVisible()
  const consent = screen.getByRole("checkbox", { name: /upload full native session content/ })
  expect(consent).not.toBeChecked()
  expect(consent).toBeEnabled()
  for (const content of [
    "messages",
    "private thinking",
    "tool calls and results",
    "metadata",
  ]) {
    expect(consent).toHaveAccessibleName(expect.stringContaining(content))
  }
  expect(screen.getByText(/Attachments are separate/)).toBeVisible()
  fireEvent.click(consent)
  expect(session.getSnapshot().fullContentConsent).toBe(true)
})

it.each([null, { id: "team", name: "Requested team", kind: "team" } satisfies CloudWorkspace])(
  "blocks capture without a verified personal destination: %j",
  (destination) => {
    const { session, value } = fixture(destination)
    render(<CloudCaptureConsent session={session} value={value} />)
    expect(screen.getByRole("checkbox")).toBeDisabled()
    if (destination) {
      expect(screen.getByText("Requested team")).toBeVisible()
      expect(screen.getByRole("status")).toHaveTextContent("has not been replaced")
    } else {
      expect(screen.getByText(/Not verified. Capture is blocked/)).toBeVisible()
    }
  },
)

it("requires an explicit source selection before full-content consent", () => {
  const { session, value } = fixture({ id: "personal", name: "Personal", kind: "personal" })
  value.cloud.draft.folders[0]!.selected = false
  render(<CloudCaptureConsent session={session} value={value} />)
  expect(screen.getByRole("checkbox")).toBeDisabled()
  expect(screen.getByText(/Select folders before confirming/)).toBeVisible()
})
