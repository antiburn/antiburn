import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import {
  defaultConnection,
  type ProviderSettings,
  type SmartCheckProvider,
} from "../../lib/smartCheckProviders"
import { CheckProviderSettings } from "./CheckProviderSettings"
import { SettingsTargetFocus } from "./SettingsTargetFocus"
import { providerSettingsFixture } from "./tests/providerSettingsFixture"
import { isConnectionFocusTarget } from "./ConnectionDialog"

const invoke = vi.hoisted(() => vi.fn())
const confirmDiscard = vi.hoisted(() => vi.fn())
const platform = vi.hoisted(() => vi.fn())
vi.mock("@tauri-apps/api/core", () => ({ invoke }))
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: confirmDiscard }))
vi.mock("../../lib/platform", () => ({ detectPlatform: platform }))
let saved: ProviderSettings
function configured(provider: SmartCheckProvider) {
  return {
    ...defaultConnection(provider),
    model: provider === "jev" ? "jev-latest" : "synthetic-model",
    credential: { kind: "connection" as const, id: `${provider}-key` },
  }
}
async function actions(label = "Jev") {
  const button = await screen.findByRole("button", { name: `More actions for ${label}` })
  await waitFor(() => expect(button).toBeEnabled())
  await act(async () => button.focus())
  fireEvent.keyDown(button, { key: "Enter" })
  await waitFor(() => expect(button).toHaveAttribute("aria-expanded", "true"))
  await screen.findByRole("menuitem", { name: "Edit" })
  return button
}
async function edit(label = "Jev") {
  await actions(label)
  fireEvent.click(screen.getByRole("menuitem", { name: "Edit" }))
  return screen.findByRole("dialog")
}
async function remove(label = "Jev") {
  const opener = await actions(label)
  fireEvent.click(screen.getByRole("menuitem", { name: "Remove" }))
  return opener
}
async function add() {
  const button = screen.getByRole("button", { name: "Add connection" })
  await waitFor(() => expect(button).toBeEnabled())
  fireEvent.click(button)
  return screen.getByRole("dialog", { name: "Add connection" })
}
beforeEach(() => {
  platform.mockReset().mockReturnValue("macos")
  HTMLElement.prototype.scrollIntoView = vi.fn()
  HTMLDialogElement.prototype.showModal = function () {
    this.open = true
  }
  HTMLDialogElement.prototype.close = function () {
    this.open = false
  }
  saved = providerSettingsFixture({ activeId: "jev", profiles: { jev: configured("jev") } })
  confirmDiscard.mockReset().mockResolvedValue(true)
  invoke.mockReset().mockImplementation(async (command, args) => {
    if (command === "get_system_one_settings") {
      saved = providerSettingsFixture(saved)
      return saved
    }
    if (command === "test_system_one_connection") return null
    if (command === "list_ollama_models")
      return [
        { id: "local-one", label: "Local one" },
        { id: "local-two", label: "Local two" },
      ]
    if (command === "save_system_one_connection") {
      const draft = args.draft
      saved = providerSettingsFixture({
        ...saved,
        activeId:
          Object.keys(saved.profiles).length === 0 ? draft.connection_id : saved.activeId,
        profiles: {
          ...saved.profiles,
          [draft.connection_id]: {
            ...draft.connection,
            revision: draft.connection.revision + 1,
            credential: draft.credential
              ? { kind: "connection", id: draft.connection_id }
              : draft.connection.credential,
          },
        },
      })
      return saved
    }
    if (command === "switch_system_one_connection") {
      saved = { ...saved, activeId: args.connectionId }
      return saved
    }
    if (command === "remove_system_one_connection") {
      const profiles = { ...saved.profiles }
      delete profiles[args.connectionId]
      saved = {
        ...saved,
        activeId: saved.activeId === args.connectionId ? null : saved.activeId,
        profiles,
      }
      return saved
    }
    if (command === "remove_system_one_credential") {
      const profile = saved.profiles[args.connectionId]!
      saved = {
        ...saved,
        profiles: {
          ...saved.profiles,
          [args.connectionId]: { ...profile, credential: null, revision: profile.revision + 1 },
        },
      }
      return null
    }
    if (command === "remove_typesafe_api_key") {
      saved = {
        ...saved,
        profiles: Object.fromEntries(
          Object.entries(saved.profiles).map(([id, profile]) => [
            id,
            profile.credential?.kind === "legacy_type_safe"
              ? { ...profile, revision: profile.revision + 1 }
              : profile,
          ]),
        ),
      }
      return null
    }
    throw new Error(`Unexpected command: ${command}`)
  })
})

