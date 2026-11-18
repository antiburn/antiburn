import { fireEvent, render, screen } from "@testing-library/react"
import { expect, it } from "vitest"
import {
  cloudProjectSelectionKey,
  emptyCloudSnapshot,
  type CloudProjectGroup,
  type CloudWorkspace,
} from "../lib/cloud"
import { CloudCaptureConsent } from "./CloudCaptureConsent"
import { CloudSession, type CloudViewSnapshot } from "./CloudSession"

function fixture(destination: CloudWorkspace | null) {
  const session = new CloudSession(undefined, false, true)
  const value: CloudViewSnapshot = {
    cloud: {
      ...emptyCloudSnapshot,
      status: "signed_in",
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
    sourceListingNeedsRetry: false,
  }
  return { session, value }
}

it("names the account, destination, full session content, and separate attachments", () => {
  const { session, value } = fixture({
    id: "personal",
    name: "Personal workspace",
    kind: "personal",
  })
  render(<CloudCaptureConsent session={session} value={value} />)
  expect(screen.getByText("Example account")).toBeVisible()
  expect(screen.getByText("Personal workspace")).toBeVisible()
  const consent = screen.getByRole("checkbox", {
    name: /Upload existing and new sessions in full/,
  })
  expect(consent).not.toBeChecked()
  expect(consent).toBeEnabled()
  for (const content of [
    "messages",
    "private thinking",
    "tool activity and results",
    "session details",
  ]) {
    expect(consent).toHaveAccessibleDescription(expect.stringContaining(content))
  }
  expect(
    screen.getByText(/External attachments and linked-file downloads are excluded/),
  ).toBeVisible()
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
      expect(screen.getByRole("status")).toHaveTextContent(
        "has not changed your selected account",
      )
    } else {
      expect(screen.getByText(/Not confirmed. Uploads are off/)).toBeVisible()
    }
  },
)

it("requires an explicit source selection before full-content consent", () => {
  const { session, value } = fixture({ id: "personal", name: "Personal", kind: "personal" })
  value.cloud.draft.folders[0]!.selected = false
  render(<CloudCaptureConsent session={session} value={value} />)
  expect(screen.getByRole("checkbox")).toBeDisabled()
  expect(screen.getByText(/Select project folders before confirming/)).toBeVisible()
})

it("describes a desktop request without claiming to read content or upload it", () => {
  const { session, value } = fixture({ id: "personal", name: "Personal", kind: "personal" })
  value.cloud.captureEnabled = true
  render(<CloudCaptureConsent session={session} value={value} />)
  const consent = screen.getByRole("checkbox", {
    name: /authorize a full-content capture request/,
  })
  expect(consent).toBeEnabled()
  expect(screen.getByText(/Upload is unavailable/)).toBeVisible()
  expect(
    screen.getByText(/does not confirm that any source content was read or captured/),
  ).toBeVisible()
  expect(
    screen.queryByRole("checkbox", { name: /Upload existing and new sessions in full/ }),
  ).not.toBeInTheDocument()
})

it("requires separate ongoing consent for existing and new project sessions", () => {
  const { session, value } = fixture({ id: "personal", name: "Personal", kind: "personal" })
  value.cloud.captureEnabled = true
  value.cloud.syncEnabled = true
  render(<CloudCaptureConsent session={session} value={value} />)
  const consent = screen.getByRole("checkbox", {
    name: /Upload existing and new sessions in full/,
  })
  expect(consent).not.toBeChecked()
  expect(consent).toHaveAccessibleName(expect.stringContaining("existing and new sessions"))
  expect(consent).toHaveAccessibleName(expect.stringContaining("while uploads are on"))
  expect(consent).toHaveAccessibleName(expect.stringContaining("folders and their worktrees"))
  expect(consent).toHaveAccessibleDescription(
    expect.stringContaining("including work in other folders"),
  )
  expect(consent).toHaveAccessibleDescription(
    expect.stringContaining("Subfolders are included"),
  )
  expect(consent).toHaveAccessibleDescription(
    expect.stringContaining("New worktrees outside these folders need approval"),
  )
  expect(screen.queryByText(/Upload is unavailable/)).not.toBeInTheDocument()
  expect(session.getSnapshot().fullContentConsent).toBe(false)
})

const projectGroups: readonly CloudProjectGroup[] = [
  {
    id: "studio",
    name: "Studio",
    path: "/Example/Studio",
    memberPaths: ["/Example/Studio", "/Example/external/studio-fix"],
    sessionCount: 3,
    worktreeCount: 1,
  },
]

it("does not reuse an older unkeyed confirmation for grouped project consent", () => {
  const { session, value } = fixture({ id: "personal", name: "Personal", kind: "personal" })
  value.fullContentConsent = true
  value.cloud.draft.folders = []
  render(
    <CloudCaptureConsent
      session={session}
      value={value}
      projects={{ groups: projectGroups, choices: {} }}
    />,
  )
  const confirm = screen.getByRole("checkbox")
  expect(confirm).toBeEnabled()
  expect(confirm).not.toBeChecked()
  fireEvent.click(confirm)
  expect(session.getSnapshot()).toMatchObject({
    fullContentConsent: true,
    fullContentConsentKey: cloudProjectSelectionKey(projectGroups, {}),
  })
})

it("requires fresh confirmation for a new external worktree or recorded CWD", () => {
  const { session, value } = fixture({ id: "personal", name: "Personal", kind: "personal" })
  value.fullContentConsent = true
  value.fullContentConsentKey = cloudProjectSelectionKey(projectGroups, {})
  const { rerender } = render(
    <CloudCaptureConsent
      session={session}
      value={value}
      projects={{ groups: projectGroups, choices: {} }}
    />,
  )
  expect(screen.getByRole("checkbox")).toBeChecked()
  const countUpdate = projectGroups.map((group) => ({ ...group, sessionCount: 10 }))
  rerender(
    <CloudCaptureConsent
      session={session}
      value={value}
      projects={{ groups: countUpdate, choices: {} }}
    />,
  )
  expect(screen.getByRole("checkbox")).toBeChecked()
  const newMember = projectGroups.map((group) => ({
    ...group,
    memberPaths: [...group.memberPaths, "/Example/external/studio-review"],
    worktreeCount: 2,
  }))
  rerender(
    <CloudCaptureConsent
      session={session}
      value={value}
      projects={{ groups: newMember, choices: {} }}
    />,
  )
  expect(screen.getByRole("checkbox")).not.toBeChecked()
  expect(value.fullContentConsentKey).toBe(cloudProjectSelectionKey(projectGroups, {}))
  fireEvent.click(screen.getByRole("checkbox"))
  expect(session.getSnapshot().fullContentConsentKey).toBe(
    cloudProjectSelectionKey(newMember, {}),
  )
})

it("does not approve a selected group without any recorded path members", () => {
  const { session, value } = fixture({ id: "personal", name: "Personal", kind: "personal" })
  render(
    <CloudCaptureConsent
      session={session}
      value={value}
      projects={{ groups: [{ ...projectGroups[0]!, memberPaths: [] }], choices: {} }}
    />,
  )
  expect(screen.getByRole("checkbox")).toBeDisabled()
})

it("does not enable upload consent for display-only agent suggestions", () => {
  const { session, value } = fixture({ id: "personal", name: "Personal", kind: "personal" })
  render(
    <CloudCaptureConsent
      session={session}
      value={value}
      projects={{
        groups: [{ ...projectGroups[0]!, captureSupported: false }],
        choices: {},
      }}
    />,
  )
  expect(screen.getByRole("checkbox")).toBeDisabled()
})
