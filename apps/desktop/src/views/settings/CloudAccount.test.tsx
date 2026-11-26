import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { expect, it, vi } from "vitest"
import {
  emptyCloudSnapshot,
  desktopCloudPort,
  type CloudSnapshot,
  type CloudWorkspaceRecovery,
} from "../../lib/cloud"
import { CloudSession } from "../../onboarding/CloudSession"
import { CloudAccount } from "./CloudAccount"
import { SettingsTargetFocus } from "./SettingsTargetFocus"
import {
  cloudProjectFixture,
  localToolsFixture,
  ongoingStatus,
} from "../../onboarding/__tests__/cloudProjectFixtures"

function fixture(initial?: CloudSnapshot) {
  let value: CloudSnapshot = initial ?? {
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "revocation_unconfirmed",
    revocationPending: true,
    localResetAvailable: true,
  }
  let publish: (value: CloudSnapshot) => void = () => {}
  let publishWorkspace: (value: CloudWorkspaceRecovery) => void = () => {}
  const resetLocal = vi.fn(async () => {
    value = {
      ...emptyCloudSnapshot,
      authEnabled: true,
      status: "signed_out",
      remoteRevocationUnconfirmed: true,
    }
    return value
  })
  const signOut = vi.fn(async () => value)
  const cancel = vi.fn(async () => {
    value = { ...value, status: "cancelled" }
    return value
  })
  const save = vi.fn(async (draft: CloudSnapshot["draft"]) => {
    value = { ...value, draft }
    return value
  })
  const prepareSync = vi.fn(async () => {})
  const start = vi.fn(async () => ({ status: "upload_unavailable" as const }))
  const reconcile = vi.fn(async () => ({ status: "upload_unavailable" as const }))
  const pick = vi.fn(async () => ["/Example/opencode/opencode.db"])
  const session = new CloudSession(
    {
      ...desktopCloudPort,
      ongoing: undefined,
      read: async () => value,
      watch: async (listener) => {
        publish = listener
        return () => {}
      },
      watchWorkspace: async (listener) => {
        publishWorkspace = listener
        return () => {}
      },
      localTools: localToolsFixture().port,
      resetLocal,
      signOut,
      cancel,
      save,
      prepareSync,
      start,
      reconcile,
      pick,
    },
    false,
    true,
  )
  return {
    session,
    resetLocal,
    signOut,
    cancel,
    save,
    prepareSync,
    start,
    reconcile,
    pick,
    emit: (next: CloudSnapshot) => publish(next),
    emitWorkspace: (next: CloudWorkspaceRecovery) => publishWorkspace(next),
  }
}

function Account({ session }: { session: CloudSession }) {
  const value = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  return <CloudAccount session={session} value={value} />
}

it("lets the user cancel a pending browser approval from Settings", async () => {
  const { session, cancel, signOut } = fixture({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "waiting",
  })
  render(<Account session={session} />)
  expect(await screen.findByText(/Waiting for browser approval/)).toBeVisible()
  const action = screen.getByRole("button", { name: "Cancel sign-in" })
  action.focus()
  fireEvent.click(action)
  expect(await screen.findByRole("button", { name: "Sign in with GitHub" })).toHaveFocus()
  expect(cancel).toHaveBeenCalledOnce()
  expect(signOut).not.toHaveBeenCalled()
})

it("requires an explicit local reset without warning about an earlier Cloud connection", async () => {
  const { session, resetLocal, emit } = fixture()
  render(<Account session={session} />)
  fireEvent.click(await screen.findByRole("button", { name: "Reset sign-in on this device" }))
  expect(resetLocal).not.toHaveBeenCalled()
  expect(screen.getByText(/This clears local sign-in only/)).toBeVisible()
  fireEvent.click(screen.getByRole("button", { name: "Reset sign-in" }))
  await waitFor(() => expect(resetLocal).toHaveBeenCalledOnce())
  expect(await screen.findByRole("button", { name: "Sign in with GitHub" })).toBeEnabled()
  expect(screen.queryByText(/Cloud has not confirmed an earlier sign-out/)).toBeNull()
  act(() =>
    emit({
      ...emptyCloudSnapshot,
      authEnabled: true,
      status: "signed_in",
      identity: { id: "synthetic-account", login: "Example" },
      destination: { id: "personal", name: "Personal", kind: "personal" },
      remoteRevocationUnconfirmed: true,
    }),
  )
  expect(screen.getByText("Signed in as Example")).toBeVisible()
  expect(screen.queryByText(/Cloud has not confirmed an earlier sign-out/)).toBeNull()
})

