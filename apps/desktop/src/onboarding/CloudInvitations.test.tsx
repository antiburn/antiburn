import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { CloudInvitations } from "./CloudInvitations"
import type {
  CloudInvitationPort,
  InvitationReply,
  InvitationState,
} from "./cloudInvitationPort"

const checked: InvitationReply = {
  kind: "list",
  listing: {
    state: "checked",
    invitations: [
      { id: "invite-1", workspace_name: "Example Team", workspace_slug: "example" },
    ],
  },
}

const attempt = (
  state: InvitationState,
  join = true,
): Extract<InvitationReply, { kind: "attempt" }> => ({
  kind: "attempt",
  id: "attempt-1",
  join,
  state,
  workspace_id: state === "joined" ? "team-1" : null,
})

async function click(name: string) {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name }))
  })
}

async function open(port: CloudInvitationPort, onJoined = vi.fn()) {
  const component = await act(async () =>
    render(<CloudInvitations accountId="account-1" port={port} onJoined={onJoined} />),
  )
  return { ...component, onJoined }
}

describe("Cloud workspace invitations", () => {
  beforeEach(() => vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "performance"] }))

  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it("does not join on return from the browser", async () => {
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce(checked)
      .mockResolvedValueOnce(attempt("waiting"))
      .mockResolvedValueOnce({ ...attempt("ready"), workspace_name: "Example Team" })
      .mockResolvedValueOnce(attempt("joined"))
    const { onJoined } = await open(port)
    await click("Verify invitation to Example Team")
    expect(port).toHaveBeenLastCalledWith("account-1", {
      action: "start",
      invitation_id: "invite-1",
      workspace_name: "Example Team",
    })
    expect(screen.queryByRole("button", { name: "Join workspace" })).toBeNull()
    await act(() => vi.advanceTimersByTimeAsync(2_000))
    expect(port).toHaveBeenLastCalledWith("account-1", { action: "status", id: "attempt-1" })
    expect(screen.getByRole("status")).toHaveTextContent("Join Example Team")
    expect(onJoined).not.toHaveBeenCalled()
    await click("Join workspace")
    expect(port).toHaveBeenLastCalledWith("account-1", { action: "finish", id: "attempt-1" })
    expect(screen.getByRole("status")).toHaveTextContent("You joined the workspace.")
    expect(onJoined).toHaveBeenCalledOnce()
  })

  it("checks the same attempt after an uncertain finish", async () => {
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce(attempt("ready"))
      .mockResolvedValueOnce({ kind: "uncertain", id: "attempt-1", join: true })
      .mockResolvedValueOnce(attempt("joined"))
    const { onJoined } = await open(port)
    await click("Join workspace")
    expect(screen.queryByRole("button", { name: "Join workspace" })).toBeNull()
    expect(onJoined).not.toHaveBeenCalled()
    await click("Check result")
    expect(port.mock.calls.map(([, request]) => request)).toEqual([
      { action: "list" },
      { action: "finish", id: "attempt-1" },
      { action: "status", id: "attempt-1" },
    ])
    expect(onJoined).toHaveBeenCalledOnce()
  })

  it("checks the list without a target and distinguishes a verified empty list", async () => {
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce({ kind: "list", listing: { state: "check_required" } })
      .mockResolvedValueOnce(attempt("waiting", false))
      .mockResolvedValueOnce(attempt("checked", false))
      .mockResolvedValueOnce({ kind: "list", listing: { state: "checked", invitations: [] } })
    await open(port)
    await click("Check in browser")
    expect(port).toHaveBeenLastCalledWith("account-1", { action: "start", invitation_id: null })
    await act(() => vi.advanceTimersByTimeAsync(2_000))
    expect(screen.queryByRole("button", { name: "Join workspace" })).toBeNull()
    await click("Review invitations")
    expect(screen.getByRole("status")).toHaveTextContent("No pending workspace invitations.")
  })

  it("allows cancellation and stops polling", async () => {
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce(attempt("waiting"))
      .mockResolvedValueOnce(attempt("canceled"))
    await open(port)
    await click("Cancel verification")
    expect(port).toHaveBeenLastCalledWith("account-1", { action: "cancel", id: "attempt-1" })
    expect(screen.getByRole("status")).toHaveTextContent("Invitation verification canceled.")
    await act(() => vi.advanceTimersByTimeAsync(30_000))
    expect(port).toHaveBeenCalledTimes(2)
  })

  it("stops polling at a fixed deadline", async () => {
    const port = vi.fn<CloudInvitationPort>().mockResolvedValue(attempt("waiting"))
    await open(port)
    await act(() => vi.advanceTimersByTimeAsync(5 * 60_000))
    expect(screen.getByRole("status")).toHaveTextContent("Verification timed out.")
    const calls = port.mock.calls.length
    await act(() => vi.advanceTimersByTimeAsync(30_000))
    expect(port).toHaveBeenCalledTimes(calls)
  })

  it.each([
    ["wrong_account", "Your browser used a different account."],
    ["unavailable", "Could not check invitations for this account."],
    ["no_seat", "This workspace has no open seats."],
    ["closed", "This invitation is no longer open."],
  ] as const)(
    "shows the %s result without a join or a destination change",
    async (state, copy) => {
      const port = vi.fn<CloudInvitationPort>().mockResolvedValue(attempt(state))
      const { onJoined } = await open(port)
      expect(screen.getByRole("status")).toHaveTextContent(copy)
      expect(screen.queryByRole("button", { name: "Join workspace" })).toBeNull()
      expect(onJoined).not.toHaveBeenCalled()
    },
  )

  it("does not apply an old attempt result after the signed-in account changes", async () => {
    let finish: (reply: InvitationReply) => void = () => {}
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce(attempt("ready"))
      .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
      .mockResolvedValueOnce({ kind: "list", listing: { state: "unavailable" } })
    const { onJoined, rerender } = await open(port)
    await click("Join workspace")
    await act(async () => {
      rerender(<CloudInvitations accountId="account-2" port={port} onJoined={onJoined} />)
    })
    await act(async () => finish(attempt("joined")))
    expect(port).toHaveBeenLastCalledWith("account-2", { action: "list" })
    expect(screen.getByRole("status")).toHaveTextContent(
      "Workspace invitations are unavailable.",
    )
    expect(onJoined).not.toHaveBeenCalled()
  })
})
