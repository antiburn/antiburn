import { describe, expect, it } from "vitest"

import type { LiveUsageMeterPayload } from "../providerUsageIpc"
import { agentListName, agentStatus, meterForAgent } from "./agentStatus"

const claude = (fields: Partial<LiveUsageMeterPayload>): LiveUsageMeterPayload => ({
  provider: "anthropic",
  displayName: "Claude",
  shown: true,
  ...fields,
})

describe("agentStatus", () => {
  it.each<{
    name: string
    sessions: number
    meter?: LiveUsageMeterPayload
    found: boolean
    line: string
  }>([
    { name: "nothing at all", sessions: 0, found: false, line: "" },
    { name: "sessions, no meter", sessions: 12, found: true, line: "12 sessions" },
    { name: "one session", sessions: 1, found: true, line: "1 session" },
    {
      name: "sessions and a login",
      sessions: 87,
      meter: claude({ detection: "signedIn" }),
      found: true,
      line: "87 sessions · Signed in",
    },
    {
      name: "a login and no sessions",
      sessions: 0,
      meter: claude({ detection: "signedIn" }),
      found: true,
      line: "No sessions yet · Signed in",
    },
    {
      name: "Claude Desktop only, with sessions",
      sessions: 41,
      meter: claude({ detection: "notInstalled", desktopAppLabel: "Claude Desktop" }),
      found: true,
      line: "41 sessions · Claude Desktop · Limits need Claude Code signed in",
    },
    {
      name: "Claude Desktop only, chat only",
      sessions: 0,
      meter: claude({ detection: "notInstalled", desktopAppLabel: "Claude Desktop" }),
      found: true,
      line: "No sessions yet · Claude Desktop · Limits need Claude Code signed in",
    },
    {
      name: "Claude Desktop with its meter turned off",
      sessions: 41,
      meter: claude({
        shown: false,
        detection: "notInstalled",
        desktopAppLabel: "Claude Desktop",
      }),
      found: true,
      line: "41 sessions · Claude Desktop",
    },
    {
      name: "the CLI installed but not signed in",
      sessions: 3,
      meter: claude({ detection: "installedNotSignedIn" }),
      found: true,
      line: "3 sessions · Not signed in",
    },
    {
      name: "the tool not installed",
      sessions: 0,
      meter: claude({ detection: "notInstalled" }),
      found: false,
      line: "",
    },
    {
      name: "detection not run yet",
      sessions: 5,
      meter: claude({ detection: "unknown" }),
      found: true,
      line: "5 sessions",
    },
    {
      name: "a login through Pi",
      sessions: 0,
      meter: claude({ detection: "signedIn", carrierLabel: "Pi" }),
      found: true,
      line: "No sessions yet · Signed in via Pi",
    },
    {
      name: "Pi found but not signed in, beside Claude Desktop",
      sessions: 0,
      meter: claude({
        detection: "installedNotSignedIn",
        carrierLabel: "Pi",
        desktopAppLabel: "Claude Desktop",
      }),
      found: true,
      line: "No sessions yet · Pi not signed in",
    },
  ])("says $name", ({ sessions, meter, found, line }) => {
    expect(agentStatus(sessions, meter)).toEqual({ found, line })
  })
})

describe("meterForAgent", () => {
  const meters = [claude({}), { provider: "openai", displayName: "Codex", shown: true }]

  it("finds the meter of the provider that bills the agent", () => {
    expect(meterForAgent("claude-code", meters)?.provider).toBe("anthropic")
    expect(meterForAgent("codex", meters)?.provider).toBe("openai")
  })

  it("has none for an agent without one fixed provider, or before meters load", () => {
    expect(meterForAgent("cline", meters)).toBeUndefined()
    expect(meterForAgent("cursor", meters)).toBeUndefined()
    expect(meterForAgent("claude-code", null)).toBeUndefined()
  })
})

describe("agentListName", () => {
  it("calls the Claude agent Claude and keeps every other registry name", () => {
    expect(agentListName("claude-code")).toBe("Claude")
    expect(agentListName("codex")).toBe("Codex")
  })
})