it("shows Add and Use with one row menu for Edit and Remove", async () => {
  saved.profiles.work = configured("jev")
  saved.profiles.local = configured("ollama")
  render(<CheckProviderSettings />)
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Add connection" })).toBeEnabled(),
  )
  expect(screen.getAllByRole("listitem")).toHaveLength(3)
  expect(
    within(screen.getByRole("listitem", { name: "Jev" })).getByText("Selected"),
  ).toBeVisible()
  expect(screen.getByRole("listitem", { name: "Jev" })).toHaveAccessibleDescription("Selected")
  expect(screen.queryByRole("button", { name: "Use Jev" })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "More actions for Jev 2" })).toBeEnabled()
  expect(screen.queryByRole("button", { name: /^Edit |^Remove / })).not.toBeInTheDocument()
  expect(screen.queryByRole("menuitem")).not.toBeInTheDocument()
  const use = screen.getByRole("button", { name: "Use Jev 2" })
  use.focus()
  fireEvent.keyDown(use, { key: "ArrowDown" })
  expect(saved.activeId).toBe("jev")
  expect(invoke.mock.calls.map(([command]) => command)).toEqual(["get_system_one_settings"])
  fireEvent.click(use)
  await waitFor(() => expect(saved.activeId).toBe("work"))
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "More actions for Jev 2" })).toHaveFocus(),
  )
  expect(invoke).toHaveBeenCalledWith("switch_system_one_connection", { connectionId: "work" })
  expect(screen.queryByRole("combobox")).not.toBeInTheDocument()
  expect(screen.queryByText(/jev-latest|work-key/)).not.toBeInTheDocument()
})

it("edits an inactive connection without activating it, then uses it only on request", async () => {
  saved.profiles.work = configured("jev")
  render(<CheckProviderSettings />)
  const original = structuredClone(saved.profiles.work)
  const dialog = await edit("Jev 2")
  expect(dialog).toHaveAccessibleName("Edit Jev 2 connection")
  expect(screen.queryByLabelText("Provider")).not.toBeInTheDocument()
  expect(screen.getByText("Saved", { exact: true })).toBeVisible()
  expect(screen.queryByLabelText("Jev API key")).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Change" }))
  fireEvent.change(screen.getByLabelText("Jev API key"), { target: { value: "synthetic-key" } })
  fireEvent.click(screen.getByRole("button", { name: "Save" }))
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  expect(saved.activeId).toBe("jev")
  expect(invoke).toHaveBeenCalledWith("save_system_one_connection", {
    draft: { connection_id: "work", connection: original, credential: "synthetic-key" },
    intent: "update",
    requestId: expect.any(String),
  })
  fireEvent.click(screen.getByRole("button", { name: "Use Jev 2" }))
  await waitFor(() => expect(saved.activeId).toBe("work"))
})

it.each([true, false])(
  "adds from an empty list=%s, selecting only the first saved connection",
  async (empty) => {
    if (empty) saved = providerSettingsFixture({ activeId: null, profiles: {} })
    render(<CheckProviderSettings />)
    const dialog = await add()
    expect(screen.getByLabelText("Provider")).toHaveFocus()
    const save = within(dialog).getByRole("button", { name: "Add connection" })
    expect(save).toBeDisabled()
    fireEvent.change(screen.getByLabelText("Jev API key"), {
      target: { value: "synthetic-second-key" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }))
    await screen.findByText("Connection test passed.")
    expect(saved.activeId).toBe(empty ? null : "jev")
    fireEvent.click(save)
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
    const call = invoke.mock.calls.find(
      ([command]) => command === "save_system_one_connection",
    )!
    const id = call[1].draft.connection_id
    expect(id).toMatch(new RegExp(`^jev-${empty ? 1 : 2}-[a-f0-9-]{36}$`))
    expect(id.length).toBeLessThanOrEqual(64)
    expect(call[1].intent).toBe("create")
    expect(saved.activeId).toBe(empty ? id : "jev")
    expect(invoke.mock.calls.map(([command]) => command)).toEqual([
      "get_system_one_settings",
      "test_system_one_connection",
      "save_system_one_connection",
    ])
  },
)

it.each(["work", "jev"])(
  "removes %s with the correct pause warning and keeps all other connections",
  async (id) => {
    saved.profiles.work = configured("jev")
    render(<CheckProviderSettings />)
    await remove(id === "jev" ? "Jev" : "Jev 2")
    await waitFor(() => expect(saved.profiles[id]).toBeUndefined())
    expect(saved.activeId).toBe(id === "jev" ? null : "jev")
    expect(confirmDiscard.mock.calls[0]![0]).toContain(
      id === "jev" ? "pauses model-based checks" : "does not pause checks",
    )
    expect(confirmDiscard.mock.calls[0]![0]).toContain("Saved results and model usage stay")
    expect(invoke).toHaveBeenCalledWith("remove_system_one_connection", {
      connectionId: id,
      revision: 1,
    })
  },
)

