import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { createPlayback, OnboardingView } from "./OnboardingView"

beforeEach(() => {
  vi.stubGlobal("matchMedia", () => ({
    matches: true,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }))
})
afterEach(() => vi.unstubAllGlobals())

it("navigates from welcome and offers local completion without a Cloud operation", () => {
  const onStepChange = vi.fn()
  const onComplete = vi.fn()
  const view = render(
    <OnboardingView step="welcome" onStepChange={onStepChange} onComplete={onComplete} />,
  )
  fireEvent.click(screen.getByRole("button", { name: /get started/i }))
  expect(onStepChange).toHaveBeenCalledWith("sessions")
  view.rerender(
    <OnboardingView step="cloud" onStepChange={onStepChange} onComplete={onComplete} />,
  )
  fireEvent.click(screen.getByRole("button", { name: /set up cloud/i }))
  expect(onStepChange).toHaveBeenCalledWith("cloudUnavailable")
  fireEvent.click(screen.getByRole("button", { name: /use locally/i }))
  expect(onComplete).toHaveBeenCalledOnce()
})

it("isolates the example in a sandbox and sends a reduced-motion end state", async () => {
  const view = render(
    <OnboardingView step="hud" hudSupported onStepChange={vi.fn()} onComplete={vi.fn()} />,
  )
  const frame = screen.getByTitle("Example demonstration") as HTMLIFrameElement
  expect(frame.getAttribute("sandbox")).toBe("allow-scripts")
  const send = vi.spyOn(frame.contentWindow!, "postMessage")
  fireEvent.load(frame)
  await waitFor(() =>
    expect(send).toHaveBeenCalledWith(
      expect.objectContaining({
        source: "antiburn-onboarding",
        state: expect.objectContaining({
          slide: 2,
          step: 4,
          elapsed: 4300,
          paused: false,
          visible: true,
        }),
      }),
      "*",
    ),
  )
  view.rerender(
    <OnboardingView
      step="hud"
      hudSupported
      visible={false}
      onStepChange={vi.fn()}
      onComplete={vi.fn()}
    />,
  )
  expect(send).toHaveBeenLastCalledWith(
    expect.objectContaining({
      state: expect.objectContaining({ paused: true, visible: false }),
    }),
    "*",
  )
  view.rerender(<OnboardingView step="welcome" onStepChange={vi.fn()} onComplete={vi.fn()} />)
  expect(screen.queryByTitle("Example demonstration")).toBeNull()
})

it("pauses and resumes normal playback and cancels its frame on teardown", () => {
  vi.useFakeTimers()
  vi.setSystemTime(0)
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) =>
    setTimeout(() => callback(Date.now()), 16),
  )
  vi.stubGlobal("cancelAnimationFrame", (id: number) => clearTimeout(id))
  vi.stubGlobal("matchMedia", () => ({
    matches: false,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }))
  const playback = createPlayback([100, 200], 300)
  const changed = vi.fn()
  const unsubscribe = playback.subscribe(changed)
  vi.advanceTimersByTime(160)
  expect(Math.floor(playback.getSnapshot())).toBe(1)
  playback.setVisible(false)
  const paused = playback.getElapsed()
  vi.advanceTimersByTime(500)
  expect(playback.getElapsed()).toBe(paused)
  playback.setVisible(true)
  vi.advanceTimersByTime(80)
  expect(Math.floor(playback.getSnapshot())).toBe(2)
  unsubscribe()
  const count = changed.mock.calls.length
  vi.advanceTimersByTime(1000)
  expect(changed).toHaveBeenCalledTimes(count)
  vi.useRealTimers()
})

it("updates theme without replacing the active demo frame", async () => {
  document.documentElement.dataset.theme = "light"
  const view = render(
    <OnboardingView step="sessions" onStepChange={vi.fn()} onComplete={vi.fn()} />,
  )
  const frame = screen.getByTitle("Example demonstration") as HTMLIFrameElement
  const send = vi.spyOn(frame.contentWindow!, "postMessage")
  await act(async () => {
    document.documentElement.dataset.theme = "dark"
  })
  expect(screen.getByTitle("Example demonstration")).toBe(frame)
  expect(send).toHaveBeenLastCalledWith(
    expect.objectContaining({ state: expect.objectContaining({ theme: "dark", step: 4 }) }),
    "*",
  )
  view.unmount()
  delete document.documentElement.dataset.theme
})