it("requires cleanup before signing in again when the connection requires it", async () => {
  const { session, emit } = fixture({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "signed_in",
    identity: { id: "synthetic-account", login: "Example" },
  })
  render(<Account session={session} />)
  await screen.findByText("Signed in as Example")
  act(() =>
    emit({
      ...emptyCloudSnapshot,
      authEnabled: true,
      status: "reauth_required",
      revocationPending: true,
    }),
  )
  expect(screen.getByText("Sign out on this device before you sign in again.")).toBeVisible()
  expect(screen.queryByRole("button", { name: "Sign in with GitHub" })).toBeNull()
})

it("refreshes eligible workspaces without signing in again or changing a saved Team policy", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const personal = { id: "synthetic-personal", name: "Personal", kind: "personal" } as const
  const saved = { id: "saved-team", name: "Saved Team", kind: "team" } as const
  const joined = { id: "joined-team", name: "Joined Team", kind: "team" } as const
  const policy = ongoingStatus({
    destinationId: saved.id,
    workspaceName: saved.name,
    status: "ready",
    policyRevision: "saved-team-policy",
    consentVersion: "desktop-project-sync-v1",
    projectChoices: { studio: true },
  })
  ongoing.status.mockResolvedValue(ongoingStatus({ teamStatuses: [policy] }))
  const workspaces = vi
    .fn<NonNullable<typeof port.workspaces>>()
    .mockResolvedValue([personal, saved])
  port.workspaces = workspaces
  const signIn = vi.spyOn(port, "begin")
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByText("2 workspaces")
  expect(screen.queryByRole("button", { name: "Open Cloud dashboard" })).toBeNull()

  workspaces.mockResolvedValue([personal, saved, joined])
  const refresh = screen.getByRole("button", { name: "Refresh workspaces" })
  refresh.focus()
  fireEvent.click(refresh)

  expect(await screen.findByText("3 workspaces")).toBeVisible()
  expect(refresh).toHaveFocus()
  expect(workspaces).toHaveBeenLastCalledWith({
    accountId: "synthetic-account",
    destinationId: personal.id,
  })
  expect(session.getSnapshot().teamStatuses?.[saved.id]).toEqual(policy)
  expect(session.getSnapshot().teamStatuses?.[joined.id]).toBeUndefined()
  expect(signIn).not.toHaveBeenCalled()
  expect(port.save).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(ongoing.setProject).not.toHaveBeenCalled()
  expect(ongoing.setEnabled).not.toHaveBeenCalled()
})

it("opens the configured dashboard without changing the desktop account", async () => {
  const { port, emit } = cloudProjectFixture()
  emit({ dashboardAvailable: true })
  const openDashboard = vi.fn<NonNullable<typeof port.openDashboard>>()
  openDashboard.mockRejectedValueOnce(new Error("Synthetic browser failure"))
  openDashboard.mockResolvedValueOnce(undefined)
  port.openDashboard = openDashboard
  const signIn = vi.spyOn(port, "begin")
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)

  const open = await screen.findByRole("button", { name: "Open Cloud dashboard" })
  fireEvent.click(open)
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Could not open the Cloud dashboard.",
  )
  await waitFor(() => expect(open).not.toHaveAttribute("aria-disabled"))
  fireEvent.click(open)
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull())
  expect(openDashboard).toHaveBeenCalledTimes(2)
  expect(openDashboard).toHaveBeenLastCalledWith()
  expect(signIn).not.toHaveBeenCalled()
  expect(screen.getByText("Signed in as Example")).toBeVisible()
})

