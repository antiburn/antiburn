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
      { id: "invite-2", workspace_name: "Example Team", workspace_slug: "example-other" },
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

async function open(port: CloudInvitationPort, showConfirmedOnly = false) {
  const onJoined = vi.fn()
  const component = await act(async () =>
    render(
      <CloudInvitations
        accountId="account-1"
        port={port}
        onJoined={onJoined}
        showConfirmedOnly={showConfirmedOnly}
      />,
    ),
  )
  return { ...component, onJoined }
}

describe("Cloud workspace invitations", () => {
  beforeEach(() => vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "performance"] }))

  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it("hides an empty onboarding list but keeps browser verification available", async () => {
    const port = vi.fn<CloudInvitationPort>().mockResolvedValue({
      kind: "list",
      listing: { state: "checked", invitations: [] },
    })
    await act(async () => {
      render(
        <CloudInvitations accountId="account-1" port={port} onJoined={vi.fn()} hideEmpty />,
      )
    })
    expect(screen.queryByRole("region", { name: "Workspace invitations" })).toBeNull()
    port.mockResolvedValue({ kind: "list", listing: { state: "check_required" } })
    await act(async () => {
      render(
        <CloudInvitations accountId="account-2" port={port} onJoined={vi.fn()} hideEmpty />,
      )
    })
    expect(screen.getByRole("button", { name: "Check in browser" })).toBeVisible()
  })

  it.each<[string, InvitationReply]>([
    ["a browser check is required", { kind: "list", listing: { state: "check_required" } }],
    ["the list is unavailable", { kind: "list", listing: { state: "unavailable" } }],
    ["the list is empty", { kind: "list", listing: { state: "checked", invitations: [] } }],
    ["there is a list-only attempt", attempt("ready", false)],
  ])("hides Settings invitations when %s", async (_description, reply) => {
    const port = vi.fn<CloudInvitationPort>().mockResolvedValueOnce(reply)
    await open(port, true)
    expect(screen.queryByRole("region", { name: "Workspace invitations" })).toBeNull()
    await act(() => vi.advanceTimersByTimeAsync(2_000))
    expect(port.mock.calls).toEqual([["account-1", { action: "list" }]])
  })

  it.each<[string, InvitationReply, boolean]>([
    ["a confirmed invitation list", checked, true],
    ["another required check", { kind: "list", listing: { state: "check_required" } }, false],
  ])("dismisses a completed list check in Settings and gets %s", async (_, reply, visible) => {
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce(attempt("checked", false))
      .mockResolvedValueOnce(reply)
    const { onJoined } = await open(port, true)
    if (visible) {
      expect(screen.getByRole("button", { name: "Join Example Team (example)" })).toBeVisible()
    } else {
      expect(screen.queryByRole("region", { name: "Workspace invitations" })).toBeNull()
    }
    expect(screen.queryByRole("button", { name: "Check in browser" })).toBeNull()
    await act(() => vi.advanceTimersByTimeAsync(2_000))
    expect(port.mock.calls).toEqual([
      ["account-1", { action: "list" }],
      ["account-1", { action: "dismiss", id: "attempt-1" }],
    ])
    expect(onJoined).not.toHaveBeenCalled()
  })

  it("hides an old join failure in Settings but keeps a new failure available for recovery", async () => {
    let resolveList: (reply: InvitationReply) => void = () => {}
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce(attempt("no_seat"))
      .mockImplementationOnce(() => new Promise((resolve) => (resolveList = resolve)))
      .mockResolvedValueOnce(attempt("waiting"))
      .mockResolvedValueOnce(attempt("wrong_account"))
    const { onJoined } = await open(port, true)
    expect(port.mock.calls.map(([, request]) => request)).toEqual([
      { action: "list" },
      { action: "dismiss", id: "attempt-1" },
    ])
    expect(screen.queryByRole("region", { name: "Workspace invitations" })).toBeNull()
    await act(async () => resolveList(checked))
    await click("Join Example Team (example)")
    await act(() => vi.advanceTimersByTimeAsync(2_000))
    expect(screen.getByRole("status")).toHaveTextContent(
      "Your browser used a different account.",
    )
    expect(screen.getByRole("button", { name: "Refresh invitations" })).toBeEnabled()
    expect(port.mock.calls.map(([, request]) => request.action)).toEqual([
      "list",
      "dismiss",
      "start",
      "status",
    ])
    expect(onJoined).not.toHaveBeenCalled()
  })

  it.each([false, true])(
    "retries the initial Settings list only once, with retry success: %s",
    async (success) => {
      const port = vi.fn<CloudInvitationPort>().mockRejectedValue(new Error("list failed"))
      if (success) {
        port.mockRejectedValueOnce(new Error("list failed")).mockResolvedValueOnce(checked)
      }
      await open(port, true)
      expect(screen.queryByRole("region", { name: "Workspace invitations" })).toBeNull()
      expect(port).toHaveBeenCalledOnce()
      await act(() => vi.advanceTimersByTimeAsync(8_000))
      expect(port.mock.calls).toEqual([
        ["account-1", { action: "list" }],
        ["account-1", { action: "list" }],
      ])
      if (success) {
        expect(
          screen.getByRole("button", { name: "Join Example Team (example)" }),
        ).toBeVisible()
      } else {
        expect(screen.queryByRole("region", { name: "Workspace invitations" })).toBeNull()
      }
    },
  )

  it("cancels the initial Settings list retry when the view closes", async () => {
    const port = vi.fn<CloudInvitationPort>().mockRejectedValueOnce(new Error("list failed"))
    const { unmount } = await open(port, true)
    unmount()
    await act(() => vi.advanceTimersByTimeAsync(2_000))
    expect(port).toHaveBeenCalledOnce()
  })

  it("shows only Join actions when Settings confirms pending invitations", async () => {
    let resolveList: (reply: InvitationReply) => void = () => {}
    const port = vi
      .fn<CloudInvitationPort>()
      .mockImplementationOnce(() => new Promise((resolve) => (resolveList = resolve)))
    await open(port, true)
    expect(screen.queryByRole("region", { name: "Workspace invitations" })).toBeNull()
    await act(async () => resolveList(checked))
    expect(
      screen.getByRole("button", { name: "Join Example Team (example)" }),
    ).toHaveTextContent(/^Join$/)
    expect(
      screen.getByRole("button", { name: "Join Example Team (example-other)" }),
    ).toBeVisible()
    expect(screen.getAllByRole("button")).toHaveLength(2)
    expect(screen.queryByRole("status")).toBeNull()
    expect(port).toHaveBeenCalledOnce()
  })

  it("keeps refresh available after a Settings join fails to start", async () => {
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce(checked)
      .mockRejectedValueOnce(new Error("start failed"))
      .mockRejectedValueOnce(new Error("list failed"))
      .mockResolvedValueOnce(checked)
    await open(port, true)
    await click("Join Example Team (example)")
    expect(screen.getByRole("status")).toHaveTextContent("Could not check Cloud invitations.")
    await act(() => vi.advanceTimersByTimeAsync(2_000))
    expect(port).toHaveBeenCalledTimes(2)
    await click("Refresh invitations")
    expect(screen.getByRole("button", { name: "Refresh invitations" })).toBeEnabled()
    await click("Refresh invitations")
    expect(screen.queryByRole("status")).toBeNull()
    expect(screen.getAllByRole("button")).toHaveLength(2)
    expect(port.mock.calls.map(([, request]) => request.action)).toEqual([
      "list",
      "start",
      "list",
      "list",
    ])
  })

  it("verifies a Settings join in the browser and checks an uncertain result without joining again", async () => {
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce(checked)
      .mockResolvedValueOnce(attempt("waiting"))
      .mockResolvedValueOnce(attempt("ready"))
      .mockResolvedValueOnce({ kind: "uncertain", id: "attempt-1", join: true })
      .mockResolvedValueOnce(attempt("joined"))
      .mockResolvedValueOnce({ kind: "list", listing: { state: "checked", invitations: [] } })
    const { onJoined } = await open(port, true)
    await click("Join Example Team (example-other)")
    expect(screen.getByRole("status")).toHaveTextContent("Verify your account in your browser")
    expect(screen.queryByRole("button", { name: /^Join workspace/ })).toBeNull()
    await act(() => vi.advanceTimersByTimeAsync(2_000))
    expect(port).toHaveBeenCalledTimes(3)
    expect(onJoined).not.toHaveBeenCalled()
    await click("Join workspace")
    expect(screen.queryByRole("button", { name: /^Join workspace/ })).toBeNull()
    expect(onJoined).not.toHaveBeenCalled()
    await click("Check result")
    expect(onJoined).toHaveBeenCalledOnce()
    expect(screen.queryByRole("region", { name: "Workspace invitations" })).toBeNull()
    expect(port.mock.calls.map(([, request]) => request)).toEqual([
      { action: "list" },
      {
        action: "start",
        invitation_id: "invite-2",
        workspace_name: "Example Team",
        workspace_slug: "example-other",
      },
      { action: "status", id: "attempt-1" },
      { action: "finish", id: "attempt-1" },
      { action: "status", id: "attempt-1" },
      { action: "dismiss", id: "attempt-1" },
    ])
  })

  it.each([false, true])(
    "refreshes the list after a lost Settings dismiss reply, with attempt retained: %s",
    async (retained) => {
      const port = vi
        .fn<CloudInvitationPort>()
        .mockResolvedValueOnce(attempt("joined"))
        .mockRejectedValueOnce(new Error("dismiss reply lost"))
        .mockResolvedValueOnce(retained ? attempt("joined") : checked)
        .mockResolvedValueOnce(checked)
      const { onJoined } = await open(port, true)
      expect(screen.getByRole("status")).toHaveTextContent(
        "You joined the workspace. Could not refresh invitations.",
      )
      expect(onJoined).toHaveBeenCalledOnce()
      await click("Refresh invitations")
      expect(screen.queryByRole("status")).toBeNull()
      expect(screen.getAllByRole("button")).toHaveLength(2)
      expect(onJoined).toHaveBeenCalledOnce()
      expect(port.mock.calls.map(([, request]) => request)).toEqual([
        { action: "list" },
        { action: "dismiss", id: "attempt-1" },
        { action: "list" },
        ...(retained ? [{ action: "dismiss", id: "attempt-1" }] : []),
      ])
    },
  )

  it("shows recovery when Settings reopens with an uncertain join", async () => {
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce({ kind: "uncertain", id: "attempt-1", join: true })
      .mockResolvedValueOnce({ kind: "expired", uncertain: true })
    await open(port, true)
    await click("Check result")
    expect(screen.getByRole("status")).toHaveTextContent(
      "Verification timed out before Cloud confirmed the result.",
    )
  })

  it("distinguishes teams with the same name and joins only after an explicit click", async () => {
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce(checked)
      .mockResolvedValueOnce(attempt("waiting"))
      .mockResolvedValueOnce({
        ...attempt("ready"),
        workspace_name: "Example Team",
        workspace_slug: "example-other",
      })
      .mockResolvedValueOnce(attempt("joined"))
    const { onJoined } = await open(port)
    expect(screen.getByText("Example Team (example)")).toBeVisible()
    expect(screen.getByText("Example Team (example-other)")).toBeVisible()
    expect(
      screen.getByRole("button", { name: "Verify invitation to Example Team (example)" }),
    ).toBeVisible()
    await click("Verify invitation to Example Team (example-other)")
    expect(port).toHaveBeenLastCalledWith("account-1", {
      action: "start",
      invitation_id: "invite-2",
      workspace_name: "Example Team",
      workspace_slug: "example-other",
    })
    expect(screen.queryByRole("button", { name: /^Join workspace/ })).toBeNull()
    await act(() => vi.advanceTimersByTimeAsync(2_000))
    expect(port).toHaveBeenLastCalledWith("account-1", { action: "status", id: "attempt-1" })
    expect(screen.getByRole("status")).toHaveTextContent("Join Example Team (example-other)")
    expect(onJoined).not.toHaveBeenCalled()
    await click("Join workspace Example Team (example-other)")
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

  it("reads the same attempt if the Desktop finish reply is lost", async () => {
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce(attempt("ready"))
      .mockRejectedValueOnce(new Error("reply lost"))
      .mockResolvedValueOnce(attempt("joined"))
    const { onJoined } = await open(port)
    await click("Join workspace")
    expect(port.mock.calls.map(([, request]) => request)).toEqual([
      { action: "list" },
      { action: "finish", id: "attempt-1" },
      { action: "status", id: "attempt-1" },
    ])
    expect(screen.getByRole("status")).toHaveTextContent("You joined the workspace.")
    expect(onJoined).toHaveBeenCalledOnce()
  })

  it("does not retry a finish when Cloud requires sign-in", async () => {
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce(attempt("ready"))
      .mockResolvedValueOnce({ kind: "reauth_required", uncertain: true })
    const { onJoined } = await open(port)
    await click("Join workspace")
    await act(() => vi.advanceTimersByTimeAsync(30_000))
    expect(port).toHaveBeenCalledTimes(2)
    expect(screen.getByRole("status")).toHaveTextContent("Cloud has not confirmed the result.")
    expect(screen.queryByRole("button")).toBeNull()
    expect(onJoined).not.toHaveBeenCalled()
  })

  it("shows a completed join on reopen until the result is dismissed", async () => {
    let finish: (reply: InvitationReply) => void = () => {}
    const port = vi
      .fn<CloudInvitationPort>()
      .mockResolvedValueOnce(attempt("ready"))
      .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
      .mockResolvedValueOnce(attempt("joined"))
      .mockResolvedValueOnce(checked)
    const first = await open(port)
    await click("Join workspace")
    first.unmount()
    await act(async () => finish(attempt("joined")))
    expect(first.onJoined).not.toHaveBeenCalled()

    const reopened = await open(port)
    expect(screen.getByRole("status")).toHaveTextContent("You joined the workspace.")
    expect(reopened.onJoined).toHaveBeenCalledOnce()
    await click("Refresh invitations")
    expect(port).toHaveBeenLastCalledWith("account-1", { action: "dismiss", id: "attempt-1" })
    expect(
      screen.getByRole("button", { name: "Verify invitation to Example Team (example)" }),
    ).toBeVisible()
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
    expect(port).toHaveBeenLastCalledWith("account-1", { action: "dismiss", id: "attempt-1" })
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
