import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import { defaultConnection } from "../../../../lib/smartCheckProviders"
import type {
  CheckAvailability,
  CheckAvailabilityEvent,
} from "../../../../lib/checkAvailability"
import type { CheckDirectory } from "../../../../lib/checkDirectories"
import { SettingsTargetFocus } from "../../../settings/SettingsTargetFocus"
import { deferred } from "../../burn-checks/tests/burnChecksTestSupport"
import type { ChecksStepSettings as ChecksStepSettingsComponent } from "./ChecksStepSettings"

const getAvailability = vi.hoisted(() => vi.fn())
const getDirectories = vi.hoisted(() => vi.fn())
const openDirectory = vi.hoisted(() => vi.fn())
const providerInvoke = vi.hoisted(() => vi.fn())
const setChecksEnabled = vi.hoisted(() => vi.fn())
const setCheckEnabled = vi.hoisted(() => vi.fn())
const subscribeAvailability = vi.hoisted(() => vi.fn())
const platform = vi.hoisted(() => vi.fn())
const availabilityEvent = vi.hoisted(() => ({
  callback: null as ((event: CheckAvailabilityEvent) => void) | null,
}))
const emptyAvailability = vi.hoisted(() => ({
  revision: 1,
  yamlEnabled: false,
  installedChecks: [],
  checks: (
    [
      "sessionsOverDepth",
      "modelOverthinking",
      "overpoweredSubagents",
      "unusedMcpServers",
      "unusedBuiltInTools",
      "unusedSkills",
      "oldModelUsage",
      "overuseOfFastMode",
      "cacheChurn",
    ] as const
  ).map((id) => ({ id, enabled: true })),
  configured: false,
  savedKey: false,
  error: null,
  usage: {
    inputTokens: 0,
    outputTokens: 0,
    confirmedCalls: 0,
    cacheHits: 0,
    unknownOutcomes: 0,
    estimatedUsd: "$0.00",
    lastUsedAtEpoch: null,
  },
}))
vi.mock("../../../../lib/checkAvailability", () => ({
  getCheckAvailability: getAvailability,
  removeTypeSafeApiKey: vi.fn(),
  setSmartBurnChecksEnabled: setChecksEnabled,
  setCheckEnabled,
  onCheckAvailabilityChanged: subscribeAvailability,
  emptyCheckAvailability: emptyAvailability,
}))
vi.mock("../../../../lib/checkDirectories", () => ({
  getCheckDirectories: getDirectories,
  openCheckDirectory: openDirectory,
}))
vi.mock("@tauri-apps/api/core", () => ({ invoke: providerInvoke }))
vi.mock("../../../../lib/platform", () => ({ detectPlatform: platform }))
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn().mockResolvedValue(true) }))
let ChecksStepSettings: typeof ChecksStepSettingsComponent
const directories: CheckDirectory[] = [
  {
    reference: "global-ref",
    scope: "global",
    label: "This device",
    path: "/Users/fixture/.antiburn/checks",
    state: "ready",
    fileCount: 2,
    message: null,
  },
  {
    reference: "project-ref",
    scope: "project",
    label: "Project",
    path: "/Users/fixture/long project path/.antiburn/checks",
    state: "ready",
    fileCount: 0,
    message: null,
  },
  {
    reference: null,
    scope: "project",
    label: "Missing project",
    path: "/Users/fixture/missing/.antiburn/checks",
    state: "missing",
    fileCount: null,
    message: null,
  },
  {
    reference: null,
    scope: "project",
    label: "Paused project",
    path: "/Users/fixture/paused/.antiburn/checks",
    state: "paused",
    fileCount: null,
    message: null,
  },
]

