import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, expect, test, vi } from "vitest"
import type { CloudSnapshot } from "../lib/cloud"
import { DeviceAccessUpdate } from "./DeviceAccessUpdate"

afterEach(cleanup)

const connection: CloudSnapshot = {
  authEnabled: true,
  dashboardAvailable: true,
  captureUpdateAvailable: true,
  sessionReadsAllowed: false,
  sessionReadUpdateAvailable: true,
  captureEnabled: true,
  syncEnabled: false,
  status: "signed_in",
  identity: { id: "synthetic-account", login: "Example" },
  draft: { screen: "identity", folders: [] },
}

test("requests only the access the user chose", () => {
  const update = vi.fn(async () => true)
  render(
    <DeviceAccessUpdate value={connection} busy={false} update={update} cancel={vi.fn()} />,
  )
  fireEvent.click(
    screen.getByRole("button", { name: "Update device access to upload sessions" }),
  )
  expect(update).toHaveBeenCalledExactlyOnceWith("capture")
  fireEvent.click(
    screen.getByRole("button", {
      name: "Update device access to search and read Cloud sessions",
    }),
  )
  expect(update).toHaveBeenLastCalledWith("session_reads")
})

test("does not offer CLI access where the session read update is unavailable", () => {
  render(
    <DeviceAccessUpdate
      value={{ ...connection, sessionReadUpdateAvailable: false }}
      busy={false}
      update={vi.fn()}
      cancel={vi.fn()}
    />,
  )
  expect(
    screen.queryByRole("button", {
      name: "Update device access to search and read Cloud sessions",
    }),
  ).toBeNull()
  expect(
    screen.getByRole("button", { name: "Update device access to upload sessions" }),
  ).toBeEnabled()
})

test("keeps the current connection visible while access is pending or denied", () => {
  const update = vi.fn()
  const cancel = vi.fn()
  const { rerender } = render(
    <DeviceAccessUpdate
      value={{ ...connection, accessUpdate: "waiting" }}
      busy={false}
      update={update}
      cancel={cancel}
    />,
  )
  expect(screen.getByRole("status")).toHaveTextContent("Your current connection stays active.")
  fireEvent.click(screen.getByRole("button", { name: "Cancel access update" }))
  expect(cancel).toHaveBeenCalledOnce()
  rerender(
    <DeviceAccessUpdate
      value={{ ...connection, accessUpdate: "denied" }}
      busy={false}
      update={update}
      cancel={cancel}
    />,
  )
  expect(screen.getByRole("status")).toHaveTextContent("Device access did not change.")
  expect(
    screen.getByRole("button", { name: "Update device access to upload sessions" }),
  ).toBeEnabled()
})
