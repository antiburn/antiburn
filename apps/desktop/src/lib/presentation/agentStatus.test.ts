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
    line: string
  }>([
    { name: "nothing at all", sessions: 0, line: "" },
    { name: "sessions, no meter", sessions: 12, line: "12 sessions" },
    { name: "one session", sessions: 1, line: "1 session" },
    {
      name: "sessions and a login",
      sessions: 87,
      meter: claude({ detection: "signedIn" }),
      line: "87 sessions · Signed in",
    },
    {
      name: "a login and no sessions",
      sessions: 0,
      meter: claude({ detection: "signedIn" }),
      line: "No sessions yet · Signed in",
    },
    {
      name: "Claude Desktop only, with sessions",
      sessions: 41,
      meter: claude({ detection: "notInstalled", desktopAppLabel: "Claude Desktop" }),
      line: "41 sessions · Claude Desktop · Limits need Claude Code signed in",
    },
    {
      name: "Claude Desktop only, chat only",
      sessions: 0,
      meter: claude({ detection: "notInstalled", desktopAppLabel: "Claude Desktop" }),
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
      line: "41 sessions · Claude Desktop",
    },
    {
      name: "the CLI installed but not signed in",
      sessions: 3,
      meter: claude({ detection: "installedNotSignedIn" }),
      line: "3 sessions · Not signed in",
    },
    {
      name: "the tool not installed",
      sessions: 0,
      meter: claude({ detection: "notInstalled" }),
      line: "",
    },
    {
      name: "detection not run yet",
      sessions: 5,
      meter: claude({ detection: "unknown" }),
      line: "5 sessions",
    },
    {
      name: "a Claude login that only Pi holds",
      sessions: 0,
      meter: claude({ detection: "signedIn", carrier: "pi", carrierLabel: "Pi" }),
      line: "",
    },
    {
      name: "Claude sessions beside a login that only Pi holds",
      sessions: 7,
      meter: claude({ detection: "signedIn", carrier: "pi", carrierLabel: "Pi" }),
      line: "7 sessions",
    },
    {
      name: "Claude Desktop beside a Claude login that only Pi holds",
      sessions: 0,
      meter: claude({
        detection: "signedIn",
        carrier: "pi",
        carrierLabel: "Pi",
        desktopAppLabel: "Claude Desktop",
      }),
      line: "No sessions yet · Claude Desktop",
    },
    {
      name: "Claude Desktop beside a Claude Code login",
      sessions: 41,
      meter: claude({ detection: "signedIn", desktopAppLabel: "Claude Desktop" }),
      line: "41 sessions · Claude Desktop · Signed in",
    },
    {
      name: "Pi found but not signed in",
      sessions: 0,
      meter: claude({ detection: "installedNotSignedIn", carrier: "pi", carrierLabel: "Pi" }),
      line: "",
    },
  ])("says $name", ({ sessions, meter, line }) => {
    // The table reads as one line: the facts column, then the note under the name.
    const status = agentStatus(sessions, meter)
    expect([status.facts, status.note].filter(Boolean).join(" · ")).toBe(line)
  })

  it("counts a desktop app or a login as found, but not a Pi login", () => {
    expect(agentStatus(0, undefined).found).toBe(false)
    expect(
      agentStatus(0, claude({ detection: "notInstalled", desktopAppLabel: "Claude Desktop" }))
        .found,
    ).toBe(true)
    expect(agentStatus(0, claude({ detection: "signedIn", carrierLabel: "Pi" })).found).toBe(
      false,
    )
  })

  it("puts the session count in the facts column and the rest under the name", () => {
    expect(
      agentStatus(41, claude({ detection: "notInstalled", desktopAppLabel: "Claude Desktop" })),
    ).toEqual({
      found: true,
      facts: "41 sessions",
      note: "Claude Desktop · Limits need Claude Code signed in",
    })
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