it("joins in Settings and refreshes the destinations without changing sync", async () => {
  const { port, emit, ongoing } = cloudProjectFixture()
  const personal = { id: "synthetic-personal", name: "Personal", kind: "personal" } as const
  const team = { id: "synthetic-team", name: "Example Studio", kind: "team" } as const
  const link = `https://dashboard.example/join#token=${"a".repeat(43)}`
  emit({ dashboardAvailable: true, teamJoinAllowed: true })
  const workspaces = vi.fn<NonNullable<typeof port.workspaces>>().mockResolvedValue([personal])
  port.workspaces = workspaces
  const teamJoin = vi
    .fn<NonNullable<typeof port.teamJoin>>()
    .mockImplementation(async (_binding, action) => {
      if (action.action === "start")
        return { status: "started", attemptId: "synthetic-join-attempt" }
      workspaces.mockResolvedValue([personal, team])
      return { status: "joined", teamName: team.name, inSetup: false }
    })
  port.teamJoin = teamJoin
  const signIn = vi.spyOn(port, "begin")
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByText("1 workspace")

  fireEvent.click(screen.getByRole("button", { name: "Join a Team" }))
  fireEvent.change(screen.getByRole("textbox", { name: "Team invite link" }), {
    target: { value: link },
  })
  fireEvent.click(screen.getByRole("button", { name: "Check invite" }))

  expect(await screen.findByText("You joined Example Studio.")).toBeVisible()
  expect(await screen.findByText("2 workspaces")).toBeVisible()
  expect(teamJoin).toHaveBeenCalledWith(
    { accountId: "synthetic-account", destinationId: personal.id },
    { action: "start", link },
  )
  expect(signIn).not.toHaveBeenCalled()
  expect(port.save).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(ongoing.setProject).not.toHaveBeenCalled()
  expect(ongoing.setEnabled).not.toHaveBeenCalled()
})

it("requests Team access without signing out the current connection", async () => {
  const { port, emit } = cloudProjectFixture()
  emit({ dashboardAvailable: true, teamJoinAllowed: false })
  const signOut = vi.spyOn(port, "signOut")
  const current = await port.read()
  const begin = vi.spyOn(port, "begin").mockResolvedValueOnce({
    ...current,
    accessUpdate: "waiting",
  })
  render(<Account session={new CloudSession(port, false, true)} />)
  const updateAccess = await screen.findByRole("button", {
    name: "Update device access to join a Team",
  })
  expect(screen.getByText("Approve this device to join a Team.")).toBeVisible()
  expect(screen.queryByRole("button", { name: "Join a Team" })).toBeNull()
  expect(begin).not.toHaveBeenCalled()
  fireEvent.click(updateAccess)
  expect(await screen.findByRole("button", { name: "Cancel access update" })).toBeVisible()
  expect(begin).toHaveBeenCalledExactlyOnceWith("team_join")
  expect(screen.getByText("Signed in as Example")).toBeVisible()
  expect(signOut).not.toHaveBeenCalled()
})

it("canceling the reset confirmation does not call a native cleanup action", async () => {
  const { session, resetLocal, signOut } = fixture()
  render(<Account session={session} />)
  fireEvent.click(await screen.findByRole("button", { name: "Reset sign-in on this device" }))
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
  expect(resetLocal).not.toHaveBeenCalled()
  expect(signOut).not.toHaveBeenCalled()
  expect(screen.getByRole("button", { name: "Reset sign-in on this device" })).toBeVisible()
})

