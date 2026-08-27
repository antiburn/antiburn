import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { TruncatedText } from "./TruncatedText"

afterEach(cleanup)

describe("TruncatedText", () => {
  it("renders its text without a redundant tooltip when it fits", () => {
    render(<TruncatedText text="Short title" />)
    const el = screen.getByText("Short title")
    expect(el.getAttribute("title")).toBeNull()
    expect(el.getAttribute("data-text")).toBeNull()
  })

  it("duplicates the text for the shimmer overlay and keeps it announceable", () => {
    render(<TruncatedText text="Running session" shimmer />)
    const el = screen.getByText("Running session")
    expect(el.getAttribute("data-text")).toBe("Running session")
    expect(el.getAttribute("aria-label")).toBe("Running session")
    expect(el.className).toContain("activity-row-title-shimmer")
  })

  it("reveals the full value once the text is actually cut off", () => {
    // jsdom reports zero dimensions, so drive the measurement directly.
    vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockReturnValue(400)
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(120)
    render(<TruncatedText text="A very long session title that will not fit" />)
    expect(
      screen.getByText("A very long session title that will not fit").getAttribute("title"),
    ).toBe("A very long session title that will not fit")
    vi.restoreAllMocks()
  })

  it("detects text that is cut off after two lines", () => {
    vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(60)
    vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(40)
    render(<TruncatedText text="A title that needs more than two lines" lines={2} />)
    const element = screen.getByText("A title that needs more than two lines")
    expect(element.className).toContain("truncated-text-lines")
    expect(element.style.getPropertyValue("--truncated-text-lines")).toBe("2")
    expect(element.getAttribute("title")).toBe("A title that needs more than two lines")
    vi.restoreAllMocks()
  })
})