it("accepts only validated messages from its own demo frame", async () => {
  const view = render(
    <OnboardingView step="sessions" onStepChange={vi.fn()} onComplete={vi.fn()} />,
  )
  const frame = screen.getByTitle("Example demonstration") as HTMLIFrameElement
  await act(async () => {
    window.dispatchEvent(
      new MessageEvent("message", {
        source: window,
        data: { source: "antiburn-onboarding-demo", type: "error" },
      }),
    )
  })
  expect(screen.queryByText(/example could not load/i)).toBeNull()
  await act(async () => {
    window.dispatchEvent(
      new MessageEvent("message", {
        source: frame.contentWindow,
        data: { source: "antiburn-onboarding-demo", type: "error" },
      }),
    )
  })
  expect(screen.getByText(/example could not load/i)).toBeTruthy()
  expect(screen.getByRole("button", { name: "Continue" })).toBeTruthy()
  view.unmount()
})

it("reports an unresponsive example without blocking navigation", () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] })
  const view = render(
    <OnboardingView step="limits" onStepChange={vi.fn()} onComplete={vi.fn()} />,
  )
  act(() => vi.advanceTimersByTime(5000))
  expect(screen.getByText(/example could not load/i)).toBeTruthy()
  expect(screen.getByRole("button", { name: "Continue" })).toBeTruthy()
  view.unmount()
  expect(vi.getTimerCount()).toBe(0)
  vi.useRealTimers()
})

it("pauses the demo when the host document hides and respects native visibility on return", () => {
  const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(false)
  const view = render(
    <OnboardingView step="sessions" onStepChange={vi.fn()} onComplete={vi.fn()} />,
  )
  const frame = screen.getByTitle("Example demonstration") as HTMLIFrameElement
  const send = vi.spyOn(frame.contentWindow!, "postMessage")
  act(() => {
    hidden.mockReturnValue(true)
    document.dispatchEvent(new Event("visibilitychange"))
  })
  expect(send).toHaveBeenLastCalledWith(
    expect.objectContaining({
      state: expect.objectContaining({ paused: true, visible: false }),
    }),
    "*",
  )
  act(() => {
    hidden.mockReturnValue(false)
    document.dispatchEvent(new Event("visibilitychange"))
  })
  expect(send).toHaveBeenLastCalledWith(
    expect.objectContaining({
      state: expect.objectContaining({ paused: false, visible: true }),
    }),
    "*",
  )
  view.rerender(
    <OnboardingView
      step="sessions"
      visible={false}
      onStepChange={vi.fn()}
      onComplete={vi.fn()}
    />,
  )
  act(() => {
    hidden.mockReturnValue(true)
    document.dispatchEvent(new Event("visibilitychange"))
    hidden.mockReturnValue(false)
    document.dispatchEvent(new Event("visibilitychange"))
  })
  expect(send).toHaveBeenLastCalledWith(
    expect.objectContaining({
      state: expect.objectContaining({ paused: true, visible: false }),
    }),
    "*",
  )
  view.unmount()
  hidden.mockRestore()
})

it("does not consume animation time before the demo is ready", () => {
  vi.useFakeTimers()
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) =>
    setTimeout(() => callback(Date.now()), 16),
  )
  vi.stubGlobal("cancelAnimationFrame", (id: number) => clearTimeout(id))
  vi.stubGlobal("matchMedia", () => ({
    matches: false,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }))
  const playback = createPlayback([100, 200], 300, false)
  const unsubscribe = playback.subscribe(vi.fn())
  vi.advanceTimersByTime(2000)
  expect(playback.getElapsed()).toBe(0)
  expect(playback.getSnapshot()).toBe(0)
  playback.setVisible(true)
  vi.advanceTimersByTime(120)
  expect(Math.floor(playback.getSnapshot())).toBe(1)
  unsubscribe()
  vi.useRealTimers()
})

it("does not time out a demo while its native window is hidden", () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] })
  const props = { step: "sessions" as const, onStepChange: vi.fn(), onComplete: vi.fn() }
  const view = render(<OnboardingView {...props} visible={false} />)
  act(() => vi.advanceTimersByTime(6000))
  expect(screen.queryByText(/example could not load/i)).toBeNull()
  view.rerender(<OnboardingView {...props} visible />)
  act(() => vi.advanceTimersByTime(4999))
  expect(screen.queryByText(/example could not load/i)).toBeNull()
  act(() => vi.advanceTimersByTime(1))
  expect(screen.getByText(/example could not load/i)).toBeTruthy()
  view.unmount()
  expect(vi.getTimerCount()).toBe(0)
  vi.useRealTimers()
})