it("resets only the team that needs local recovery without leaving a remote warning", async () => {
  const accountId = "synthetic-account"
  const team = ongoingStatus({
    accountId,
    destinationId: "synthetic-team",
    workspaceName: "Example Team",
    status: "reauth_required",
    localResetAvailable: true,
    remoteRevocationUnconfirmed: true,
  })
  const otherTeam = ongoingStatus({
    ...team,
    destinationId: "other-team",
    workspaceName: "Other Team",
    localResetAvailable: false,
  })
  const { port, ongoing } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(ongoingStatus({ teamStatuses: [team, otherTeam] }))
  const resetWorkspace = vi.fn(async () => "signed_out" as const)
  port.resetWorkspace = resetWorkspace
  const resetLocal = vi.spyOn(port, "resetLocal")
  const signOut = vi.spyOn(port, "signOut")
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  fireEvent.click(
    await screen.findByRole("button", { name: "Clear old connection for Example Team" }),
  )
  expect(
    screen.queryByRole("button", { name: "Clear old connection for Other Team" }),
  ).toBeNull()
  expect(resetWorkspace).not.toHaveBeenCalled()
  expect(
    screen.getByText(/does not close it in Cloud or change which folders sync/),
  ).toBeVisible()
  resetWorkspace.mockRejectedValueOnce(new Error("Synthetic keyring failure"))
  const confirm = screen.getByRole("button", { name: "Confirm cleanup for Example Team" })
  fireEvent.click(confirm)
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Could not clear the old Team connection",
  )
  await waitFor(() => expect(confirm).toBeEnabled())
  fireEvent.click(confirm)
  await waitFor(() =>
    expect(resetWorkspace).toHaveBeenLastCalledWith(
      { accountId, destinationId: "synthetic-personal" },
      "synthetic-team",
    ),
  )
  await waitFor(() =>
    expect(screen.queryByRole("button", { name: /clear.*Example Team/i })).toBeNull(),
  )
  expect(screen.queryByText(/earlier connection for Example Team/)).toBeNull()
  expect(screen.getByText("Signed in as Example")).toBeVisible()
  expect(session.getSnapshot().workspaceConnections?.["other-team"]).toMatchObject({
    localResetAvailable: false,
    remoteRevocationUnconfirmed: true,
  })
  expect(resetLocal).not.toHaveBeenCalled()
  expect(signOut).not.toHaveBeenCalled()
})

it("uses legacy cleanup events only for the signed-in account", async () => {
  const accountId = "synthetic-account"
  const { session, emitWorkspace } = fixture({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "signed_in",
    identity: { id: accountId, login: "Example" },
    destination: { id: "synthetic-personal", name: "Personal", kind: "personal" },
  })
  render(<Account session={session} />)
  await screen.findByText("Signed in as Example")
  const team: CloudWorkspaceRecovery = {
    accountId,
    workspaceId: "synthetic-team",
    workspaceName: "Example Team",
    remoteRevocationUnconfirmed: true,
  }
  act(() => emitWorkspace(team))
  act(() => emitWorkspace({ ...team, accountId: "other-account", localResetAvailable: true }))
  expect(
    screen.queryByRole("button", { name: "Clear old connection for Example Team" }),
  ).toBeNull()
  act(() => emitWorkspace({ ...team, localResetAvailable: true }))
  expect(
    screen.getByRole("button", { name: "Clear old connection for Example Team" }),
  ).toBeEnabled()
})

it("requires source selection and sends account-bound folder changes from Settings", async () => {
  const { session, save } = fixture({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "signed_in",
    identity: { id: "synthetic-account", login: "Example" },
    destination: { id: "synthetic-personal", name: "Personal", kind: "personal" },
    draft: {
      screen: "folders",
      folders: [{ path: "/Example/Project", selected: false }],
    },
  })
  const container = document.createElement("div")
  document.body.append(container)
  render(<Account session={session} />, { container })
  const source = await screen.findByRole("switch", {
    name: "Select /Example/Project for Cloud capture",
  })
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  const row = screen.getByText("Cloud folders").closest<HTMLElement>("[data-settings-control]")!
  row.scrollIntoView = vi.fn()
  new SettingsTargetFocus().attach(container, "sessions", "cloudFolders", 1)()
  expect(source).toHaveFocus()
  expect(source).not.toBeChecked()
  expect(save).not.toHaveBeenCalled()
  fireEvent.click(source)
  await waitFor(() =>
    expect(screen.getByRole("switch", { name: /Select \/Example\/Project/ })).toBeChecked(),
  )
  expect(save).toHaveBeenCalledWith(
    { screen: "folders", folders: [{ path: "/Example/Project", selected: true }] },
    { accountId: "synthetic-account", destinationId: "synthetic-personal" },
    true,
  )
  expect(screen.queryByText(/It does not sign out your browser/)).not.toBeInTheDocument()
  expect(row).toHaveAttribute("data-settings-control", "cloudFolders")
})

