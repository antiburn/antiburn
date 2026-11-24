import { expect, it, vi } from "vitest"
import { CloudSession } from "./CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "./__tests__/cloudProjectFixtures"

it("stops the scan batch and preserves the error when Pause fails", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const personal = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
  })
  const team = { ...personal, destinationId: "synthetic-team", policyRevision: "team-policy" }
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", name: "Personal", kind: "personal" as const },
    { id: "synthetic-team", name: "Team", kind: "team" as const },
  ])
  ongoing.status.mockResolvedValue(personal)
  let rejectScan!: (error: Error) => void
  ongoing.backfill.mockImplementationOnce(
    () => new Promise((_, reject) => (rejectScan = reject)),
  )
  ongoing.setEnabled.mockRejectedValueOnce(new Error("Synthetic Pause failure"))
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.workspaces).toHaveLength(2))
    emitSync(team)
    const scan = session.syncNow(session.getSnapshot())
    expect(session.getSnapshot().canPauseBackfill).toBe(true)

    expect(await session.controlSync(true)).toBe(false)
    rejectScan(new Error("Synthetic scan failure"))
    expect(await scan).toBe(false)
    expect(ongoing.backfill).toHaveBeenCalledTimes(1)
    expect(session.getSnapshot().uploadActionError).toBe(
      "Could not change Cloud uploads. Try again.",
    )
  } finally {
    stop()
  }
})
