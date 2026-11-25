import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { expect, it, vi } from "vitest"
import { CLOUD_BUSY_MESSAGE } from "../../lib/cloud"
import { CloudSession } from "../../onboarding/CloudSession"
import { cloudProjectFixture } from "../../onboarding/__tests__/cloudProjectFixtures"
import { CloudAccount } from "./CloudAccount"

function Account({ session }: { session: CloudSession }) {
  const value = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  return <CloudAccount session={session} value={value} />
}

it("keeps sign-in available when opening the browser fails", async () => {
  const { port } = cloudProjectFixture()
  const signedOut = {
    ...(await port.read()),
    status: "signed_out" as const,
    identity: null,
    destination: null,
  }
  vi.mocked(port.read).mockResolvedValue(signedOut)
  port.begin = vi.fn(async () => signedOut).mockRejectedValueOnce(new Error(CLOUD_BUSY_MESSAGE))
  render(<Account session={new CloudSession(port, false, true)} />)
  const signIn = await screen.findByRole("button", { name: "Sign in with GitHub" })
  fireEvent.click(signIn)
  expect(await screen.findByRole("alert")).toHaveTextContent("Cloud is busy.")
  expect(screen.queryByRole("button", { name: "Check again" })).not.toBeInTheDocument()
  fireEvent.click(signIn)
  expect(port.begin).toHaveBeenCalledTimes(2)
  await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument())
})

it.each([
  ["read failure", "Could not check Cloud sign-in."],
  ["missing workspace", "Could not verify your Cloud workspaces."],
] as const)("checks again in Settings after %s", async (failure, message) => {
  const { port, ongoing } = cloudProjectFixture()
  const healthy = await port.read()
  let finishVerification!: () => void
  const workspaces = vi.mocked(port.workspaces!).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finishVerification = () => resolve([healthy.destination!])
      }),
  )
  const read = vi.fn<typeof port.read>()
  for (let attempt = 0; attempt < 3; attempt++)
    if (failure === "read failure") {
      read.mockRejectedValueOnce(new Error("Synthetic account read failure"))
    } else {
      read.mockResolvedValueOnce({ ...healthy, destination: null })
    }
  port.read = read.mockResolvedValue(healthy)
  render(<Account session={new CloudSession(port, false, true)} />)

  expect(await screen.findByText(message)).toBeVisible()
  expect(read).toHaveBeenCalledTimes(3)
  expect(ongoing.groups).not.toHaveBeenCalled()

  fireEvent.click(screen.getByRole("button", { name: "Check again" }))
  expect(screen.getByRole("button", { name: "Check again" })).toBeDisabled()
  expect(screen.queryByText("Antiburn Cloud is coming soon")).not.toBeInTheDocument()
  expect(await screen.findByText("Signed in as Example")).toBeVisible()
  expect(screen.queryByRole("button", { name: "Check again" })).not.toBeInTheDocument()
  expect(await screen.findByText("Checking Cloud workspace access…")).toBeVisible()
  const binding = { accountId: "synthetic-account", destinationId: "synthetic-personal" }
  await waitFor(() => expect(workspaces).toHaveBeenCalledExactlyOnceWith(binding))
  expect(screen.getByText("Studio", { selector: "strong" })).toBeVisible()
  expect(screen.queryByRole("switch", { name: /Sync Studio/ })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Start syncing" })).toHaveAttribute(
    "aria-disabled",
    "true",
  )
  expect(ongoing.enable).not.toHaveBeenCalled()

  finishVerification()
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeVisible()
  expect(ongoing.groups).toHaveBeenCalledExactlyOnceWith(binding, null)
  expect(read).toHaveBeenCalledTimes(4)
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(ongoing.setProject).not.toHaveBeenCalled()
})
