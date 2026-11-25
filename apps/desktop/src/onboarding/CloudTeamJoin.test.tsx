import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { expect, test, vi } from "vitest"
import type { CloudTeamJoinAction, CloudTeamJoinResult } from "../lib/cloud"
import { CloudTeamJoin } from "./CloudTeamJoin"

const inviteLink = `https://dashboard.example/join#token=${"a".repeat(43)}`
const attemptId = "11111111-2222-4333-8444-555555555555"
const emailId = "22222222-2222-4333-8444-555555555555"
const teamName = "Example Studio"
const email = "member@example.com"

test("checks the invite, verifies email, and waits for an explicit join", async () => {
  const send = vi.fn<(action: CloudTeamJoinAction) => Promise<CloudTeamJoinResult>>()
  send.mockResolvedValueOnce({ status: "started", attemptId })
  send.mockResolvedValueOnce({ status: "needsEmail", attemptId, teamName, email })
  send.mockResolvedValueOnce({ status: "verificationPending", attemptId, teamName, email })
  send.mockResolvedValueOnce({ status: "ready", attemptId, teamName, email, emailId })
  send.mockResolvedValueOnce({ status: "joined", teamName, inSetup: true })

  render(<CloudTeamJoin send={send} />)
  const open = screen.getByRole("button", { name: "Join a Team" })
  expect(screen.queryByRole("textbox")).not.toBeInTheDocument()
  fireEvent.click(open)
  const invite = screen.getByRole("textbox", { name: "Team invite link" })
  expect(invite).toHaveFocus()
  fireEvent.change(invite, { target: { value: inviteLink } })
  fireEvent.click(screen.getByRole("button", { name: "Check invite" }))

  const workEmail = await screen.findByRole("textbox", { name: "Work email" })
  expect(workEmail).toHaveValue(email)
  fireEvent.click(screen.getByRole("button", { name: "Use this email" }))

  const verification = await screen.findByRole("button", { name: "Check verification" })
  expect(screen.getByText(/verify it with the same Cloud account/)).toBeVisible()
  fireEvent.click(verification)

  const join = await screen.findByRole("button", { name: "Join Team" })
  expect(join).toHaveFocus()
  expect(send.mock.calls).toEqual([
    [{ action: "start", link: inviteLink }],
    [{ action: "resume", attemptId }],
    [{ action: "selectEmail", attemptId, address: email }],
    [{ action: "resume", attemptId }],
  ])
  fireEvent.click(join)

  expect(await screen.findByRole("status")).toHaveTextContent(
    "You joined Example Studio. The owner still needs to open the Team.",
  )
  expect(send).toHaveBeenLastCalledWith({ action: "claim", attemptId, emailId })
  fireEvent.click(screen.getByRole("button", { name: "Done" }))
  expect(open).toHaveFocus()
  expect(screen.queryByRole("status")).not.toBeInTheDocument()
})

test("checks the same attempt when its first resume is not confirmed", async () => {
  const send = vi.fn<(action: CloudTeamJoinAction) => Promise<CloudTeamJoinResult>>()
  send.mockResolvedValueOnce({ status: "started", attemptId })
  send.mockRejectedValueOnce("team_join_unconfirmed")
  send.mockResolvedValueOnce({ status: "ready", attemptId, teamName, email, emailId })
  render(<CloudTeamJoin send={send} />)
  fireEvent.click(screen.getByRole("button", { name: "Join a Team" }))
  fireEvent.change(screen.getByRole("textbox", { name: "Team invite link" }), {
    target: { value: inviteLink },
  })
  fireEvent.click(screen.getByRole("button", { name: "Check invite" }))
  expect(await screen.findByRole("alert")).toHaveTextContent("Could not confirm")
  expect(screen.queryByRole("textbox")).toBeNull()
  fireEvent.click(screen.getByRole("button", { name: "Check invite" }))
  expect(await screen.findByRole("button", { name: "Join Team" })).toBeEnabled()
  expect(send.mock.calls.slice(1)).toEqual([
    [{ action: "resume", attemptId }],
    [{ action: "resume", attemptId }],
  ])
})

test("hides the Team name before resume and offers no retry to a removed member", async () => {
  let resolveResume: ((result: CloudTeamJoinResult) => void) | undefined
  const resume = new Promise<CloudTeamJoinResult>((resolve) => {
    resolveResume = resolve
  })
  const send = vi.fn<(action: CloudTeamJoinAction) => Promise<CloudTeamJoinResult>>()
  const startWithUnexpectedName = { status: "started", attemptId, teamName } as const
  send.mockResolvedValueOnce(startWithUnexpectedName)
  send.mockReturnValueOnce(resume)
  render(<CloudTeamJoin send={send} />)
  fireEvent.click(screen.getByRole("button", { name: "Join a Team" }))
  fireEvent.change(screen.getByRole("textbox", { name: "Team invite link" }), {
    target: { value: inviteLink },
  })
  fireEvent.click(screen.getByRole("button", { name: "Check invite" }))
  await waitFor(() => expect(send).toHaveBeenLastCalledWith({ action: "resume", attemptId }))
  expect(screen.getByRole("button", { name: "Working…" })).toBeInTheDocument()
  expect(screen.queryByText(/Example Studio/)).toBeNull()
  await act(async () => resolveResume?.({ status: "removed" }))
  expect(await screen.findByRole("status")).toHaveTextContent(
    "An admin removed you from this workspace.",
  )
  expect(screen.queryByRole("button", { name: "Check invite" })).toBeNull()
  expect(screen.queryByRole("button", { name: "Join Team" })).toBeNull()
  expect(screen.queryByText(/Example Studio/)).toBeNull()
})

