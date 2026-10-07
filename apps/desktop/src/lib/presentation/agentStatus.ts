import type { LiveUsageMeterPayload } from "../providerUsageIpc"
import { agentDisplayName, agentProvider } from "./agents"
import { liveToolName } from "./liveUsage"

/**
 * What an agent list says about one agent: what this computer has for it,
 * in one plain line.
 *
 * The Agents step settings show it. It combines two answers
 * that come from different places: the scan's session count, and the usage
 * meter's login detection. Only an agent billed by one fixed provider has a
 * meter, so the login part applies to Claude, Codex and Antigravity.
 *
 * A login that Pi holds is Pi's, not the agent's. Pi uses the provider's
 * models, but it is a separate coding agent. So the line ignores a login
 * that comes through Pi, and that login never makes an agent found. The
 * provider's desktop app still belongs to the agent, so the line names it.
 */
export interface AgentStatus {
  /** Sessions, a login, or the provider's desktop app turned up. */
  found: boolean
  /** The session count for the facts column, or "" when there is nothing to say. */
  facts: string
  /** The desktop app and login, for the line under the name, or "". */
  note: string
}

/**
 * The agent's name in an agent list. Claude Desktop's Code tab and Cowork
 * sessions are Claude sessions too, so the list says "Claude". Everywhere
 * else keeps the registry name.
 */
export function agentListName(slug: string): string {
  return slug === "claude-code" ? "Claude" : agentDisplayName(slug)
}

/** The meter for the provider that bills `slug`, if any. */
export function meterForAgent(
  slug: string,
  meters: readonly LiveUsageMeterPayload[] | null,
): LiveUsageMeterPayload | undefined {
  const provider = agentProvider(slug)
  if (!provider) return undefined
  return meters?.find((meter) => meter.provider === provider)
}

export function agentStatus(
  sessionsSeen: number,
  meter: LiveUsageMeterPayload | undefined,
): AgentStatus {
  const ownMeter = meter?.carrierLabel === "Pi" ? undefined : meter
  const notes: string[] = []
  const desktopApp = meter?.desktopAppLabel
  if (desktopApp) notes.push(desktopApp)
  const login = ownMeter ? loginPart(ownMeter, Boolean(desktopApp)) : null
  if (login) notes.push(login)

  const signedInOrInstalled =
    ownMeter?.detection === "signedIn" || ownMeter?.detection === "installedNotSignedIn"
  const found = sessionsSeen > 0 || signedInOrInstalled || Boolean(desktopApp)
  const facts =
    sessionsSeen > 0
      ? `${sessionsSeen} ${sessionsSeen === 1 ? "session" : "sessions"}`
      : found
        ? "No sessions yet"
        : ""
  return { found, facts, note: notes.join(" · ") }
}

/** The login half of the line, or null when detection has nothing definite. */
function loginPart(meter: LiveUsageMeterPayload, desktopApp: boolean): string | null {
  switch (meter.detection) {
    case "signedIn":
      return "Signed in"
    case "installedNotSignedIn":
      return desktopApp && meter.shown
        ? `Limits need ${liveToolName(meter)} signed in`
        : "Not signed in"
    case "notInstalled":
      // The desktop app keeps its own sign-in, which antiburn does not read.
      // Limits need the tool's login, so say which one.
      return desktopApp && meter.shown ? `Limits need ${liveToolName(meter)} signed in` : null
    default:
      return null
  }
}
