import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { expect, it, vi } from "vitest"
import type { CloudOngoingConsent, CloudOngoingStatus, CloudSnapshot } from "../lib/cloud"
import { CloudAccount } from "../views/settings/CloudAccount"
import { CloudOnboarding } from "./CloudOnboarding"
import { CloudSession } from "./CloudSession"
import { desktopInvitationPort } from "./cloudInvitationPort"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "./__tests__/cloudProjectFixtures"

vi.mock("./cloudInvitationPort", () => ({
  desktopInvitationPort: vi.fn(async () => ({
    kind: "list",
    listing: { state: "checked", invitations: [] },
  })),
}))

const personal = {
  id: "synthetic-personal",
  name: "Personal",
  kind: "personal",
  membershipGeneration: null,
} as const
const team = {
  id: "synthetic-team",
  name: "Example Team",
  kind: "team",
  membershipGeneration: "generation-1",
} as const
const studio = projectGroup()
const website = projectGroup({
  id: "website",
  path: "/Example/Website",
  label: "Website",
  folderPaths: ["/Example/Website"],
  projects: [{ ...studio.projects[0]!, projectPath: "/Example/Website" }],
})
const accountId = "synthetic-account"

function fixture(patch: Partial<CloudSnapshot> = {}) {
  const fixture = cloudProjectFixture()
  const policies = new Map<string, CloudOngoingStatus>()
  const current = (destinationId: string) =>
    policies.get(destinationId) ?? ongoingStatus({ destinationId })
  const save = (consent: CloudOngoingConsent) => {
    const status = ongoingStatus({
      destinationId: consent.destinationId,
      status: "ready",
      consentVersion: consent.version,
      policyRevision: `saved-${consent.destinationId}`,
      projects: consent.projects,
      projectChoices: consent.projectChoices,
      selectedSources: consent.selectedSources,
    })
    policies.set(consent.destinationId, status)
    fixture.emit({
      syncEnabled: true,
      savedDestinationId: consent.destinationId,
      savedDestinationIds: [...policies.keys()].sort(),
    })
    return status
  }
  fixture.emit({ destinations: [personal, team], savedDestinationIds: [], ...patch })
  fixture.ongoing.status.mockImplementation(async (binding) =>
    current(binding?.destinationId ?? personal.id),
  )
  fixture.ongoing.groups.mockResolvedValue({
    groups: [studio, website],
    sourceRoots: [{ sourceKind: "claude_jsonl", path: studio.projects[0]!.sourcePath }],
    nextCursor: null,
    partial: false,
  })
  fixture.ongoing.enable.mockImplementation(async (consent) => save(consent))
  return { ...fixture, policies, save }
}

const start = () => screen.getByRole("button", { name: "Start syncing" })
const route = (name: RegExp) => screen.getByRole("combobox", { name })

it("routes visible folder groups separately and retries only the unconfirmed destination", async () => {
  const { port, ongoing, save } = fixture()
  ongoing.enable.mockImplementationOnce(async (request) => save(request))
  ongoing.enable.mockRejectedValueOnce(new Error("synthetic failure"))
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  await screen.findByRole("combobox", { name: /Destination for Studio/ })
  expect(route(/Destination for Studio/)).toHaveDisplayValue("Don’t sync")
  expect(start()).toBeDisabled()
  fireEvent.change(route(/Destination for Studio/), { target: { value: personal.id } })
  fireEvent.change(route(/Destination for Website/), { target: { value: team.id } })
  expect(ongoing.enable).not.toHaveBeenCalled()
  fireEvent.click(start())
  expect(await screen.findByText("Personal: Sync choices saved.")).toBeVisible()
  expect(screen.getByText("Example Team: Could not confirm sync choices.")).toBeVisible()
  expect(ongoing.enable).toHaveBeenNthCalledWith(
    1,
    expect.objectContaining({
      accountId,
      destinationId: personal.id,
      membershipGeneration: null,
      projects: studio.projects,
      fullContent: true,
    }),
  )
  expect(ongoing.enable).toHaveBeenNthCalledWith(
    2,
    expect.objectContaining({
      accountId,
      destinationId: team.id,
      membershipGeneration: team.membershipGeneration,
      projects: website.projects,
    }),
  )
  fireEvent.click(screen.getByRole("button", { name: "Retry unconfirmed destinations" }))
  expect(await screen.findByRole("heading", { name: "Cloud sync choices saved" })).toBeVisible()
  expect(ongoing.enable.mock.calls.map(([request]) => request.destinationId)).toEqual([
    personal.id,
    team.id,
    team.id,
  ])
})