beforeEach(async () => {
  vi.resetModules()
  ;({ ChecksStepSettings } = await import("./ChecksStepSettings"))
  HTMLElement.prototype.scrollIntoView = vi.fn()
  HTMLDialogElement.prototype.showModal = function () {
    this.open = true
  }
  HTMLDialogElement.prototype.close = function () {
    this.open = false
  }
  availabilityEvent.callback = null
  platform.mockReset().mockReturnValue("macos")
  subscribeAvailability.mockReset().mockImplementation(async (callback) => {
    availabilityEvent.callback = callback
    return () => undefined
  })
  getAvailability.mockReset().mockResolvedValue(emptyAvailability)
  getDirectories.mockReset().mockResolvedValue(directories)
  openDirectory.mockReset().mockResolvedValue(undefined)
  setChecksEnabled.mockReset().mockImplementation(async (enabled: boolean) => ({
    ...emptyAvailability,
    yamlEnabled: enabled,
  }))
  setCheckEnabled.mockReset().mockImplementation(async (id: string, enabled: boolean) => ({
    ...emptyAvailability,
    checks: emptyAvailability.checks.map((check) =>
      check.id === id ? { ...check, enabled } : check,
    ),
  }))
  providerInvoke.mockReset().mockImplementation(async (command, args) => {
    if (command === "get_system_one_settings")
      return {
        activeId: "jev",
        profiles: { jev: defaultConnection("jev") },
        pendingOperation: null,
        displayNumbers: { jev: 1 },
      }
    if (command === "save_system_one_connection")
      return {
        pendingOperation: null,
        displayNumbers: { jev: 1 },
        activeId: "jev",
        profiles: {
          jev: { ...args.draft.connection, credential: { kind: "connection", id: "jev" } },
        },
      }
    throw new Error(`Unexpected command: ${command}`)
  })
})

it("keeps one configuration card and useful file access while checks are off", async () => {
  render(<ChecksStepSettings />)
  const modelSection = screen
    .getByRole("heading", { name: "Model-based checks" })
    .closest("section")!
  expect(within(modelSection).getAllByRole("heading")).toHaveLength(2)
  expect(screen.getAllByRole("switch")).toHaveLength(10)
  expect(screen.getByRole("switch", { name: "Run checks" })).not.toBeChecked()
  expect(
    screen.getByText(
      "Each check file defines which session data the check sends to your provider.",
    ),
  ).toBeVisible()
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "More actions for Jev" })).toBeEnabled(),
  )
  expect(screen.queryByLabelText("Provider")).not.toBeInTheDocument()
  expect(screen.queryByLabelText("Jev API key")).not.toBeInTheDocument()
  expect(modelSection.querySelectorAll(".rounded-popover")).toHaveLength(1)
  const card = screen.getByRole("switch", { name: "Run checks" }).closest(".rounded-popover")!
  expect(
    within(card as HTMLElement).getByRole("button", { name: "Check files" }),
  ).toHaveAttribute("aria-expanded", "false")
  expect(
    within(card as HTMLElement).getByRole("button", { name: "Model usage" }),
  ).toHaveAttribute("aria-expanded", "false")
  fireEvent.click(screen.getByRole("button", { name: "Check files" }))
  await screen.findByText("2 check files")
  expect(getDirectories).toHaveBeenCalledOnce()
  expect(screen.queryByText(/Checks discovered/)).not.toBeInTheDocument()
  expect(setChecksEnabled).not.toHaveBeenCalled()
})

it("opens only the issued folder reference, suppresses concurrent presses, and keeps inline failures", async () => {
  const first = deferred<void>()
  openDirectory.mockReturnValueOnce(first.promise)
  getDirectories.mockResolvedValueOnce([
    directories[0],
    { ...directories[1]!, fileCount: 1 },
    ...directories.slice(2),
  ])
  render(<ChecksStepSettings />)
  fireEvent.click(screen.getByRole("button", { name: "Check files" }))
  const open = await screen.findByRole("button", { name: /^Open in .*This device/ })
  fireEvent.click(open)
  fireEvent.click(open)
  expect(openDirectory).toHaveBeenCalledExactlyOnceWith("global-ref")
  expect(open).toBeDisabled()
  const project = screen.getByRole("button", { name: /^Open in .*Project \(/ })
  expect(project).toBeEnabled()
  fireEvent.click(project)
  await waitFor(() => expect(project).toBeEnabled())
  expect(openDirectory).toHaveBeenCalledWith("project-ref")
  await act(async () => first.reject(new Error("private path error")))
  expect(screen.getByRole("alert")).toHaveTextContent("Could not open a check folder")
  expect(screen.getByText(/^Could not open this check folder\./)).toBeVisible()
  expect(screen.queryByText(/private path/)).not.toBeInTheDocument()
  expect(screen.getByText("1 check file")).toBeVisible()
  expect(screen.queryByText("Folder not created")).not.toBeInTheDocument()
  expect(screen.queryByText(/unknown/i)).not.toBeInTheDocument()
  expect(screen.getAllByRole("button", { name: /^Open in / })).toHaveLength(2)
})

it("keeps initial folder loading visible and omits unavailable folder rows", async () => {
  const read = deferred<CheckDirectory[]>()
  getDirectories.mockReturnValueOnce(read.promise)
  render(<ChecksStepSettings />)
  fireEvent.click(screen.getByRole("button", { name: "Check files" }))
  expect(screen.getByText("Loading check folders…")).toBeVisible()
  expect(screen.queryByText(/0 check files/)).not.toBeInTheDocument()
  await act(async () =>
    read.resolve([
      {
        ...directories[0]!,
        reference: null,
        state: "unavailable",
        fileCount: null,
        message: "Folder access is unavailable.",
      },
    ]),
  )
  expect(screen.getByText("No check files found.")).toBeVisible()
  expect(screen.queryByText(/unavailable|unknown/i)).not.toBeInTheDocument()
  expect(screen.queryByText(directories[0]!.path)).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: /^Open in / })).not.toBeInTheDocument()
})