it("removes the last connection and leaves Add usable", async () => {
  render(<CheckProviderSettings />)
  await remove()
  await screen.findByText("Add a connection to set up model-based checks.")
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Add connection" })).toHaveFocus(),
  )
  expect(saved).toMatchObject({ activeId: null, profiles: {}, pendingOperation: null })
  await add()
  expect(screen.getByLabelText("Jev API key")).toHaveValue("")
})

it.each(["cancel", "failure"])(
  "restores the row menu trigger after Remove %s",
  async (outcome) => {
    render(<CheckProviderSettings />)
    const opener = await actions()
    if (outcome === "cancel") confirmDiscard.mockResolvedValueOnce(false)
    else invoke.mockRejectedValueOnce(new Error("Could not remove the connection."))
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove" }))
    await waitFor(() => expect(opener).toBeEnabled())
    await waitFor(() => expect(opener).toHaveFocus())
    expect(saved.profiles.jev).toBeDefined()
    expect(screen.queryByText("Connection removed.")).not.toBeInTheDocument()
  },
)

it("offers explicit use or forget of a retained shared key in Add after all connections are gone", async () => {
  saved = providerSettingsFixture({ activeId: null, profiles: {} })
  const { rerender } = render(<CheckProviderSettings legacyKeySaved />)
  const dialog = await add()
  expect(within(dialog).getByRole("button", { name: "Add connection" })).toBeDisabled()
  fireEvent.click(screen.getByRole("button", { name: "Use saved key" }))
  expect(within(dialog).getByRole("button", { name: "Add connection" })).toBeEnabled()
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  rerender(<CheckProviderSettings legacyKeySaved />)
  await add()
  fireEvent.click(screen.getByRole("button", { name: "Forget shared TypeSafe key" }))
  await screen.findByText("Shared TypeSafe key forgotten.")
  expect(confirmDiscard).toHaveBeenCalledWith(
    "Forget this key? Other connections that use it will stop working.",
    expect.anything(),
  )
  expect(invoke).toHaveBeenCalledWith("remove_typesafe_api_key")
  expect(screen.queryByRole("button", { name: "Use saved key" })).not.toBeInTheDocument()
})

it.each(["Cancel", "Escape"])(
  "restores the actual Add opener after %s without writing or asking to discard untouched fields",
  async (action) => {
    render(<CheckProviderSettings />)
    const dialog = await add()
    const provider = screen.getByLabelText("Provider")
    fireEvent.keyDown(provider, { key: "Tab", shiftKey: true })
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus()
    fireEvent.keyDown(screen.getByRole("button", { name: "Cancel" }), { key: "Tab" })
    expect(provider).toHaveFocus()
    if (action === "Cancel") fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    else fireEvent(dialog, new Event("cancel", { cancelable: true }))
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
    expect(screen.getByRole("button", { name: "Add connection" })).toHaveFocus()
    expect(confirmDiscard).not.toHaveBeenCalled()
    expect(invoke.mock.calls.map(([command]) => command)).toEqual(["get_system_one_settings"])
  },
)

it.each(["Cancel", "Escape"])(
  "keeps dialog focus after menu cleanup and restores the row trigger after %s",
  async (action) => {
    render(<CheckProviderSettings />)
    const dialog = await edit()
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(screen.queryByRole("menu")).not.toBeInTheDocument()
    expect(within(dialog).getByRole("button", { name: "Change" })).toHaveFocus()
    if (action === "Cancel") fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    else fireEvent(dialog, new Event("cancel", { cancelable: true }))
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
    expect(screen.getByRole("button", { name: "More actions for Jev" })).toHaveFocus()
    expect(confirmDiscard).not.toHaveBeenCalled()
    expect(invoke.mock.calls.map(([command]) => command)).toEqual(["get_system_one_settings"])
  },
)

it("keeps a declined discard draft, then clears it and restores the Edit opener", async () => {
  render(<CheckProviderSettings />)
  const dialog = await edit()
  fireEvent.click(screen.getByRole("button", { name: "Change" }))
  fireEvent.change(screen.getByLabelText("Jev API key"), {
    target: { value: "synthetic-replacement" },
  })
  confirmDiscard.mockResolvedValueOnce(false)
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
  await waitFor(() => expect(confirmDiscard).toHaveBeenCalledOnce())
  expect(screen.getByLabelText("Jev API key")).toHaveValue("synthetic-replacement")
  fireEvent(dialog, new Event("cancel", { cancelable: true }))
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  expect(screen.getByRole("button", { name: "More actions for Jev" })).toHaveFocus()
  await edit()
  expect(screen.queryByLabelText("Jev API key")).not.toBeInTheDocument()
})