test("requires another explicit claim when the ready email changes", async () => {
  const newEmail = "other@example.com"
  const newEmailId = "33333333-2222-4333-8444-555555555555"
  const send = vi.fn<(action: CloudTeamJoinAction) => Promise<CloudTeamJoinResult>>()
  send.mockResolvedValueOnce({ status: "started", attemptId })
  send.mockResolvedValueOnce({ status: "ready", attemptId, teamName, email, emailId })
  send.mockResolvedValueOnce({
    status: "ready",
    attemptId,
    teamName,
    email: newEmail,
    emailId: newEmailId,
  })
  send.mockResolvedValueOnce({ status: "joined", teamName, inSetup: false })
  render(<CloudTeamJoin send={send} />)
  fireEvent.click(screen.getByRole("button", { name: "Join a Team" }))
  fireEvent.change(screen.getByRole("textbox", { name: "Team invite link" }), {
    target: { value: inviteLink },
  })
  fireEvent.click(screen.getByRole("button", { name: "Check invite" }))
  fireEvent.click(await screen.findByRole("button", { name: "Join Team" }))

  const summary = await screen.findByText(/The selected email changed/)
  expect(summary).toHaveTextContent(newEmail)
  expect(summary).toHaveFocus()
  const join = screen.getByRole("button", { name: "Join Team" })
  expect(fireEvent.keyDown(summary, { key: "Enter", repeat: true })).toBe(false)
  expect(fireEvent.keyDown(join, { key: "Enter", repeat: true })).toBe(false)
  const form = join.closest("form")
  if (!form) throw new Error("Expected the Team join form.")
  fireEvent.submit(form)
  expect(send).toHaveBeenLastCalledWith({ action: "claim", attemptId, emailId })
  expect(send).toHaveBeenCalledTimes(3)

  fireEvent.click(join)
  expect(await screen.findByRole("status")).toHaveTextContent("You joined Example Studio.")
  expect(send).toHaveBeenLastCalledWith({ action: "claim", attemptId, emailId: newEmailId })
})

test("requests verification again and lets the user change the pending address", async () => {
  const otherEmail = "other@example.com"
  const pending = { status: "verificationPending", attemptId, teamName, email } as const
  const send = vi.fn<(action: CloudTeamJoinAction) => Promise<CloudTeamJoinResult>>()
  send.mockResolvedValueOnce({ status: "started", attemptId })
  send.mockResolvedValueOnce(pending)
  send.mockResolvedValueOnce(pending)
  send.mockResolvedValueOnce({ ...pending, email: otherEmail })
  render(<CloudTeamJoin send={send} />)
  fireEvent.click(screen.getByRole("button", { name: "Join a Team" }))
  fireEvent.change(screen.getByRole("textbox", { name: "Team invite link" }), {
    target: { value: inviteLink },
  })
  fireEvent.click(screen.getByRole("button", { name: "Check invite" }))

  fireEvent.click(await screen.findByRole("button", { name: "Request another email" }))
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Change email" })).toBeEnabled(),
  )
  expect(send).toHaveBeenLastCalledWith({ action: "selectEmail", attemptId, address: email })
  fireEvent.click(screen.getByRole("button", { name: "Change email" }))
  const field = screen.getByRole("textbox", { name: "Work email" })
  expect(field).toHaveFocus()
  expect(field).toHaveValue(email)
  fireEvent.change(field, { target: { value: otherEmail } })
  fireEvent.click(screen.getByRole("button", { name: "Use this email" }))

  expect(await screen.findByText(/Open the link sent to other@example.com/)).toBeVisible()
  expect(send).toHaveBeenLastCalledWith({
    action: "selectEmail",
    attemptId,
    address: otherEmail,
  })
  expect(send.mock.calls.some(([action]) => action.action === "claim")).toBe(false)
})

test("retries a full Team with the same verified attempt", async () => {
  const send = vi.fn<(action: CloudTeamJoinAction) => Promise<CloudTeamJoinResult>>()
  send.mockResolvedValueOnce({ status: "started", attemptId })
  send.mockResolvedValueOnce({ status: "ready", attemptId, teamName, email, emailId })
  send.mockResolvedValueOnce({ status: "full" })
  send.mockResolvedValueOnce({ status: "joined", teamName, inSetup: false })
  render(<CloudTeamJoin send={send} />)
  fireEvent.click(screen.getByRole("button", { name: "Join a Team" }))
  fireEvent.change(screen.getByRole("textbox", { name: "Team invite link" }), {
    target: { value: inviteLink },
  })
  fireEvent.click(screen.getByRole("button", { name: "Check invite" }))
  fireEvent.click(await screen.findByRole("button", { name: "Join Team" }))
  expect(await screen.findByRole("alert")).toHaveTextContent("no seats available")
  fireEvent.click(screen.getByRole("button", { name: "Join Team" }))
  expect(await screen.findByRole("status")).toHaveTextContent("You joined Example Studio.")
  expect(send.mock.calls.slice(2)).toEqual([
    [{ action: "claim", attemptId, emailId }],
    [{ action: "claim", attemptId, emailId }],
  ])
})