it("loads fresh folders on each visit and opens only the new issued reference", async () => {
  openDirectory.mockRejectedValueOnce(new Error("old reference is unavailable"))
  const firstVisit = render(<ChecksStepSettings />)
  fireEvent.click(screen.getByRole("button", { name: "Check files" }))
  const open = await screen.findByRole("button", { name: /^Open in .*This device/ })
  expect(screen.queryByRole("button", { name: /refresh folders/i })).not.toBeInTheDocument()
  fireEvent.click(open)
  await screen.findByText(
    "Could not open this check folder. Reopen Settings → Checks to try again.",
  )
  firstVisit.unmount()
  const freshRead = deferred<CheckDirectory[]>()
  getDirectories.mockReturnValueOnce(freshRead.promise)
  const secondVisit = render(<ChecksStepSettings control="checkDirectories" />)
  expect(screen.getByText("Loading check folders…")).toBeVisible()
  expect(screen.queryByText("2 check files")).not.toBeInTheDocument()
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  await act(async () => freshRead.reject(new Error("private read failure")))
  expect(screen.queryByText("2 check files")).not.toBeInTheDocument()
  expect(screen.getByRole("alert")).toHaveTextContent(
    "Could not load check folders. Reopen Settings → Checks to try again.",
  )
  expect(
    screen.queryByRole("button", { name: /refresh|retry loading check folders/i }),
  ).not.toBeInTheDocument()
  secondVisit.unmount()
  getDirectories.mockResolvedValueOnce([
    { ...directories[0]!, reference: "new-global-ref", fileCount: 3 },
    ...directories.slice(1),
  ])
  render(<ChecksStepSettings control="checkDirectories" />)
  await screen.findByText("3 check files")
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: /^Open in .*This device/ }))
  await waitFor(() => expect(openDirectory).toHaveBeenCalledWith("new-global-ref"))
  expect(openDirectory.mock.calls).toEqual([["global-ref"], ["new-global-ref"]])
  expect(getDirectories).toHaveBeenCalledTimes(3)
  expect(providerInvoke.mock.calls.map(([command]) => command)).toEqual([
    "get_system_one_settings",
    "get_system_one_settings",
    "get_system_one_settings",
  ])
  expect(setChecksEnabled).not.toHaveBeenCalled()
})

it.each([
  ["macos", "Finder"],
  ["windows", "File Explorer"],
  ["linux", "file manager"],
  ["unknown", "file manager"],
])("uses the %s folder-open label", async (value, label) => {
  platform.mockReturnValue(value)
  render(<ChecksStepSettings />)
  fireEvent.click(screen.getByRole("button", { name: "Check files" }))
  expect(
    await screen.findByRole("button", {
      name: `Open in ${label}: This device (${directories[0]!.path})`,
    }),
  ).toBeEnabled()
})

it("keeps folder failures visible while closed and reloads only after reopening Checks", async () => {
  getDirectories.mockRejectedValueOnce(new Error("private read error"))
  const firstVisit = render(<ChecksStepSettings />)
  await screen.findByText(
    "Could not load check folders. Reopen Settings → Checks to try again.",
  )
  expect(screen.getByRole("button", { name: "Check files" })).toHaveAttribute(
    "aria-expanded",
    "false",
  )
  fireEvent.click(screen.getByRole("button", { name: "Check files" }))
  expect(getDirectories).toHaveBeenCalledOnce()
  expect(screen.queryByRole("button", { name: /refresh|retry/i })).not.toBeInTheDocument()
  firstVisit.unmount()
  render(<ChecksStepSettings control="checkDirectories" />)
  await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument())
  expect(getDirectories).toHaveBeenCalledTimes(2)
  expect(providerInvoke.mock.calls.map(([command]) => command)).toEqual([
    "get_system_one_settings",
    "get_system_one_settings",
  ])
  expect(setChecksEnabled).not.toHaveBeenCalled()
})