for (const os of ["macos", "windows", "linux"] as const) {
  it.each([
    ["jev", "typeSafeApiKey", "Jev API key"],
    ["ollama", "ollamaUrl", "Ollama URL"],
    ["ollama", "ollamaModel", "Ollama model"],
    ["ollama", "ollamaApiKey", "Ollama API key (optional)"],
    ["cloudflare", "cloudflareAccountId", "Cloudflare account ID"],
    ["cloudflare", "cloudflareApiToken", "Cloudflare API token"],
    ["cloudflare", "cloudflareModel", "Cloudflare model"],
    ["custom", "customUrl", "Exact inference URL"],
    ["custom", "customModel", "Custom model"],
    ["custom", "customApiKey", "Custom API key (optional)"],
    ["custom", "customResponseMode", "Response mode"],
  ] as const)(
    `focuses the %s field %s on ${os} without writes or provider requests`,
    async (provider, control, label) => {
      platform.mockReturnValue(os)
      saved.profiles.target = configured(provider)
      saved = providerSettingsFixture(saved)
      const original = structuredClone(saved)
      const { container, rerender } = render(
        <CheckProviderSettings control={control} targetRevision={1} />,
      )
      const focus = new SettingsTargetFocus()
      const stop = focus.attach(
        container.firstElementChild as HTMLDivElement,
        "checks",
        control,
        1,
      )
      await waitFor(() => expect(screen.getByLabelText(label)).toHaveFocus())
      expect(screen.getByLabelText<HTMLInputElement>(label).labels).toHaveLength(1)
      const cancel = screen.getByRole("button", { name: "Cancel" })
      cancel.focus()
      rerender(<CheckProviderSettings control={control} targetRevision={1} />)
      expect(cancel).toHaveFocus()
      rerender(<CheckProviderSettings control={control} targetRevision={2} />)
      focus.attach(container.firstElementChild as HTMLDivElement, "checks", control, 2)()
      expect(screen.getByLabelText(label)).toHaveFocus()
      expect(saved).toEqual(original)
      expect(invoke.mock.calls.map(([command]) => command)).toEqual(["get_system_one_settings"])
      stop()
    },
  )
}

it("keeps a non-first search target through failed-read Retry", async () => {
  saved.profiles.custom = configured("custom")
  invoke.mockRejectedValueOnce(new Error("private read failure"))
  const { container } = render(
    <CheckProviderSettings control="customResponseMode" targetRevision={1} />,
  )
  const focus = new SettingsTargetFocus()
  const stop = focus.attach(
    container.firstElementChild as HTMLDivElement,
    "checks",
    "customResponseMode",
    1,
  )
  const retry = await screen.findByRole("button", { name: "Retry loading connections" })
  await waitFor(() => expect(retry).toHaveFocus())
  fireEvent.click(retry)
  await waitFor(() => expect(screen.getByLabelText("Response mode")).toHaveFocus())
  expect(screen.getByLabelText("Exact inference URL")).not.toHaveFocus()
  expect(invoke.mock.calls.map(([command]) => command)).toEqual([
    "get_system_one_settings",
    "get_system_one_settings",
  ])
  stop()
})

