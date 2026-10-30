import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { PromotionMedia } from "./PromotionMedia"

const media = {
  kind: "image" as const,
  src: "/dark.gif",
  poster: "/dark.png",
  alt: "Promo preview",
  light: { kind: "video" as const, src: "/light.mp4", poster: "/light.png" },
}
let dark = false
const changes = new EventTarget()
beforeEach(() => {
  dark = false
  delete document.documentElement.dataset.theme
  vi.spyOn(document, "hasFocus").mockReturnValue(true)
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => undefined)
  vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined)
  vi.stubGlobal("matchMedia", (query: string) => ({
    get matches() {
      return query.includes("color-scheme") && dark
    },
    addEventListener: changes.addEventListener.bind(changes),
    removeEventListener: changes.removeEventListener.bind(changes),
  }))
})
afterEach(() => {
  cleanup()
  delete document.documentElement.dataset.theme
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})
it("pairs the light video and poster on first render", () => {
  render(<PromotionMedia media={media} />)
  const preview = () => screen.getByRole("img", { name: media.alt })
  expect(preview().querySelector("video")).toHaveAttribute("src", "/light.mp4")
  expect(preview().querySelector("video")).toHaveAttribute("poster", "/light.png")
  expect(preview().querySelector("img")).toHaveAttribute("src", "/light.png")
})
it("switches explicit themes atomically and clears video failure on return", async () => {
  render(<PromotionMedia media={media} />)
  const preview = () => screen.getByRole("img", { name: media.alt })
  fireEvent.error(preview().querySelector("video")!)
  expect(preview().querySelector("video")).toBeNull()
  await act(async () => {
    document.documentElement.dataset.theme = "dark"
  })
  expect(preview().querySelector("img")).toHaveAttribute("src", "/dark.gif")
  expect(preview().querySelector("video")).toBeNull()
  await act(async () => {
    document.documentElement.dataset.theme = "light"
  })
  expect(preview().querySelector("video")).toHaveAttribute("src", "/light.mp4")
  expect(preview().querySelector("img")).toHaveAttribute("src", "/light.png")
})
it("follows system changes unless an explicit theme overrides them", async () => {
  render(<PromotionMedia media={media} />)
  const preview = () => screen.getByRole("img", { name: media.alt })
  act(() => {
    dark = true
    changes.dispatchEvent(new Event("change"))
  })
  expect(preview().querySelector("img")).toHaveAttribute("src", "/dark.gif")
  await act(async () => {
    document.documentElement.dataset.theme = "light"
  })
  expect(preview().querySelector("video")).toHaveAttribute("src", "/light.mp4")
})
it("retains base media for definitions without a light variant", () => {
  render(
    <PromotionMedia
      media={{ kind: media.kind, src: media.src, poster: media.poster, alt: media.alt }}
    />,
  )
  expect(screen.getByRole("img", { name: media.alt }).querySelector("img")).toHaveAttribute(
    "src",
    "/dark.gif",
  )
})

it("keeps video position and playback across blur, focus, and visibility changes", () => {
  render(<PromotionMedia media={media} />)
  const video = screen.getByRole("img", { name: media.alt }).querySelector("video")!
  video.currentTime = 5
  vi.mocked(video.play).mockClear()
  vi.mocked(video.pause).mockClear()
  act(() => {
    window.dispatchEvent(new Event("blur"))
    document.dispatchEvent(new Event("visibilitychange"))
    window.dispatchEvent(new Event("focus"))
  })
  expect(video.currentTime).toBe(5)
  expect(video.pause).not.toHaveBeenCalled()
  expect(video.play).not.toHaveBeenCalled()
  expect(video).toHaveAttribute("loop")
})

it("retains the GIF source when the window loses focus", () => {
  document.documentElement.dataset.theme = "dark"
  vi.spyOn(document, "hasFocus").mockReturnValue(false)
  render(<PromotionMedia media={media} />)
  const image = screen.getByRole("img", { name: media.alt }).querySelector("img")!
  expect(image).toHaveAttribute("src", "/dark.gif")
  act(() => {
    window.dispatchEvent(new Event("blur"))
    window.dispatchEvent(new Event("focus"))
  })
  expect(image).toHaveAttribute("src", "/dark.gif")
})