it("checks the saved policy after a lost reply without sending it again", async () => {
  const { port, ongoing, save } = fixture()
  ongoing.enable.mockImplementationOnce(async (consent) => {
    save(consent)
    throw new Error("synthetic lost reply")
  })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const website = await screen.findByRole("combobox", { name: /Destination for Website/ })
  fireEvent.change(website, { target: { value: team.id } })
  fireEvent.click(start())
  fireEvent.click(await screen.findByRole("button", { name: "Retry unconfirmed destinations" }))
  expect(await screen.findByRole("heading", { name: "Cloud sync choices saved" })).toBeVisible()
  expect(ongoing.enable).toHaveBeenCalledOnce()
})

it("fences the remaining request when Team membership changes during Start", async () => {
  const { port, emit, ongoing, save } = fixture()
  ongoing.enable.mockImplementationOnce(async (consent) => {
    const result = save(consent)
    emit({ destinations: [personal, { ...team, membershipGeneration: "generation-2" }] })
    return result
  })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  await screen.findByRole("combobox", { name: /Destination for Studio/ })
  fireEvent.change(route(/Destination for Studio/), { target: { value: personal.id } })
  fireEvent.change(route(/Destination for Website/), { target: { value: team.id } })
  fireEvent.click(start())
  await waitFor(() => expect(route(/Destination for Studio/)).toBeDisabled())
  await waitFor(() => expect(start()).toBeEnabled())
  expect(ongoing.enable).toHaveBeenCalledOnce()
  expect(screen.queryByRole("button", { name: "Retry unconfirmed destinations" })).toBeNull()
  fireEvent.click(start())
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledTimes(2))
  expect(ongoing.enable).toHaveBeenLastCalledWith(
    expect.objectContaining({ destinationId: team.id, membershipGeneration: "generation-2" }),
  )
})

it("preserves previously confirmed paths and locks them against implicit retargeting", async () => {
  const { port, ongoing, policies } = fixture({ savedDestinationIds: [personal.id] })
  const prior = ongoingStatus({
    destinationId: personal.id,
    consentVersion: "desktop-project-sync-v1",
    policyRevision: "existing-policy",
    status: "ready",
    projects: studio.projects,
    projectChoices: { studio: true, "not-visible": false },
    selectedSources: [{ sourceKind: "claude_jsonl", path: studio.projects[0]!.sourcePath }],
  })
  policies.set(personal.id, prior)
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const studioRoute = await screen.findByRole("combobox", { name: /Destination for Studio/ })
  expect(studioRoute).toBeDisabled()
  expect(studioRoute).toHaveDisplayValue("Personal")
  fireEvent.change(route(/Destination for Website/), { target: { value: personal.id } })
  fireEvent.click(start())
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
  expect(ongoing.enable).toHaveBeenCalledWith(
    expect.objectContaining({
      destinationId: personal.id,
      expectedPolicyRevision: "existing-policy",
      projects: [...studio.projects, ...website.projects],
      projectChoices: { studio: true, "not-visible": false, website: true },
    }),
  )
})

