import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { expect, it, vi } from "vitest"
import { emptyCloudSnapshot, nativeCloudPort, type CloudSnapshot } from "../../lib/cloud"
import { CloudSession } from "../../onboarding/CloudSession"
import { CloudAccount } from "./CloudAccount"
import { SettingsTargetFocus } from "./SettingsTargetFocus"

function fixture(initial?: CloudSnapshot) {
  let value: CloudSnapshot = initial ?? {
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "revocation_unconfirmed",
    revocationPending: true,
    localResetAvailable: true,
  }
  let publish: (value: CloudSnapshot) => void = () => {}
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
  const save = vi.fn(async (draft: CloudSnapshot["draft"]) => {
    value = { ...value, draft }
    return value
  })
  const session = new CloudSession(
    {
      ...nativeCloudPort,
      read: async () => value,
      watch: async (listener) => {
        publish = listener
        return () => {}
      },
      agent: async () => "unavailable",
      resetLocal,
      signOut,
      save,
    },
    false,
    true,
  )
  return { session, resetLocal, signOut, save, emit: (next: CloudSnapshot) => publish(next) }
}

function Account({ session }: { session: CloudSession }) {
  const value = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  return <CloudAccount session={session} value={value} />
}

it("requires an explicit reset and keeps the remote warning while reconnecting", async () => {
  const { session, resetLocal, emit } = fixture()
  render(<Account session={session} />)
  fireEvent.click(await screen.findByRole("button", { name: "Reset local connection" }))
  expect(resetLocal).not.toHaveBeenCalled()
  expect(screen.getByText(/this does not revoke the old connection/)).toBeVisible()
  fireEvent.click(screen.getByRole("button", { name: "Reset and keep warning" }))
  await waitFor(() => expect(resetLocal).toHaveBeenCalledOnce())
  expect(await screen.findByRole("button", { name: "Sign in" })).toBeEnabled()
  expect(
    screen.getByText(/Cloud has not confirmed revocation of an earlier connection/),
  ).toBeVisible()
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
  expect(
    screen.getByText(/Cloud has not confirmed revocation of an earlier connection/),
  ).toBeVisible()
})

it("canceling the reset confirmation does not call a native cleanup action", async () => {
  const { session, resetLocal, signOut } = fixture()
  render(<Account session={session} />)
  fireEvent.click(await screen.findByRole("button", { name: "Reset local connection" }))
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
  expect(resetLocal).not.toHaveBeenCalled()
  expect(signOut).not.toHaveBeenCalled()
  expect(screen.getByRole("button", { name: "Reset local connection" })).toBeVisible()
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
  const consent = screen.getByRole("checkbox", { name: /upload full native session content/ })
  expect(consent).toBeDisabled()
  const row = screen.getByText("Cloud folders").closest<HTMLElement>("[data-settings-control]")!
  row.scrollIntoView = vi.fn()
  new SettingsTargetFocus().attach(container, "sessions", "cloudFolders", 1)()
  expect(screen.getByRole("button", { name: "Choose folders…" })).toHaveFocus()
  expect(source).not.toBeChecked()
  expect(save).not.toHaveBeenCalled()
  fireEvent.click(source)
  await waitFor(() => expect(consent).toBeEnabled())
  expect(save).toHaveBeenCalledWith(
    { screen: "folders", folders: [{ path: "/Example/Project", selected: true }] },
    { accountId: "synthetic-account", destinationId: "synthetic-personal" },
    true,
  )
  expect(screen.getByText(/It does not sign out your browser/)).toBeVisible()
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
  expect(screen.getByRole("button", { name: "Choose folders…" })).toBeDisabled()
  expect(screen.getByRole("checkbox")).toBeDisabled()
  fireEvent.click(logout)
  await waitFor(() => expect(signOut).toHaveBeenCalledOnce())
})