it("saves a credential without enabling checks", async () => {
  render(<ChecksStepSettings />)
  const actions = await screen.findByRole("button", { name: "More actions for Jev" })
  await waitFor(() => expect(actions).toBeEnabled())
  fireEvent.keyDown(actions, { key: "ArrowDown" })
  fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }))
  fireEvent.change(await screen.findByLabelText("Jev API key"), {
    target: { value: "synthetic-key" },
  })
  fireEvent.click(screen.getByRole("button", { name: "Save" }))
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  expect(screen.getByRole("switch", { name: "Run checks" })).not.toBeChecked()
  expect(setChecksEnabled).not.toHaveBeenCalled()
})

it("allows enabling after the first Add even when availability.configured stays false", async () => {
  providerInvoke.mockImplementation(async (command, args) => {
    if (command === "get_system_one_settings")
      return { activeId: null, profiles: {}, pendingOperation: null, displayNumbers: {} }
    if (command === "save_system_one_connection")
      return {
        pendingOperation: null,
        displayNumbers: { [args.draft.connection_id]: 1 },
        activeId: args.draft.connection_id,
        profiles: {
          [args.draft.connection_id]: {
            ...args.draft.connection,
            credential: { kind: "connection", id: "synthetic-key" },
          },
        },
      }
    throw new Error(`Unexpected command: ${command}`)
  })
  render(<ChecksStepSettings />)
  const add = screen.getByRole("button", { name: "Add connection" })
  await waitFor(() => expect(add).toBeEnabled())
  const run = screen.getByRole("switch", { name: "Run checks" })
  expect(run).toBeDisabled()
  fireEvent.click(add)
  fireEvent.change(screen.getByLabelText("Jev API key"), { target: { value: "synthetic-key" } })
  fireEvent.click(
    within(screen.getByRole("dialog")).getByRole("button", { name: "Add connection" }),
  )
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  expect(run).not.toBeChecked()
  expect(run).toBeEnabled()
  expect(setChecksEnabled).not.toHaveBeenCalled()
  fireEvent.click(run)
  await waitFor(() => expect(run).toBeChecked())
  expect(
    screen.queryByText("Add and select a connection before running checks."),
  ).not.toBeInTheDocument()
  expect(setChecksEnabled).toHaveBeenCalledExactlyOnceWith(true)
})

it("allows pausing enabled checks during pending recovery but blocks enabling again", async () => {
  getAvailability.mockResolvedValueOnce({ ...emptyAvailability, yamlEnabled: true })
  providerInvoke.mockResolvedValueOnce({
    activeId: "jev",
    profiles: { jev: defaultConnection("jev") },
    displayNumbers: { jev: 1 },
    pendingOperation: {
      id: "pending-operation",
      kind: "save",
      connectionId: "jev",
      requestId: "another-window",
    },
  })
  render(<ChecksStepSettings />)
  const run = screen.getByRole("switch", { name: "Run checks" })
  await waitFor(() => expect(run).toBeChecked())
  expect(
    screen.queryByText("Add and select a connection before running checks."),
  ).not.toBeInTheDocument()
  expect(run).toBeEnabled()
  fireEvent.click(run)
  await waitFor(() => expect(run).not.toBeChecked())
  expect(run).toBeDisabled()
  expect(setChecksEnabled).toHaveBeenCalledExactlyOnceWith(false)
})

it("uses one empty usage sentence instead of zero counters", async () => {
  render(<ChecksStepSettings />)
  await screen.findByText("No model requests recorded on this device.")
  fireEvent.click(screen.getByRole("button", { name: "Model usage" }))
  expect(screen.queryByText("Input tokens")).not.toBeInTheDocument()
  expect(screen.queryByText(/Clear index/)).not.toBeInTheDocument()
})