it.each(["jev", "ollama", "cloudflare", "custom"] as const)(
  "disables the %s Add form during recovery reload and keeps its create request",
  async (provider) => {
    render(<CheckProviderSettings legacyKeySaved />)
    const dialog = await add()
    fireEvent.change(screen.getByLabelText("Provider"), { target: { value: provider } })
    if (provider === "jev")
      fireEvent.change(screen.getByLabelText("Jev API key"), {
        target: { value: "keep-secret" },
      })
    else if (provider === "ollama") {
      fireEvent.click(screen.getByRole("button", { name: "Find models" }))
      await screen.findByRole("option", { name: "Local one" })
      fireEvent.change(screen.getByLabelText("Ollama model"), {
        target: { value: "local-one" },
      })
    } else if (provider === "cloudflare") {
      fireEvent.change(screen.getByLabelText("Cloudflare account ID"), {
        target: { value: "keep-account" },
      })
      fireEvent.change(screen.getByLabelText("Cloudflare API token"), {
        target: { value: "keep-secret" },
      })
    } else {
      fireEvent.change(screen.getByLabelText("Exact inference URL"), {
        target: { value: "https://api.typesafe.ai/v1/systemone" },
      })
      fireEvent.change(screen.getByLabelText("Custom model"), {
        target: { value: "jev-1.14.0" },
      })
    }
    const values = () =>
      [...dialog.querySelectorAll<HTMLInputElement | HTMLSelectElement>("input, select")].map(
        (field) => field.value,
      )
    const retainedValues = values()
    invoke
      .mockRejectedValueOnce(new Error("save failed"))
      .mockRejectedValueOnce(new Error("read failed"))
    fireEvent.click(within(dialog).getByRole("button", { name: "Add connection" }))
    const retry = await screen.findByRole("button", { name: "Retry loading connections" })
    const firstSave = invoke.mock.calls.find(
      ([command]) => command === "save_system_one_connection",
    )![1]
    let finish!: (settings: ProviderSettings) => void
    invoke.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    fireEvent.click(retry)
    for (const field of dialog.querySelectorAll("input, select, button"))
      expect(field).toBeDisabled()
    fireEvent.change(screen.getByLabelText("Provider"), {
      target: { value: provider === "ollama" ? "jev" : "ollama" },
    })
    expect(confirmDiscard).not.toHaveBeenCalled()
    await act(async () => finish(saved))
    expect(values()).toEqual(retainedValues)
    expect(screen.getByRole("dialog", { name: "Add connection" })).toBe(dialog)
    fireEvent.click(within(dialog).getByRole("button", { name: "Add connection" }))
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
    expect(invoke).toHaveBeenLastCalledWith("save_system_one_connection", {
      draft: firstSave.draft,
      intent: "create",
      requestId: expect.any(String),
    })
    expect(saved.profiles[firstSave.draft.connection_id]?.provider).toBe(provider)
    expect(saved.profiles.jev).toEqual(configured("jev"))
  },
)

it("opens the search field while a different row menu is open without selecting a menu action", async () => {
  saved.profiles.custom = configured("custom")
  const { container, rerender } = render(<CheckProviderSettings />)
  await actions("Jev")
  rerender(<CheckProviderSettings control="customResponseMode" targetRevision={1} />)
  const focus = new SettingsTargetFocus()
  const stop = focus.attach(
    container.firstElementChild as HTMLDivElement,
    "checks",
    "customResponseMode",
    1,
  )
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
  expect(screen.getByLabelText("Response mode")).toHaveFocus()
  expect(screen.queryByRole("menu")).not.toBeInTheDocument()
  expect(invoke.mock.calls.map(([command]) => command)).toEqual(["get_system_one_settings"])
  stop()
})

it("keeps a failed Use error outside the editor and does not mark the wrong connection selected", async () => {
  saved.profiles.work = configured("jev")
  render(<CheckProviderSettings />)
  const use = await screen.findByRole("button", { name: "Use Jev 2" })
  await waitFor(() => expect(use).toBeEnabled())
  invoke.mockRejectedValueOnce(new Error("The selected provider needs a valid credential."))
  fireEvent.click(use)
  await screen.findByText("The selected provider needs a valid credential.")
  await waitFor(() => expect(use).toHaveFocus())
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  expect(
    within(screen.getByRole("listitem", { name: "Jev" })).getByText("Selected"),
  ).toBeVisible()
})

it.each([false, true])(
  "preserves a partial-save draft when readback fails=%s without claiming rollback",
  async (failedRead) => {
    render(<CheckProviderSettings />)
    await edit()
    fireEvent.click(screen.getByRole("button", { name: "Change" }))
    fireEvent.change(screen.getByLabelText("Jev API key"), { target: { value: "keep-secret" } })
    invoke.mockImplementationOnce(async (_command, { requestId }) => {
      saved = {
        ...saved,
        profiles: { ...saved.profiles, jev: { ...saved.profiles.jev!, revision: 2 } },
        pendingOperation: { id: "pending-save", kind: "save", connectionId: "jev", requestId },
      }
      throw new Error(
        "Could not complete the connection operation. Use Retry in Settings → Checks.",
      )
    })
    if (failedRead) invoke.mockRejectedValueOnce(new Error("read failed"))
    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    await screen.findByText(
      "Could not complete the connection operation. Use Retry in Settings → Checks.",
    )
    await waitFor(() => expect(screen.getByLabelText("Jev API key")).toBeEnabled())
    expect(screen.getByLabelText("Jev API key")).toHaveValue("keep-secret")
    if (failedRead) {
      expect(
        screen.getByText("Could not confirm the active connection. Retry loading connections."),
      ).toBeVisible()
      expect(screen.queryByText("Selected", { exact: true })).not.toBeInTheDocument()
      fireEvent.click(screen.getByRole("button", { name: "Retry loading connections" }))
      await waitFor(() => expect(screen.getByText("Selected", { exact: true })).toBeVisible())
    }
  },
)

