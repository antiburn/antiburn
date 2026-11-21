import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { PromotionMedia } from "./PromotionMedia"

const media = {
  kind: "image" as const,
  src: "/dark.gif",
  poster: "/dark.png",
  alt: "Promo preview",
  light: { kind: "image" as const, src: "/light.webp", poster: "/light.png" },
}
let dark = false
let reduced = false
const changes = new EventTarget()
beforeEach(() => {
  dark = false
  reduced = false
  delete document.documentElement.dataset.theme
  vi.spyOn(document, "hasFocus").mockReturnValue(true)
  vi.stubGlobal("matchMedia", (query: string) => ({
    get matches() {
      return query.includes("color-scheme") ? dark : reduced
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

const preview = () => screen.getByRole("img", { name: media.alt })

it("shows the light animation on first render", () => {
  render(<PromotionMedia media={media} />)
  expect(preview().querySelector("img")).toHaveAttribute("src", "/light.webp")
  expect(preview().querySelector("video")).toBeNull()
})

it("switches explicit themes atomically and clears a failure on return", async () => {
  render(<PromotionMedia media={media} />)
  fireEvent.error(preview().querySelector("img")!)
  expect(preview().querySelector("img")).toHaveAttribute("src", "/light.png")
  await act(async () => {
    document.documentElement.dataset.theme = "dark"
  })
  expect(preview().querySelector("img")).toHaveAttribute("src", "/dark.gif")
  await act(async () => {
    document.documentElement.dataset.theme = "light"
  })
  expect(preview().querySelector("img")).toHaveAttribute("src", "/light.webp")
})

it("follows system changes unless an explicit theme overrides them", async () => {
  render(<PromotionMedia media={media} />)
  act(() => {
    dark = true
    changes.dispatchEvent(new Event("change"))
  })
  expect(preview().querySelector("img")).toHaveAttribute("src", "/dark.gif")
  await act(async () => {
    document.documentElement.dataset.theme = "light"
  })
  expect(preview().querySelector("img")).toHaveAttribute("src", "/light.webp")
})

it("retains base media for definitions without a light variant", () => {
  render(
    <PromotionMedia
      media={{ kind: media.kind, src: media.src, poster: media.poster, alt: media.alt }}
    />,
  )
  expect(preview().querySelector("img")).toHaveAttribute("src", "/dark.gif")
})

it("shows the still poster when the reader requests less motion", () => {
  reduced = true
  render(<PromotionMedia media={media} />)
  expect(preview().querySelector("img")).toHaveAttribute("src", "/light.png")
})

it("tries the animation after a reduced-motion poster fails", () => {
  reduced = true
  render(<PromotionMedia media={media} />)
  const image = preview().querySelector("img")!
  expect(image).toHaveAttribute("src", "/light.png")
  fireEvent.error(image)
  act(() => {
    reduced = false
    changes.dispatchEvent(new Event("change"))
  })
  expect(image).toHaveAttribute("src", "/light.webp")
  fireEvent.error(image)
  expect(image).toHaveAttribute("src", "/light.png")
})

it("falls back to the poster once, and never loops on a broken poster", () => {
  render(<PromotionMedia media={media} />)
  const image = preview().querySelector("img")!
  fireEvent.error(image)
  expect(preview().querySelector("img")).toHaveAttribute("src", "/light.png")
  // The poster is the last resort, so its own failure must not re-arm onError.
  expect(preview().querySelector("img")).not.toHaveAttribute("src", "/light.webp")
})

it("retains the animated source when the window loses focus", () => {
  document.documentElement.dataset.theme = "dark"
  vi.spyOn(document, "hasFocus").mockReturnValue(false)
  render(<PromotionMedia media={media} />)
  const image = preview().querySelector("img")!
  expect(image).toHaveAttribute("src", "/dark.gif")
  act(() => {
    window.dispatchEvent(new Event("blur"))
    window.dispatchEvent(new Event("focus"))
  })
  expect(image).toHaveAttribute("src", "/dark.gif")
})
