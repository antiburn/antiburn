import { beforeEach, describe, expect, it } from "vitest"

import {
  claudeCodePresent,
  claudePluginOffer,
  readClaudePluginOfferDismissed,
  writeClaudePluginOfferDismissed,
  type ClaudePluginStatusPayload,
} from "./claudePluginOffer"
import type { LiveUsageMeterPayload } from "./providerUsageIpc"

function meter(overrides: Partial<LiveUsageMeterPayload>): LiveUsageMeterPayload {
  return { provider: "anthropic", displayName: "Claude", shown: true, ...overrides }
}

const ABSENT: ClaudePluginStatusPayload = { state: "absent", version: null, enabled: true }
const INSTALLED: ClaudePluginStatusPayload = {
  state: "installed",
  version: "0.2.1",
  enabled: true,
}
const UNKNOWN: ClaudePluginStatusPayload = { state: "unknown", version: null, enabled: true }

describe("claudeCodePresent", () => {
  it("counts a signed-in or installed Claude Code CLI", () => {
    expect(claudeCodePresent([meter({ detection: "signedIn" })], 0)).toBe(true)
    expect(claudeCodePresent([meter({ detection: "installedNotSignedIn" })], 0)).toBe(true)
  })

  it("counts Claude Desktop alone, because the plugin works in its Code tab", () => {
    expect(
      claudeCodePresent(
        [meter({ detection: "notInstalled", desktopAppLabel: "Claude Desktop" })],
        0,
      ),
    ).toBe(true)
  })

  it("counts local Claude Code sessions even with no meter", () => {
    expect(claudeCodePresent([], 3)).toBe(true)
  })

  it("ignores other providers and an absent Claude", () => {
    expect(claudeCodePresent([meter({ provider: "openai", detection: "signedIn" })], 0)).toBe(
      false,
    )
    expect(claudeCodePresent([meter({ detection: "notInstalled" })], 0)).toBe(false)
    expect(claudeCodePresent([meter({})], 0)).toBe(false)
  })
})

describe("claudePluginOffer", () => {
  const here = [meter({ detection: "signedIn" })]

  it("offers when Claude Code is here, the plugin is absent, and nothing was dismissed", () => {
    expect(
      claudePluginOffer({ meters: here, claudeSessions: 0, status: ABSENT, dismissed: false }),
    ).toBe("offer")
  })

  it("shows installed over everything, dismissed or not", () => {
    expect(
      claudePluginOffer({ meters: [], claudeSessions: 0, status: INSTALLED, dismissed: true }),
    ).toBe("installed")
  })

  it("hides after a dismissal", () => {
    expect(
      claudePluginOffer({ meters: here, claudeSessions: 0, status: ABSENT, dismissed: true }),
    ).toBe("hidden")
  })

  it("hides without Claude Code", () => {
    expect(
      claudePluginOffer({ meters: [], claudeSessions: 0, status: ABSENT, dismissed: false }),
    ).toBe("hidden")
  })

  it("hides when the status is unknown or not yet read", () => {
    expect(
      claudePluginOffer({ meters: here, claudeSessions: 0, status: UNKNOWN, dismissed: false }),
    ).toBe("hidden")
    expect(
      claudePluginOffer({ meters: here, claudeSessions: 0, status: null, dismissed: false }),
    ).toBe("hidden")
  })
})

describe("dismissal storage", () => {
  beforeEach(() => localStorage.clear())

  it("starts undismissed and remembers a dismissal", () => {
    expect(readClaudePluginOfferDismissed()).toBe(false)
    writeClaudePluginOfferDismissed(new Date("2026-10-21T00:00:00Z"))
    expect(readClaudePluginOfferDismissed()).toBe(true)
    expect(JSON.parse(localStorage.getItem("antiburn.claudePluginOffer.v1")!)).toEqual({
      dismissedAt: "2026-10-21T00:00:00.000Z",
    })
  })

  it("treats a malformed value as undismissed", () => {
    localStorage.setItem("antiburn.claudePluginOffer.v1", "{nope")
    expect(readClaudePluginOfferDismissed()).toBe(false)
    localStorage.setItem("antiburn.claudePluginOffer.v1", JSON.stringify({ dismissedAt: 1 }))
    expect(readClaudePluginOfferDismissed()).toBe(false)
  })
})