it.each(["$0.00000017", null])(
  "keeps known usage with %s cost and omits unknown fields",
  async (cost) => {
    getAvailability.mockResolvedValueOnce({
      ...emptyAvailability,
      usage: {
        ...emptyAvailability.usage,
        confirmedCalls: 2,
        inputTokens: 1200,
        unknownOutcomes: 1,
        estimatedUsd: cost,
      },
    })
    render(<ChecksStepSettings />)
    await screen.findByText(/2 requests with recorded usage/)
    expect(screen.getByText(/2 requests with recorded usage/).textContent).toBe(
      `2 requests with recorded usage${cost === null ? "" : ` · ${cost} estimated`}`,
    )
    expect(screen.queryByText(/unknown|cost estimate unavailable/i)).not.toBeInTheDocument()
    expect(screen.queryByText("Input tokens")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Model usage" }))
    expect(screen.getByText("Input tokens").nextElementSibling).toHaveTextContent("1,200")
    expect(screen.queryByText(/unknown|not included in the estimate/i)).not.toBeInTheDocument()
  },
)

it("omits unknown-only usage and keeps known unpriced usage without a cost status", async () => {
  getAvailability.mockResolvedValueOnce({
    ...emptyAvailability,
    usage: { ...emptyAvailability.usage, unknownOutcomes: 2, estimatedUsd: null },
  })
  render(<ChecksStepSettings />)
  await waitFor(() => expect(getAvailability).toHaveBeenCalledOnce())
  expect(screen.queryByRole("button", { name: "Model usage" })).not.toBeInTheDocument()
  expect(
    screen.queryByText("No model requests recorded on this device."),
  ).not.toBeInTheDocument()
  act(() =>
    availabilityEvent.callback?.({
      status: "updated",
      snapshot: {
        ...emptyAvailability,
        usage: { ...emptyAvailability.usage, confirmedCalls: 1, estimatedUsd: null },
      },
    }),
  )
  expect(screen.getByText("1 request with recorded usage")).toBeVisible()
  expect(screen.queryByText(/unknown|cost estimate unavailable/i)).not.toBeInTheDocument()
})

it.each(["checkDirectories", "checkModelUsage"] as const)(
  "search reveals and focuses %s without invoking an action",
  async (control) => {
    const { container, rerender } = render(
      <ChecksStepSettings control={control} targetRevision={1} />,
    )
    await screen.findByText("No model requests recorded on this device.")
    const focus = new SettingsTargetFocus()
    const stop = focus.attach(
      container.firstElementChild as HTMLDivElement,
      "checks",
      control,
      1,
    )
    const button = await screen.findByRole("button", {
      name: control === "checkDirectories" ? "Check files" : "Model usage",
    })
    expect(button).toHaveAttribute("aria-expanded", "true")
    expect(button).toHaveFocus()
    fireEvent.click(button)
    expect(button).toHaveAttribute("aria-expanded", "false")
    rerender(<ChecksStepSettings control={control} targetRevision={2} />)
    expect(button).toHaveAttribute("aria-expanded", "true")
    await waitFor(() => expect(providerInvoke).toHaveBeenCalledOnce())
    expect(openDirectory).not.toHaveBeenCalled()
    expect(setChecksEnabled).not.toHaveBeenCalled()
    stop()
  },
)

it.each(["unknown-only", "failed", "loading"])(
  "search focuses the model-based group when usage is %s without invoking a control",
  async (scenario) => {
    if (scenario === "unknown-only")
      getAvailability.mockResolvedValueOnce({
        ...emptyAvailability,
        usage: { ...emptyAvailability.usage, unknownOutcomes: 2, estimatedUsd: null },
      })
    else if (scenario === "failed")
      getAvailability.mockRejectedValueOnce(new Error("read failed"))
    else getAvailability.mockReturnValueOnce(deferred<CheckAvailability>().promise)
    const { container } = render(
      <ChecksStepSettings control="checkModelUsage" targetRevision={1} />,
    )
    await waitFor(() => expect(providerInvoke).toHaveBeenCalledOnce())
    if (scenario === "failed") await screen.findByRole("alert")
    const focus = new SettingsTargetFocus()
    const stop = focus.attach(
      container.firstElementChild as HTMLDivElement,
      "checks",
      "checkModelUsage",
      1,
    )
    const group = screen
      .getByRole("heading", { name: "Model-based checks" })
      .closest("section")!
    expect(group).toHaveFocus()
    expect(screen.queryByRole("button", { name: "Model usage" })).not.toBeInTheDocument()
    expect(
      screen.queryByText(/unknown usage|cost estimate unavailable/i),
    ).not.toBeInTheDocument()
    expect(setChecksEnabled).not.toHaveBeenCalled()
    expect(openDirectory).not.toHaveBeenCalled()
    expect(providerInvoke.mock.calls.map(([command]) => command)).toEqual([
      "get_system_one_settings",
    ])
    stop()
  },
)

it("keeps each concurrent toggle pending until its own request settles and retains its error", async () => {
  const skills = deferred<typeof emptyAvailability>()
  const servers = deferred<typeof emptyAvailability>()
  setCheckEnabled.mockImplementation((id: string) =>
    id === "unusedSkills" ? skills.promise : servers.promise,
  )
  render(<ChecksStepSettings />)
  act(() => availabilityEvent.callback?.({ status: "updated", snapshot: emptyAvailability }))
  const skillSwitch = screen.getByRole("switch", { name: "Unused skills" })
  const serverSwitch = screen.getByRole("switch", { name: "Unused MCP servers" })
  fireEvent.click(skillSwitch)
  fireEvent.click(serverSwitch)
  expect(skillSwitch).toBeDisabled()
  expect(serverSwitch).toBeDisabled()
  await act(async () => skills.reject(new Error("write failed")))
  expect(skillSwitch).toBeEnabled()
  expect(serverSwitch).toBeDisabled()
  expect(screen.getByRole("alert")).toHaveTextContent("Could not disable Unused skills")
  await act(async () => servers.resolve(emptyAvailability))
  expect(serverSwitch).toBeEnabled()
  expect(screen.getByRole("switch", { name: "Model overthinking" })).toBeChecked()
  expect(screen.getByRole("alert")).toHaveTextContent("Could not disable Unused skills")
})

it("omits stale usage while retaining the separate availability error and retry", async () => {
  getAvailability.mockResolvedValueOnce({
    ...emptyAvailability,
    usage: { ...emptyAvailability.usage, confirmedCalls: 2, inputTokens: 1200 },
  })
  render(<ChecksStepSettings />)
  await screen.findByText(/2 requests with recorded usage/)
  fireEvent.click(screen.getByRole("button", { name: "Model usage" }))
  act(() => availabilityEvent.callback?.({ status: "failed" }))
  expect(screen.queryByRole("button", { name: "Model usage" })).not.toBeInTheDocument()
  expect(screen.getByRole("alert")).toHaveTextContent("Could not refresh check status")
  expect(screen.queryByText("Input tokens")).not.toBeInTheDocument()
  expect(
    screen.queryByText(/2 requests with recorded usage|unknown|out of date.*usage/i),
  ).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Retry loading check status" }))
  await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument())
  expect(screen.getByText("No model requests recorded on this device.")).toBeVisible()
})

