import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { renderAgentIcon } from "./agentIcon"
import { VENDOR_ARTWORK } from "./vendorArtwork"
import {
  AGENT_SLUGS,
  agentDisplayName,
  agentIconName,
  GENERIC_AGENT_ICON,
  type AgentIconAppearance,
  type AgentSurface,
} from "./presentation/agents"

afterEach(cleanup)

function renderIcon(
  slug: string,
  size = 18,
  surface?: AgentSurface,
  appearance?: AgentIconAppearance,
) {
  render(<div data-testid="host">{renderAgentIcon(slug, size, surface, appearance)}</div>)
  const wrapper = screen.getByTestId("host").firstElementChild
  if (!(wrapper instanceof HTMLElement)) throw new Error(`no icon rendered for ${slug}`)
  return wrapper
}

describe("renderAgentIcon", () => {
  it.each([...AGENT_SLUGS])("labels %s and exposes its icon slot", (slug) => {
    const wrapper = renderIcon(slug)
    expect(wrapper).toHaveAttribute("role", "img")
    expect(wrapper).toHaveAttribute("aria-label", agentDisplayName(slug))
    expect(wrapper).toHaveAttribute("title", agentDisplayName(slug))
    expect(wrapper).toHaveAttribute("data-agent-icon", agentIconName(slug))
  })

  it.each(AGENT_SLUGS.filter((slug) => VENDOR_ARTWORK[agentIconName(slug)]))(
    "provides both themed vendor assets for %s without a letter tile",
    (slug) => {
      const wrapper = renderIcon(slug, 20)
      const artwork = VENDOR_ARTWORK[agentIconName(slug)]!
      for (const theme of ["light", "dark"] as const) {
        const img = wrapper.querySelector(`.agent-icon-${theme}`)
        expect(img).toHaveAttribute("src", artwork[theme])
        expect(img).toHaveAttribute("width", "20")
        expect(img).toHaveAttribute("height", "20")
        expect(img).toHaveAttribute("aria-hidden", "true")
        expect(img).toHaveAttribute("alt", "")
      }
      expect(wrapper.textContent).toBe("")
    },
  )

  it.each([...AGENT_SLUGS])("keeps %s neutral without changing its identity", (slug) => {
    const name = agentIconName(slug)
    const wrapper = renderIcon(slug, 40, "cli", "neutral")
    expect(wrapper).toHaveAttribute("aria-label", agentDisplayName(slug))
    expect(wrapper).toHaveClass("text-label-tertiary")
    expect(wrapper.querySelector("img")).toBeNull()
    const artwork = VENDOR_ARTWORK[name]!
    const mask = wrapper.firstElementChild as HTMLElement
    expect(mask).toHaveClass("bg-current")
    expect(mask.style.width).toBe("40px")
    expect(mask.style.maskImage).toContain(artwork.mask)
    expect(mask.style.maskMode).toBe(artwork.maskMode)
    expect(wrapper.querySelector("svg")).toBeNull()
    expect(wrapper.textContent).toBe("")
  })

  it.each(["cli", "ide_desktop", "unknown"] as const)(
    "draws a surface glyph for an unknown slug on %s",
    (surface) => {
      const wrapper = renderIcon("not-a-real-agent", 18, surface)
      expect(wrapper).toHaveAttribute("data-agent-icon", GENERIC_AGENT_ICON)
      expect(wrapper.querySelector("svg")).not.toBeNull()
    },
  )

  it("gives Oh My Pi its own mark rather than Pi's", () => {
    expect(VENDOR_ARTWORK.omp!.dark).not.toBe(VENDOR_ARTWORK.pi!.dark)
    expect(renderIcon("omp").querySelector("img")).toHaveAttribute(
      "src",
      VENDOR_ARTWORK.omp!.light,
    )
  })

  it("does not use Google's AMP framework artwork for the Amp agent", async () => {
    const source = decodeURIComponent(VENDOR_ARTWORK.amp!.mask.split(",").slice(1).join(","))
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(source))
    const checksum = Array.from(new Uint8Array(digest), (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("")
    expect(checksum).toBe("569a48840bb0d4166ea95784b718c8d2496d4ebf8de1e1a93e544aef08b86b4e")
  })
})

describe("brand registries", () => {
  it("contains no unused icon slot", () => {
    const slots = new Set(AGENT_SLUGS.map(agentIconName))
    for (const name of Object.keys(VENDOR_ARTWORK)) {
      expect(slots.has(name), name).toBe(true)
    }
  })
})