it("keeps a removed draft destination visible without falling back to Personal", async () => {
  const { port, emit, ongoing } = fixture()
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const website = await screen.findByRole("combobox", { name: /Destination for Website/ })
  fireEvent.change(website, { target: { value: team.id } })
  act(() => emit({ destinations: [personal] }))
  expect(await screen.findByText("Destination no longer available")).toBeVisible()
  expect(start()).toBeDisabled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("discards unsaved Settings choices on Finish and reopens with new folders off", async () => {
  const { port, ongoing } = fixture()
  const session = new CloudSession(port, false, true)
  function Account() {
    const value = useSyncExternalStore(session.subscribe, session.getSnapshot)
    return <CloudAccount session={session} value={value} />
  }
  render(<Account />)
  fireEvent.click(await screen.findByRole("button", { name: "Choose folders to sync" }))
  const website = await screen.findByRole("combobox", { name: /Destination for Website/ })
  await waitFor(() => expect(website).toBeEnabled())
  fireEvent.change(website, { target: { value: team.id } })
  expect(start()).toBeEnabled()
  fireEvent.click(screen.getByRole("button", { name: "Finish without syncing" }))
  expect(ongoing.enable).not.toHaveBeenCalled()
  fireEvent.click(screen.getByRole("button", { name: "Choose folders to sync" }))
  expect(
    await screen.findByRole("combobox", { name: /Destination for Website/ }),
  ).toHaveDisplayValue("Don’t sync")
})

it("locks folders from a saved Team that is unavailable and not the last saved destination", async () => {
  const { port, ongoing, policies } = fixture({
    destinations: [personal],
    savedDestinationId: personal.id,
    savedDestinationIds: [personal.id, team.id],
  })
  policies.set(personal.id, ongoingStatus({ consentVersion: "desktop-project-sync-v1" }))
  policies.set(
    team.id,
    ongoingStatus({
      destinationId: team.id,
      consentVersion: "desktop-project-sync-v1",
      requiresDestinationReview: true,
      status: "fenced",
      projects: studio.projects,
    }),
  )
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const studioRoute = await screen.findByRole("combobox", { name: /Destination for Studio/ })
  expect(studioRoute).toBeDisabled()
  expect(studioRoute).toHaveDisplayValue("Unavailable destination")
  expect(ongoing.status).toHaveBeenCalledWith({ accountId, destinationId: team.id })
  fireEvent.change(studioRoute, { target: { value: personal.id } })
  expect(start()).toBeDisabled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("shows both running Settings routes and pauses only the named Team", async () => {
  const { port, ongoing, policies } = fixture({ savedDestinationIds: [personal.id, team.id] })
  policies.set(
    personal.id,
    ongoingStatus({ status: "ready", consentVersion: "desktop-project-sync-v1" }),
  )
  const teamPolicy = ongoingStatus({
    destinationId: team.id,
    status: "syncing",
    consentVersion: "desktop-project-sync-v1",
  })
  policies.set(team.id, teamPolicy)
  ongoing.pause.mockResolvedValue({ ...teamPolicy, paused: true, status: "paused" })
  const session = new CloudSession(port, false, true)
  function Account() {
    const value = useSyncExternalStore(session.subscribe, session.getSnapshot)
    return <CloudAccount session={session} value={value} />
  }
  render(<Account />)
  expect(await screen.findByText("Sessions syncing")).toBeVisible()
  expect(screen.getByText("Background sync enabled")).toBeVisible()
  fireEvent.click(screen.getByRole("button", { name: "Pause uploads for Example Team" }))
  expect(
    await screen.findByRole("button", { name: "Resume uploads for Example Team" }),
  ).toBeEnabled()
  expect(screen.getByRole("button", { name: "Pause uploads for Personal" })).toBeEnabled()
  expect(ongoing.pause).toHaveBeenCalledExactlyOnceWith({ accountId, destinationId: team.id })
})

it("revokes only the Personal folder when the Team was saved last", async () => {
  const lastTeam = { ...team, id: "aaa-team" }
  const { port, ongoing, policies } = fixture({
    destinations: [lastTeam, personal],
    destination: lastTeam,
    savedDestinationId: lastTeam.id,
    savedDestinationIds: [lastTeam.id, personal.id],
  })
  const personalPolicy = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    policyRevision: "personal-revision",
    projects: studio.projects,
  })
  const teamPolicy = ongoingStatus({
    destinationId: lastTeam.id,
    status: "syncing",
    consentVersion: "desktop-project-sync-v1",
    policyRevision: "team-revision",
    projects: website.projects,
  })
  policies.set(personal.id, personalPolicy)
  policies.set(lastTeam.id, teamPolicy)
  ongoing.setProject.mockImplementation(async ({ destinationId }) => {
    const saved = { ...personalPolicy, policyRevision: "personal-removed", projects: [] }
    policies.set(destinationId, saved)
    return saved
  })
  const session = new CloudSession(port, false, true)
  function Account() {
    const value = useSyncExternalStore(session.subscribe, session.getSnapshot)
    return <CloudAccount session={session} value={value} />
  }
  render(<Account />)
  const studioSwitch = await screen.findByRole("switch", { name: /Sync Studio/ })
  expect(studioSwitch).toHaveAccessibleName(/to Personal$/)
  expect(studioSwitch).toBeChecked()
  expect(screen.getByRole("switch", { name: /Sync Website/ })).not.toBeChecked()
  expect(ongoing.groups).toHaveBeenCalledExactlyOnceWith(
    { accountId, destinationId: personal.id },
    null,
  )
  fireEvent.click(studioSwitch)
  await waitFor(() => expect(studioSwitch).not.toBeChecked())
  expect(ongoing.setProject).toHaveBeenCalledExactlyOnceWith({
    accountId,
    destinationId: personal.id,
    expectedPolicyRevision: "personal-revision",
    groupId: studio.id,
    selected: false,
    projects: [],
    folderPaths: studio.folderPaths,
  })
  expect(policies.get(lastTeam.id)).toEqual(teamPolicy)
  expect(screen.getByText("Sessions syncing")).toBeVisible()
})

it("can continue from identity when capture is supported and invitations are disabled", async () => {
  const { port, ongoing } = fixture({
    draft: { screen: "identity", folders: [] },
    invitationEnabled: false,
  })
  render(
    <CloudOnboarding
      port={port}
      analytics={false}
      holdIdentity
      onBack={vi.fn()}
      onComplete={vi.fn()}
    />,
  )
  const next = await screen.findByRole("button", { name: "Continue" })
  expect(next).toBeVisible()
  fireEvent.click(next)
  expect(await screen.findByRole("heading", { name: "Choose folders to sync" })).toBeVisible()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("moves to folder setup after Desktop verifies a first browser approval", async () => {
  const { port, ongoing, emit } = fixture({
    status: "signed_out",
    identity: null,
    destination: null,
    destinations: null,
    savedDestinationIds: null,
    captureEnabled: false,
    invitationEnabled: false,
    draft: { screen: "handoff", folders: [] },
  })
  port.begin = vi.fn(async () => {
    const waiting = { ...(await port.read()), status: "waiting" as const }
    emit(waiting)
    return waiting
  })
  const props = {
    port,
    analytics: false,
    holdIdentity: true,
    onBack: vi.fn(),
    onComplete: vi.fn(),
  }
  const { rerender } = render(<CloudOnboarding {...props} />)
  fireEvent.click(await screen.findByRole("button", { name: "Sign in with GitHub" }))
  expect(await screen.findByText(/Waiting for browser approval/)).toBeVisible()
  rerender(<CloudOnboarding {...props} visible={false} />)

  act(() => emit({ status: "returned" }))
  expect(screen.getByText(/Checking your sign-in/)).toBeVisible()
  expect(screen.queryByRole("heading", { name: "You’re signed in" })).toBeNull()
  act(() =>
    emit({
      status: "signed_in",
      identity: { id: accountId, login: "Example" },
      destination: personal,
      destinations: [personal],
      savedDestinationIds: [],
      captureEnabled: true,
      invitationEnabled: true,
    }),
  )
  rerender(<CloudOnboarding {...props} visible />)

  expect(screen.getByRole("heading", { name: "You’re signed in" })).toBeVisible()
  await waitFor(() =>
    expect(desktopInvitationPort).toHaveBeenCalledWith(accountId, { action: "list" }),
  )
  expect(screen.queryByRole("region", { name: "Workspace invitations" })).toBeNull()
  fireEvent.click(screen.getByRole("button", { name: "Continue" }))
  expect(await screen.findByRole("heading", { name: "Choose folders to sync" })).toBeVisible()
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).not.toBeChecked()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(props.onComplete).not.toHaveBeenCalled()
})
