import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { expect, it, vi } from "vitest"
import type { CloudOngoingConsent, CloudOngoingStatus, CloudSnapshot } from "../lib/cloud"
import { CloudAccount } from "../views/settings/CloudAccount"
import { CloudOnboarding } from "./CloudOnboarding"
import { CloudSession } from "./CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "./__tests__/cloudProjectFixtures"

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
    fixture.emit({ syncEnabled: true, savedDestinationId: consent.destinationId })
    return status
  }
  fixture.emit({ destinations: [personal, team], ...patch })
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
  const { port, ongoing, policies } = fixture()
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