it("keeps revocation available when folder setup cannot load", async () => {
  const { session, signOut } = fixture({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "signed_in",
    identity: { id: "synthetic-account", login: "Example" },
    destination: { id: "synthetic-personal", name: "Personal", kind: "personal" },
    setupError: "Could not read Cloud setup.",
  })
  render(<Account session={session} />)
  const logout = await screen.findByRole("button", { name: "Sign out" })
  expect(logout).toBeEnabled()
  expect(screen.queryByRole("button", { name: /Choose folders/ })).not.toBeInTheDocument()
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  fireEvent.click(logout)
  await waitFor(() => expect(signOut).toHaveBeenCalledOnce())
})

const desktopCapture: CloudSnapshot = {
  ...emptyCloudSnapshot,
  authEnabled: true,
  captureEnabled: true,
  status: "signed_in",
  identity: { id: "synthetic-account", login: "Example" },
  destination: { id: "personal", name: "Personal", kind: "personal" },
  draft: {
    screen: "complete",
    folders: [{ path: "/Example/claude/projects/project", selected: true }],
  },
}

it("saves a desktop capture request from Settings after onboarding without claiming upload", async () => {
  const { session, prepareSync, start } = fixture(desktopCapture)
  render(<Account session={session} />)
  const save = await screen.findByRole("button", { name: "Save capture request" })
  expect(save).toBeEnabled()
  expect(
    screen.queryByRole("button", { name: /Choose Claude|Choose OpenCode/ }),
  ).not.toBeInTheDocument()
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  expect(save).toBeEnabled()
  fireEvent.click(save)
  await waitFor(() => expect(start).toHaveBeenCalledOnce())
  expect(prepareSync).toHaveBeenCalledOnce()
  expect(
    await screen.findByText("Capture request saved on this device. Upload unavailable."),
  ).toBeVisible()
  expect(screen.queryByText(/Sessions syncing/)).not.toBeInTheDocument()
})

it.each([
  { draft: { screen: "complete" as const, folders: [] } },
  { destination: null },
  { destination: { id: "team", name: "Team", kind: "team" as const } },
  { status: "reauth_required" as const },
  { setupError: "Could not read Cloud setup." },
])(
  "disables desktop saves without selected sources and current personal context: %j",
  async (patch) => {
    const { session, prepareSync } = fixture({ ...desktopCapture, ...patch })
    render(<Account session={session} />)
    const save = await screen.findByRole("button", { name: "Save capture request" })
    expect(save).toBeDisabled()
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
    fireEvent.click(save)
    expect(prepareSync).not.toHaveBeenCalled()
  },
)

it("does not save or authorize a request when source selection changes", async () => {
  const { session, start } = fixture(desktopCapture)
  render(<Account session={session} />)
  const save = await screen.findByRole("button", { name: "Save capture request" })
  expect(save).toBeEnabled()
  fireEvent.click(screen.getByRole("switch"))
  await waitFor(() => expect(save).toBeDisabled())
  expect(start).not.toHaveBeenCalled()
})

it("uses an already discovered OpenCode source without a Settings picker", async () => {
  const { session, pick, save, prepareSync } = fixture({
    ...desktopCapture,
    draft: {
      screen: "complete",
      folders: [
        {
          path: "/Example/opencode/opencode.db",
          sourceKind: "opencode_sqlite_v2",
          selected: true,
        },
      ],
    },
  })
  render(<Account session={session} />)
  const source = await screen.findByRole("switch", {
    name: "Select /Example/opencode/opencode.db for Cloud capture",
  })
  expect(source).toBeChecked()
  expect(pick).not.toHaveBeenCalled()
  expect(save).not.toHaveBeenCalled()
  expect(screen.queryByRole("button", { name: /Choose OpenCode/ })).not.toBeInTheDocument()
  expect(screen.getByText("OpenCode V2: /Example/opencode/opencode.db")).toBeVisible()
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  expect(prepareSync).not.toHaveBeenCalled()
  fireEvent.click(screen.getByRole("button", { name: "Save capture request" }))
  await waitFor(() => expect(prepareSync).toHaveBeenCalledOnce())
  expect(prepareSync).toHaveBeenCalledWith(
    expect.any(Array),
    expect.any(String),
    expect.objectContaining({
      version: "native-full-content-v2",
      fullContent: true,
      attachments: false,
      selectedSources: [
        { sourceKind: "opencode_sqlite_v2", path: "/Example/opencode/opencode.db" },
      ],
    }),
  )
})