it("keeps pending removal Retry reachable and focused after deletion and remount", async () => {
  const first = render(<CheckProviderSettings />)
  await actions()
  invoke.mockImplementationOnce(async () => {
    saved = {
      ...saved,
      activeId: null,
      profiles: {},
      pendingOperation: {
        id: "remove-token",
        kind: "remove",
        connectionId: "jev",
        requestId: null,
      },
    }
    throw new Error("Could not finish credential cleanup. Use Retry in Settings → Checks.")
  })
  fireEvent.click(screen.getByRole("menuitem", { name: "Remove" }))
  const retry = await screen.findByRole("button", { name: "Retry connection change" })
  await waitFor(() => expect(retry).toHaveFocus())
  expect(screen.queryByRole("button", { name: "More actions for Jev" })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Add connection" })).toBeDisabled()
  first.unmount()
  render(<CheckProviderSettings />)
  const remounted = await screen.findByRole("button", { name: "Retry connection change" })
  invoke.mockImplementationOnce(async (_command, args) => {
    expect(args).toEqual({ operationId: "remove-token" })
    saved = { ...saved, pendingOperation: null }
    return { settings: saved, outcome: "completed" }
  })
  fireEvent.click(remounted)
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Retry connection change" }),
    ).not.toBeInTheDocument(),
  )
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Add connection" })).toHaveFocus(),
  )
})

it.each(["Cancel", "Escape"])(
  "focuses global Retry after pending Add and %s without discarding its draft",
  async (action) => {
    saved = providerSettingsFixture({ activeId: null, profiles: {} })
    render(<CheckProviderSettings />)
    const opener = screen.getByRole("button", { name: "Add connection" })
    const dialog = await add()
    fireEvent.change(screen.getByLabelText("Jev API key"), {
      target: { value: "keep-add-secret" },
    })
    invoke.mockImplementationOnce(async (_command, { draft, requestId }) => {
      saved = {
        ...saved,
        pendingOperation: {
          id: "pending-add",
          kind: "save",
          connectionId: draft.connection_id,
          requestId,
        },
      }
      throw new Error(
        "Could not complete the connection operation. Use Retry in Settings → Checks.",
      )
    })
    fireEvent.click(within(dialog).getByRole("button", { name: "Add connection" }))
    await screen.findByRole("button", { name: "Retry connection change" })
    if (action === "Cancel") fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    else fireEvent(dialog, new Event("cancel", { cancelable: true }))
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
    const retry = screen.getByRole("button", { name: "Retry connection change" })
    await waitFor(() => expect(retry).toHaveFocus())
    expect(screen.getAllByRole("button", { name: "Retry connection change" })).toHaveLength(1)
    expect(opener).toBeDisabled()
    expect(saved.pendingOperation?.id).toBe("pending-add")
    expect(saved.profiles).toEqual({})
    expect(confirmDiscard).not.toHaveBeenCalled()
    expect(invoke.mock.calls.map(([command]) => command)).toEqual([
      "get_system_one_settings",
      "save_system_one_connection",
      "get_system_one_settings",
    ])
    invoke.mockImplementationOnce(async () => {
      saved = { ...saved, pendingOperation: null }
      return { settings: saved, outcome: "rolled_back" }
    })
    fireEvent.click(retry)
    await waitFor(() => expect(opener).toBeEnabled())
    fireEvent.click(opener)
    expect(screen.getByLabelText("Jev API key")).toHaveValue("keep-add-secret")
    expect(invoke.mock.calls.map(([command]) => command)).toEqual([
      "get_system_one_settings",
      "save_system_one_connection",
      "get_system_one_settings",
      "retry_system_one_connection_operation",
    ])
  },
)

it.each([
  ["switch", "Jev 2", "work", "switch_system_one_connection"],
  ["remove", "Jev", "jev", "remove_system_one_connection"],
] as const)(
  "focuses Retry after precommit %s and keeps unsafe actions disabled",
  async (kind, label, connectionId, command) => {
    saved.profiles.work = configured("jev")
    render(<CheckProviderSettings />)
    const opener = await screen.findByRole("button", {
      name: kind === "switch" ? `Use ${label}` : `More actions for ${label}`,
    })
    await waitFor(() => expect(opener).toBeEnabled())
    if (kind === "remove") await actions(label)
    invoke.mockImplementationOnce(async () => {
      saved = {
        ...saved,
        pendingOperation: { id: "precommit-operation", kind, connectionId, requestId: null },
      }
      throw new Error(
        "Could not complete the connection operation. Use Retry in Settings → Checks.",
      )
    })
    fireEvent.click(
      kind === "switch" ? opener : screen.getByRole("menuitem", { name: "Remove" }),
    )
    const retry = await screen.findByRole("button", { name: "Retry connection change" })
    await waitFor(() => expect(retry).toHaveFocus())
    expect(opener).toBeInTheDocument()
    if (kind === "switch") expect(opener).toBeDisabled()
    else {
      expect(opener).toBeEnabled()
      await actions(label)
      expect(screen.getByRole("menuitem", { name: "Remove" })).toHaveAttribute(
        "aria-disabled",
        "true",
      )
      expect(screen.getByRole("menuitem", { name: "Edit" })).not.toHaveAttribute(
        "aria-disabled",
        "true",
      )
    }
    expect(screen.getAllByRole("listitem")).toHaveLength(2)
    expect(saved.activeId).toBe("jev")
    expect(saved.pendingOperation?.id).toBe("precommit-operation")
    expect(invoke.mock.calls.map(([called]) => called)).toEqual([
      "get_system_one_settings",
      command,
      "get_system_one_settings",
    ])
  },
)

