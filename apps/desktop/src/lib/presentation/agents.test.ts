import { describe, expect, it } from "vitest"

import {
  AGENT_SLUGS,
  agentDisplayName,
  agentIconName,
  agentSupportsAnalysis,
  defaultAgentSurface,
  GENERIC_AGENT_ICON,
} from "./agents"

describe("defaultAgentSurface", () => {
  it("returns 'cli' for single-variant CLI agents", () => {
    expect(defaultAgentSurface("opencode")).toBe("cli")
    expect(defaultAgentSurface("amp-code")).toBe("cli")
    expect(defaultAgentSurface("pi")).toBe("cli")
  })

  it("returns 'ide_desktop' for editor-only agents", () => {
    expect(defaultAgentSurface("kiro")).toBe("ide_desktop")
    expect(defaultAgentSurface("windsurf")).toBe("ide_desktop")
  })

  it("returns 'unknown' for bi-modal agents (cannot disambiguate from slug)", () => {
    expect(defaultAgentSurface("claude-code")).toBe("unknown")
    expect(defaultAgentSurface("codex")).toBe("unknown")
    expect(defaultAgentSurface("copilot")).toBe("unknown")
    expect(defaultAgentSurface("cline")).toBe("unknown")
    expect(defaultAgentSurface("antigravity")).toBe("unknown")
    expect(defaultAgentSurface("cursor")).toBe("unknown")
  })

  it("returns 'unknown' for unrecognised slugs", () => {
    expect(defaultAgentSurface("totally-made-up")).toBe("unknown")
    expect(defaultAgentSurface("")).toBe("unknown")
  })
})

describe("agentDisplayName / agentIconName", () => {
  it("resolves a known slug to its display name and icon slot", () => {
    expect(agentDisplayName("claude-code")).toBe("Claude Code")
    expect(agentIconName("claude-code")).toBe("claude")
  })

  it("falls back to the generic icon slot for unknown slugs", () => {
    expect(agentIconName("totally-made-up")).toBe(GENERIC_AGENT_ICON)
    expect(agentIconName("")).toBe(GENERIC_AGENT_ICON)
  })

  it("title-cases an unknown slug rather than returning nothing", () => {
    expect(agentDisplayName("totally-made-up")).toBe("Totally Made Up")
    expect(agentDisplayName("brand-new-agent")).toBe("Brand New Agent")
    expect(agentDisplayName("")).toBe("")
  })
})

describe("agentSupportsAnalysis", () => {
  it("is true only for agents with a dedicated analysis adapter", () => {
    expect(agentSupportsAnalysis("claude-code")).toBe(true)
    expect(agentSupportsAnalysis("codex")).toBe(true)
    expect(agentSupportsAnalysis("cursor")).toBe(true)
    expect(agentSupportsAnalysis("opencode")).toBe(true)
    expect(agentSupportsAnalysis("antigravity")).toBe(true)
    expect(agentSupportsAnalysis("pi")).toBe(true)
  })

  it("is false for generic-fallback agents and unknown slugs", () => {
    expect(agentSupportsAnalysis("copilot")).toBe(false)
    expect(agentSupportsAnalysis("kiro")).toBe(false)
    expect(agentSupportsAnalysis("totally-made-up")).toBe(false)
  })
})

describe("registry shape", () => {
  it("covers the ratified agent matrix and nothing else", () => {
    expect([...AGENT_SLUGS].sort()).toEqual(
      [
        "amp-code",
        "antigravity",
        "claude-code",
        "cline",
        "codex",
        "copilot",
        "cursor",
        "kiro",
        "opencode",
        "pi",
        "windsurf",
      ].sort(),
    )
  })

  it("gives every agent a distinct icon slot name", () => {
    const slots = AGENT_SLUGS.map(agentIconName)
    expect(new Set(slots).size).toBe(slots.length)
    expect(slots).not.toContain(GENERIC_AGENT_ICON)
  })
})
