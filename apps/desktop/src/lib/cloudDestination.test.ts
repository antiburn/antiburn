import { expect, it } from "vitest"
import { cloudDestinationChoice } from "./cloudDestination"
import type { CloudWorkspace } from "./cloud"

const personal: CloudWorkspace = { id: "personal", name: "Personal", kind: "personal" }
const teamA: CloudWorkspace = { id: "team-a", name: "Example A", kind: "team" }
const teamB: CloudWorkspace = { id: "team-b", name: "Example B", kind: "team" }

it("blocks unknown or incomplete memberships instead of guessing personal", () => {
  for (const memberships of [null, [], [teamA], [personal, personal]]) {
    expect(cloudDestinationChoice("account-a", memberships, null)).toEqual({
      status: "unknown",
      destination: null,
    })
  }
})

it("chooses personal for zero teams and the team for one team", () => {
  expect(cloudDestinationChoice("account-a", [personal], null)).toEqual({
    status: "ready",
    destination: personal,
  })
  expect(cloudDestinationChoice("account-a", [personal, teamA], null)).toEqual({
    status: "ready",
    destination: teamA,
  })
})

it("requires an explicit saved team default when several teams exist", () => {
  expect(cloudDestinationChoice("account-a", [personal, teamA, teamB], null)).toEqual({
    status: "choose",
    destination: null,
    teams: [teamA, teamB],
  })
  expect(
    cloudDestinationChoice("account-a", [personal, teamA, teamB], {
      accountId: "account-a",
      teamId: "team-b",
    }),
  ).toEqual({ status: "ready", destination: teamB })
})

it("does not reuse another account's team default", () => {
  expect(
    cloudDestinationChoice("account-b", [personal, teamA, teamB], {
      accountId: "account-a",
      teamId: "team-b",
    }),
  ).toMatchObject({ status: "choose", destination: null })
})

it("blocks a removed saved default even if a personal or one-team fallback exists", () => {
  for (const memberships of [[personal], [personal, teamA]]) {
    expect(
      cloudDestinationChoice("account-a", memberships, {
        accountId: "account-a",
        teamId: "removed-team",
      }),
    ).toEqual({ status: "missing_default", destination: null })
  }
})