it("does not treat disabled, inert, hidden, or disconnected openers as focus targets", () => {
  const { container, unmount } = render(<button type="button">Opener</button>)
  const opener = screen.getByRole("button", { name: "Opener" })
  expect(isConnectionFocusTarget(opener)).toBe(true)
  opener.setAttribute("disabled", "")
  expect(isConnectionFocusTarget(opener)).toBe(false)
  opener.removeAttribute("disabled")
  container.setAttribute("inert", "")
  expect(isConnectionFocusTarget(opener)).toBe(false)
  container.removeAttribute("inert")
  container.hidden = true
  expect(isConnectionFocusTarget(opener)).toBe(false)
  container.hidden = false
  unmount()
  expect(isConnectionFocusTarget(opener)).toBe(false)
})

it("closes a pending editor without discarding the secret and completes only its own retry", async () => {
  render(<CheckProviderSettings />)
  await edit()
  fireEvent.click(screen.getByRole("button", { name: "Change" }))
  fireEvent.change(screen.getByLabelText("Jev API key"), { target: { value: "keep-secret" } })
  invoke.mockImplementationOnce(async (_command, { requestId }) => {
    saved = {
      ...saved,
      profiles: { ...saved.profiles, jev: { ...saved.profiles.jev!, revision: 2 } },
      pendingOperation: { id: "save-token", kind: "save", connectionId: "jev", requestId },
    }
    throw new Error(
      "Could not complete the connection operation. Use Retry in Settings → Checks.",
    )
  })
  fireEvent.click(screen.getByRole("button", { name: "Save" }))
  await screen.findByRole("button", { name: "Retry connection change" })
  expect(screen.getByRole("button", { name: "Save" })).toBeDisabled()
  expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled()
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  expect(screen.getByRole("button", { name: "Retry connection change" })).toBeVisible()
  expect(confirmDiscard).not.toHaveBeenCalled()
  await edit()
  expect(screen.getByLabelText("Jev API key")).toHaveValue("keep-secret")
  invoke.mockImplementationOnce(async () => {
    saved = { ...saved, pendingOperation: null }
    return { settings: saved, outcome: "completed" }
  })
  fireEvent.click(screen.getByRole("button", { name: "Retry connection change" }))
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  await edit()
  expect(screen.queryByLabelText("Jev API key")).not.toBeInTheDocument()
})

it("submits explicit saved-key reuse instead of only enabling Add", async () => {
  saved = providerSettingsFixture({ activeId: null, profiles: {} })
  render(<CheckProviderSettings legacyKeySaved />)
  const dialog = await add()
  fireEvent.click(screen.getByRole("button", { name: "Use saved key" }))
  fireEvent.click(within(dialog).getByRole("button", { name: "Add connection" }))
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  const call = invoke.mock.calls.find(([command]) => command === "save_system_one_connection")!
  expect(call[1]).toMatchObject({
    intent: "create",
    requestId: expect.any(String),
    draft: {
      credential: null,
      connection: { provider: "jev", credential: { kind: "legacy_type_safe" } },
    },
  })
})

