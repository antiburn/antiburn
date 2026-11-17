import type { CloudWorkspace } from "./cloud"

export interface SavedTeamDefault {
  accountId: string
  teamId: string
}

export type CloudDestinationChoice =
  | { status: "unknown" | "missing_default"; destination: null }
  | { status: "choose"; destination: null; teams: CloudWorkspace[] }
  | { status: "ready"; destination: CloudWorkspace }

export function cloudDestinationChoice(
  accountId: string,
  memberships: CloudWorkspace[] | null,
  saved: SavedTeamDefault | null,
): CloudDestinationChoice {
  if (!accountId || memberships === null) return { status: "unknown", destination: null }
  const ids = new Set(memberships.map((workspace) => workspace.id))
  if (ids.size !== memberships.length || memberships.some((workspace) => !workspace.id)) {
    return { status: "unknown", destination: null }
  }
  const teams = memberships.filter((workspace) => workspace.kind === "team")
  const personal = memberships.filter((workspace) => workspace.kind === "personal")
  if (personal.length !== 1) return { status: "unknown", destination: null }
  if (saved?.accountId === accountId) {
    const destination = teams.find((workspace) => workspace.id === saved.teamId)
    return destination
      ? { status: "ready", destination }
      : { status: "missing_default", destination: null }
  }
  if (teams.length === 0) return { status: "ready", destination: personal[0]! }
  if (teams.length === 1) return { status: "ready", destination: teams[0]! }
  return { status: "choose", destination: null, teams }
}

export function personalCloudDestination(
  granted: CloudWorkspace | null,
  requestedId: string | null = null,
): CloudWorkspace | null {
  if (
    !granted ||
    granted.kind !== "personal" ||
    !granted.id ||
    (requestedId !== null && requestedId !== granted.id)
  )
    return null
  return granted
}