it("retries a failed availability subscription and does not hide failure after a toggle", async () => {
  subscribeAvailability.mockRejectedValueOnce(new Error("subscription unavailable"))
  render(<ChecksStepSettings />)
  await screen.findByText("Could not load check status. Try again.")
  fireEvent.click(screen.getByRole("switch", { name: "Run checks" }))
  await waitFor(() => expect(screen.getByRole("switch", { name: "Run checks" })).toBeChecked())
  expect(screen.getByRole("alert")).toHaveTextContent("Could not refresh check status")
  fireEvent.click(screen.getByRole("button", { name: "Retry loading check status" }))
  await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument())
  expect(subscribeAvailability).toHaveBeenCalledTimes(2)
})

it("ignores stale initial reads and events after unsubscribe", async () => {
  const read = deferred<CheckAvailability>()
  getAvailability.mockReturnValueOnce(read.promise)
  const { unmount } = render(<ChecksStepSettings />)
  await waitFor(() => expect(availabilityEvent.callback).not.toBeNull())
  const current = { ...emptyAvailability, revision: 2, yamlEnabled: true }
  act(() => availabilityEvent.callback?.({ status: "updated", snapshot: current }))
  await act(async () => read.resolve(emptyAvailability))
  expect(screen.getByRole("switch", { name: "Run checks" })).toBeChecked()
  const oldCallback = availabilityEvent.callback
  unmount()
  oldCallback?.({
    status: "updated",
    snapshot: { ...current, revision: 3, yamlEnabled: false },
  })
  getAvailability.mockReturnValueOnce(new Promise(() => undefined))
  render(<ChecksStepSettings />)
  expect(screen.getByRole("switch", { name: "Run checks" })).toBeChecked()
})
