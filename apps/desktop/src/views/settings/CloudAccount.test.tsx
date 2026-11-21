import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { expect, it, vi } from "vitest"
import { emptyCloudSnapshot, desktopCloudPort, type CloudSnapshot } from "../../lib/cloud"
import { CloudSession } from "../../onboarding/CloudSession"
import { CloudAccount } from "./CloudAccount"
import { SettingsTargetFocus } from "./SettingsTargetFocus"
import { localToolsFixture } from "../../onboarding/__tests__/cloudProjectFixtures"

function fixture(initial?: CloudSnapshot) {
  let value: CloudSnapshot = initial ?? {
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "signed_out",
  }
  let publish: (value: CloudSnapshot) => void = () => {}
  const read = vi.fn(async () => value)
  const signOut = vi.fn(async () => value)
  const begin = vi.fn(async (): Promise<CloudSnapshot> => ({ ...value, status: "waiting" }))
  const cancel = vi.fn(async (): Promise<CloudSnapshot> => ({ ...value, status: "cancelled" }))
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
      read,
      watch: async (listener) => {
        publish = listener
        return () => {}
      },
      localTools: localToolsFixture().port,
      signOut,
      begin,
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
    read,
    signOut,
    begin,
    cancel,
    save,
    prepareSync,
    start,
    reconcile,
    pick,
    emit: (next: CloudSnapshot) => publish(next),
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

it("shows a closed account and offers the regular sign-in action", async () => {
  const { session, begin } = fixture({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "reauth_required",
  })
  let openBrowser!: (snapshot: CloudSnapshot) => void
  begin.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        openBrowser = resolve
      }),
  )
  render(<Account session={session} />)
  const signIn = await screen.findByRole("button", { name: "Sign in with GitHub" })
  expect(screen.getByText("Not signed in")).toBeVisible()
  expect(screen.getByText("Your sign-in ended. Sign in again to use Cloud.")).toBeVisible()
  expect(screen.queryByRole("button", { name: "Sign out" })).toBeNull()
  fireEvent.click(signIn)
  expect(screen.getByRole("button", { name: "Starting sign-in…" })).toBeDisabled()
  await act(async () =>
    openBrowser({ ...emptyCloudSnapshot, authEnabled: true, status: "waiting" }),
  )
  expect(screen.getByRole("button", { name: "Cancel sign-in" })).toBeEnabled()
})

it("keeps browser cancellation separate from sign-out", async () => {
  const { session, begin, cancel, emit } = fixture()
  render(<Account session={session} />)
  const github = await screen.findByRole("button", { name: "Sign in with GitHub" })
  expect(screen.getByText("Signed out on this device")).toBeVisible()
  expect(github).toBeEnabled()
  fireEvent.click(github)
  expect(await screen.findByText(/Waiting for browser approval/)).toBeVisible()
  expect(begin).toHaveBeenCalledOnce()
  expect(screen.queryByRole("button", { name: "Sign in with GitHub" })).toBeNull()
  fireEvent.click(screen.getByRole("button", { name: "Cancel sign-in" }))
  expect(await screen.findByText("Sign-in cancelled.")).toBeVisible()
  expect(cancel).toHaveBeenCalledOnce()
  expect(screen.queryByText(/confirm sign-out/)).toBeNull()
  act(() =>
    emit({
      ...emptyCloudSnapshot,
      authEnabled: true,
      status: "signed_in",
      identity: { id: "synthetic-account", login: "Example" },
      destination: { id: "personal", name: "Personal", kind: "personal" },
    }),
  )
  expect(screen.getByText("Signed in as Example")).toBeVisible()
  expect(screen.queryByText(/confirm sign-out/)).toBeNull()
})

it("allows sign-out when a saved sign-in cannot be checked", async () => {
  const { session, signOut } = fixture({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "sign_in_check_failed",
  })
  signOut.mockResolvedValueOnce({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "signed_out",
  })
  render(<Account session={session} />)
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Couldn’t check sign-in. Try again.",
  )
  expect(screen.getByRole("button", { name: "Try again" })).toBeEnabled()
  fireEvent.click(screen.getByRole("button", { name: "Sign out" }))
  expect(await screen.findByText("Signed out on this device")).toBeVisible()
  expect(screen.getByRole("button", { name: "Sign in with GitHub" })).toBeEnabled()
})

it("announces the sign-in check and its result when retrying", async () => {
  const { session, read } = fixture({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "sign_in_check_failed",
  })
  render(<Account session={session} />)
  const retry = await screen.findByRole("button", { name: "Try again" })
  let finishCheck!: (snapshot: CloudSnapshot) => void
  read.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finishCheck = resolve
      }),
  )
  const status = screen.getByRole("status")
  fireEvent.click(retry)
  expect(status).toHaveTextContent("Checking sign-in…")
  expect(screen.queryByRole("alert")).toBeNull()
  expect(retry).toBeDisabled()
  expect(screen.getByRole("button", { name: "Sign out" })).toBeDisabled()
  await act(async () =>
    finishCheck({ ...emptyCloudSnapshot, authEnabled: true, status: "signed_out" }),
  )
  expect(status).toHaveTextContent("Signed out on this device")
  expect(screen.getByRole("button", { name: "Sign in with GitHub" })).toBeEnabled()
})

it("does not claim sign-out or show sign-in when local sign-out fails", async () => {
  const { session, signOut } = fixture({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "sign_in_check_failed",
  })
  signOut.mockResolvedValueOnce({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "sign_out_failed",
  })
  render(<Account session={session} />)
  fireEvent.click(await screen.findByRole("button", { name: "Sign out" }))
  expect(await screen.findByText("Couldn’t sign out. Try again.")).toBeVisible()
  expect(screen.queryByText("Signed out on this device")).toBeNull()
  expect(screen.queryByRole("button", { name: "Try again" })).toBeNull()
  expect(screen.queryByRole("button", { name: "Sign in with GitHub" })).toBeNull()
  expect(screen.getByRole("button", { name: "Sign out" })).toBeEnabled()
})

it("shows sign-out progress and then the normal sign-in button", async () => {
  const { session, signOut } = fixture({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "signed_in",
    identity: { id: "synthetic-account", login: "Example" },
  })
  let finishSignOut!: (snapshot: CloudSnapshot) => void
  signOut.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finishSignOut = resolve
      }),
  )
  render(<Account session={session} />)
  expect(screen.getByText("Checking Cloud…")).toBeVisible()
  fireEvent.click(await screen.findByRole("button", { name: "Sign out" }))
  expect(screen.getByText("Signing out…")).toBeVisible()
  expect(screen.getByRole("button", { name: "Sign out" })).toBeDisabled()
  await act(async () =>
    finishSignOut({ ...emptyCloudSnapshot, authEnabled: true, status: "signed_out" }),
  )
  expect(screen.getByRole("button", { name: "Sign in with GitHub" })).toBeEnabled()
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
  await waitFor(() => expect(source).toBeChecked())
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