it("saves a retained Jev replacement after Add forgets the shared key", async () => {
  saved.profiles.jev = { ...configured("jev"), credential: { kind: "legacy_type_safe" } }
  const { rerender } = render(<CheckProviderSettings legacyKeySaved />)
  await edit()
  fireEvent.click(screen.getByRole("button", { name: "Change" }))
  fireEvent.change(screen.getByLabelText("Jev API key"), {
    target: { value: "retained-replacement" },
  })
  rerender(
    <CheckProviderSettings legacyKeySaved control="decisionProvider" targetRevision={1} />,
  )
  const dialog = screen.getByRole("dialog", { name: "Add connection" })
  fireEvent.click(screen.getByRole("button", { name: "Forget shared TypeSafe key" }))
  await waitFor(() => expect(screen.getByLabelText("Jev API key")).toBeEnabled())
  fireEvent.change(screen.getByLabelText("Jev API key"), {
    target: { value: "new-connection-key" },
  })
  fireEvent.click(within(dialog).getByRole("button", { name: "Add connection" }))
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  await edit()
  expect(screen.getByLabelText("Jev API key")).toHaveValue("retained-replacement")
  expect(screen.queryByRole("button", { name: "Forget shared TypeSafe key" })).toBeNull()
  fireEvent.click(screen.getByRole("button", { name: "Save" }))
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  expect(invoke.mock.calls.at(-1)).toEqual([
    "save_system_one_connection",
    expect.objectContaining({
      intent: "update",
      draft: expect.objectContaining({
        connection_id: "jev",
        credential: "retained-replacement",
        connection: expect.objectContaining({ revision: 2 }),
      }),
    }),
  ])
})

it("blocks focused controls, duplicate saves, dismissal, and defers search until Save completes", async () => {
  saved.profiles.local = configured("ollama")
  const { container, rerender } = render(<CheckProviderSettings />)
  await edit()
  const save = screen.getByRole("button", { name: "Save" })
  save.focus()
  let finish!: () => void
  invoke.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = () => resolve(saved)
      }),
  )
  fireEvent.click(save)
  for (const field of screen.getByRole("dialog").querySelectorAll("input, select, button"))
    expect(field).toBeDisabled()
  fireEvent.click(save)
  fireEvent(screen.getByRole("dialog"), new Event("cancel", { cancelable: true }))
  rerender(<CheckProviderSettings control="ollamaModel" targetRevision={1} />)
  const focus = new SettingsTargetFocus()
  const stop = focus.attach(
    container.firstElementChild as HTMLDivElement,
    "checks",
    "ollamaModel",
    1,
  )
  expect(screen.queryByLabelText("Ollama model")).not.toBeInTheDocument()
  await act(async () => finish())
  await waitFor(() => expect(screen.getByLabelText("Ollama model")).toHaveFocus())
  expect(confirmDiscard).not.toHaveBeenCalled()
  expect(invoke.mock.calls.map(([command]) => command)).toEqual([
    "get_system_one_settings",
    "save_system_one_connection",
  ])
  stop()
})

it("looks up Ollama models only on Find and never auto-selects one", async () => {
  render(<CheckProviderSettings />)
  const dialog = await add()
  fireEvent.change(screen.getByLabelText("Provider"), { target: { value: "ollama" } })
  const model = screen.getByRole("combobox", { name: "Ollama model" })
  expect(model).toHaveValue("")
  expect(invoke.mock.calls.map(([command]) => command)).toEqual(["get_system_one_settings"])
  fireEvent.click(screen.getByRole("button", { name: "Find models" }))
  await screen.findByRole("option", { name: "Local two" })
  expect(model).toHaveValue("")
  expect(within(dialog).getByRole("button", { name: "Add connection" })).toBeDisabled()
  fireEvent.change(model, { target: { value: "local-two" } })
  expect(within(dialog).getByRole("button", { name: "Add connection" })).toBeEnabled()
  expect(saved.activeId).toBe("jev")
})

it("keeps the saved Ollama choice after a failed reload and a canceled late lookup", async () => {
  saved = providerSettingsFixture({
    activeId: "local",
    profiles: { local: configured("ollama") },
  })
  render(<CheckProviderSettings />)
  await edit("Ollama · Synthetic model")
  const model = screen.getByLabelText("Ollama model")
  expect(model).toHaveValue("synthetic-model")
  fireEvent.click(screen.getByRole("button", { name: "Find models" }))
  await screen.findByRole("option", { name: "Local one" })
  expect(model).toHaveValue("synthetic-model")
  invoke.mockRejectedValueOnce(new Error("private provider output"))
  fireEvent.click(screen.getByRole("button", { name: "Reload models" }))
  await screen.findByText(
    "Could not find installed models. Check the server URL and authentication, then try again.",
  )
  expect(screen.getByRole("option", { name: "Local two" })).toBeInTheDocument()
  expect(screen.queryByText(/private provider output/)).not.toBeInTheDocument()
  let resolve!: (models: Array<{ id: string; label: string }>) => void
  invoke.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done
      }),
  )
  fireEvent.click(screen.getByRole("button", { name: "Reload models" }))
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  await act(async () => resolve([{ id: "stale", label: "Stale model" }]))
  await edit("Ollama · Synthetic model")
  expect(screen.queryByRole("option", { name: "Stale model" })).not.toBeInTheDocument()
})
