import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { expect, it, vi } from "vitest"
import { emptyCloudSnapshot, nativeCloudPort, type CloudSnapshot } from "../../lib/cloud"
import { CloudSession } from "../../onboarding/CloudSession"
import { CloudAccount } from "./CloudAccount"

function fixture() {
  let value: CloudSnapshot = {
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
    },
    false,
    true,
  )
  return { session, resetLocal, signOut, emit: (next: CloudSnapshot) => publish(next) }
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
