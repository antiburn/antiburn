import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { expect, it, vi } from "vitest"
import {
  emptyCloudSnapshot,
  type CloudSnapshot,
  type CloudWorkspaceRecovery,
} from "../../lib/cloud"
import { CloudSession } from "../../onboarding/CloudSession"
import { cloudCaptureUnsupported } from "../../onboarding/cloudProjectScope"
import { CloudAccount } from "./CloudAccount"
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
  const { port, ongoing } = cloudProjectFixture()
  const session = new CloudSession(
    {
      ...port,
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
    ongoing,
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

it("recovers workspace access without reading folders when capture is unsupported", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  emit({ captureEnabled: false, setupError: cloudCaptureUnsupported })
  const personal = { id: "synthetic-personal", name: "Personal", kind: "personal" } as const
  const team = { id: "joined-team", name: "Joined Team", kind: "team" } as const
  const workspaces = vi.fn<NonNullable<typeof port.workspaces>>().mockResolvedValue([personal])
  port.workspaces = workspaces
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByText("1 workspace")

  const refresh = screen.getByRole("button", { name: "Refresh workspaces" })
  const accessDenied = "You do not have access to the selected Cloud workspace."
  workspaces.mockRejectedValueOnce(new Error("Cloud workspace access denied."))
  fireEvent.click(refresh)
  expect(await screen.findByText(accessDenied)).toBeVisible()

  workspaces.mockResolvedValue([personal, team])
  fireEvent.click(refresh)

  expect(await screen.findByText("2 workspaces")).toBeVisible()
  expect(screen.queryByText(accessDenied)).toBeNull()
  expect(workspaces).toHaveBeenCalledTimes(3)
  expect(workspaces).toHaveBeenLastCalledWith({
    accountId: "synthetic-account",
    destinationId: personal.id,
  })
  expect(ongoing.groups).not.toHaveBeenCalled()
  expect(session.getSnapshot().projects?.loaded).toBe(false)
  expect(port.save).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
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

it("offers browser workspace setup in Settings even without the dashboard shortcut", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const openSetup = vi.fn().mockResolvedValue(undefined)
  port.openSetup = openSetup
  const signIn = vi.spyOn(port, "begin")
  render(<Account session={new CloudSession(port, false, true)} />)

  fireEvent.click(await screen.findByRole("button", { name: "Finish setup in browser" }))
  expect(openSetup).toHaveBeenCalledExactlyOnceWith()
  expect(screen.queryByRole("button", { name: "Open Cloud dashboard" })).toBeNull()
  expect(signIn).not.toHaveBeenCalled()
  expect(port.save).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("keeps session access updates cancellable in Settings after sign-in is lost", async () => {
  const { port, emit, ongoing } = cloudProjectFixture()
  emit({ sessionReadsAllowed: false, sessionReadUpdateAvailable: true })
  const signOut = vi.spyOn(port, "signOut")
  const current = await port.read()
  const disconnected: CloudSnapshot = {
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "reauth_required",
  }
  const begin = vi
    .spyOn(port, "begin")
    .mockResolvedValueOnce({ ...current, accessUpdate: "waiting" })
    .mockResolvedValueOnce({ ...disconnected, status: "waiting" })
  const cancel = vi.spyOn(port, "cancel").mockResolvedValue(disconnected)
  render(<Account session={new CloudSession(port, false, true)} />)
  const updateAccess = await screen.findByRole("button", {
    name: "Update device access to search and read Cloud sessions",
  })
  expect(screen.getByText(/approve this device to search and read/i)).toBeVisible()
  expect(begin).not.toHaveBeenCalled()
  fireEvent.click(updateAccess)
  expect(await screen.findByRole("button", { name: "Cancel access update" })).toBeVisible()
  expect(begin).toHaveBeenCalledExactlyOnceWith("session_reads")
  expect(screen.getByText("Signed in as Example")).toBeVisible()
  expect(screen.getByText(/your current connection stays active/i)).toBeVisible()

  act(() => emit({ ...disconnected, destination: null, accessUpdate: "waiting" }))
  expect(screen.getByText("Not signed in")).toBeVisible()
  expect(screen.queryByText(/your current connection stays active/i)).toBeNull()
  expect(screen.queryByRole("button", { name: "Sign in with GitHub" })).toBeNull()
  expect(screen.getByRole("button", { name: "Sign out" })).toBeEnabled()
  fireEvent.click(screen.getByRole("button", { name: "Cancel access update" }))

  const signIn = await screen.findByRole("button", { name: "Sign in with GitHub" })
  expect(screen.queryByRole("button", { name: "Cancel access update" })).toBeNull()
  fireEvent.click(signIn)
  expect(await screen.findByRole("button", { name: "Cancel sign-in" })).toBeVisible()
  expect(begin).toHaveBeenLastCalledWith(undefined)
  expect(cancel).toHaveBeenCalledOnce()
  expect(signOut).not.toHaveBeenCalled()
  expect(port.save).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
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

it.each([
  { destination: null },
  { destination: { id: "team", name: "Team", kind: "team" as const } },
  { status: "reauth_required" as const },
  { setupError: "Could not read Cloud setup." },
])("does not grant folder sync without current Personal authority: %j", async (patch) => {
  const { port, ongoing, emit } = cloudProjectFixture()
  emit(patch)
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await waitFor(() => expect(session.getSnapshot().loaded).toBe(true))
  if ("setupError" in patch) {
    const start = screen.getByRole("button", { name: "Start syncing" })
    expect(start).toHaveAttribute("aria-disabled", "true")
    await act(async () => fireEvent.click(start))
  } else {
    expect(screen.queryByRole("button", { name: "Start syncing" })).not.toBeInTheDocument()
  }
  await act(async () => session.startPendingProjects(session.getSnapshot(), false))
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(ongoing.setProject).not.toHaveBeenCalled()
})
