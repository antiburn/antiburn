import { beforeEach, describe, expect, it, vi } from "vitest"
import type { OnboardingState } from "./ipc"
import {
  DesktopOnboardingSession,
  type DesktopOnboardingPort,
} from "./DesktopOnboardingSession"
const telemetry = vi.hoisted(() => ({ viewed: vi.fn(), action: vi.fn(), unavailable: vi.fn() }))
vi.mock("./onboardingAnalytics", () => ({
  noteOnboardingScreenViewed: telemetry.viewed,
  noteOnboardingAction: telemetry.action,
  noteOnboardingCloudUnavailable: telemetry.unavailable,
}))
const settle = async () => {
  await Promise.resolve()
  await Promise.resolve()
  await Promise.resolve()
}
function port(state: OnboardingState = "welcome"): DesktopOnboardingPort {
  return { read: vi.fn().mockResolvedValue(state), write: vi.fn(async (next) => next) }
}
beforeEach(() => vi.clearAllMocks())
describe("DesktopOnboardingSession", () => {
  it("resumes its draft without completing on close", async () => {
    const storage = port("checks"),
      session = new DesktopOnboardingSession(storage)
    const stop = session.subscribe(vi.fn())
    await settle()
    expect(session.getSnapshot().state).toBe("checks")
    stop()
    expect(storage.write).not.toHaveBeenCalled()
  })
  it("retries failed saves and completes only after durable success", async () => {
    const storage = port("cloud")
    vi.mocked(storage.write).mockRejectedValueOnce(new Error("disk"))
    const session = new DesktopOnboardingSession(storage),
      stop = session.subscribe(vi.fn())
    await settle()
    await session.change("completed")
    expect(session.getSnapshot()).toMatchObject({
      state: "cloud",
      busy: false,
      error: expect.any(String),
    })
    await session.change("completed")
    expect(session.getSnapshot()).toMatchObject({ state: "completed", error: null })
    await session.change("welcome")
    expect(storage.write).toHaveBeenCalledTimes(2)
    stop()
  })
  it("retries the native handoff after a completed draft reloads", async () => {
    const storage = port("completed")
    const session = new DesktopOnboardingSession(storage)
    const stop = session.subscribe(vi.fn())
    await settle()
    await session.change("completed")
    expect(storage.write).toHaveBeenCalledWith("completed")
    expect(telemetry.action).not.toHaveBeenCalled()
    stop()
  })
  it("serializes saves and ignores responses after unmount", async () => {
    let resolve!: (state: OnboardingState) => void
    const storage = port()
    vi.mocked(storage.write).mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done
        }),
    )
    const session = new DesktopOnboardingSession(storage),
      stop = session.subscribe(vi.fn())
    await settle()
    const pending = session.change("sessions")
    await session.change("limits")
    expect(storage.write).toHaveBeenCalledTimes(1)
    stop()
    resolve("sessions")
    await pending
    expect(session.getSnapshot().state).toBe("welcome")
  })
  it("ignores an obsolete read during StrictMode resubscription", async () => {
    let resolve!: (state: OnboardingState) => void
    const storage = port("limits")
    vi.mocked(storage.read).mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done
        }),
    )
    const session = new DesktopOnboardingSession(storage)
    session.subscribe(vi.fn())()
    const stop = session.subscribe(vi.fn())
    await settle()
    resolve("welcome")
    await settle()
    expect(session.getSnapshot().state).toBe("limits")
    stop()
  })
  it("retries a failed load without writing default settings", async () => {
    const storage = port("hud")
    vi.mocked(storage.read).mockRejectedValueOnce(new Error("unavailable"))
    const session = new DesktopOnboardingSession(storage),
      stop = session.subscribe(vi.fn())
    await settle()
    expect(session.getSnapshot()).toMatchObject({ state: null, error: expect.any(String) })
    await session.load()
    expect(session.getSnapshot()).toMatchObject({ state: "hud", error: null })
    expect(storage.write).not.toHaveBeenCalled()
    stop()
  })
  it("reports visible entries once and does not count minimizing as dismissal", async () => {
    let visibility!: (visible: boolean) => void
    const unlisten = vi.fn(),
      storage = {
        ...port("cloud"),
        getVisible: async () => false,
        onVisible: async (handler: (visible: boolean) => void) => {
          visibility = handler
          return unlisten
        },
      }
    const session = new DesktopOnboardingSession(storage),
      stop = session.subscribe(vi.fn())
    await settle()
    expect(telemetry.viewed).not.toHaveBeenCalled()
    visibility(true)
    visibility(true)
    expect(telemetry.viewed).toHaveBeenCalledTimes(1)
    await session.change("cloudUnavailable")
    expect(telemetry.unavailable).not.toHaveBeenCalled()
    visibility(false)
    expect(telemetry.action).not.toHaveBeenCalledWith("cloudUnavailable", "dismissed")
    visibility(true)
    expect(telemetry.viewed).toHaveBeenCalledTimes(3)
    stop()
    expect(unlisten).toHaveBeenCalledOnce()
  })
})
