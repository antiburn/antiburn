import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import {
  defaultConnection,
  type ProviderSettings,
  type SmartCheckProvider,
} from "../../lib/smartCheckProviders"
import { searchApp, resolveStepSettingsSearchTarget } from "../../lib/appSearch"
import { CheckProviderSettings } from "./CheckProviderSettings"
import { SettingsTargetFocus } from "./SettingsTargetFocus"

const invoke = vi.hoisted(() => vi.fn())
vi.mock("@tauri-apps/api/core", () => ({ invoke }))
let saved: ProviderSettings
beforeEach(() => {
  saved = { activeId: "jev", profiles: { jev: defaultConnection("jev") } }
  invoke.mockReset()
  invoke.mockImplementation(async (command, args) => {
    if (command === "get_system_one_settings") return saved
    if (command === "save_system_one_connection") {
      saved = {
        activeId: args.draft.connection_id,
        profiles: { ...saved.profiles, [args.draft.connection_id]: args.draft.connection },
      }
      return saved
    }
    if (command === "switch_system_one_connection")
      return { ...saved, activeId: args.connectionId }
    return null
  })
})

async function setup(provider: SmartCheckProvider) {
  render(<CheckProviderSettings />)
  await waitFor(() => expect(screen.getByLabelText("TypeSafe API key")).toBeEnabled())
  fireEvent.change(screen.getByLabelText("Smart check provider"), {
    target: { value: provider },
  })
  if (provider === "ollama" || provider === "custom")
    fireEvent.change(screen.getByLabelText("Provider model"), {
      target: { value: "test-model" },
    })
  if (provider === "custom")
    fireEvent.change(screen.getByLabelText("Exact inference URL"), {
      target: { value: "https://proxy.example/prefix/infer?mode=one" },
    })
  if (provider === "cloudflare")
    fireEvent.change(screen.getByLabelText("Account ID"), { target: { value: "test-account" } })
}

it("shows the decision model flow without opening advanced settings", async () => {
  await setup("jev")
  expect(screen.getByText("Decision model")).toBeVisible()
  expect(screen.queryByText("Model connection")).not.toBeInTheDocument()
  expect(screen.queryByText(/Active connection:|Editing:/)).not.toBeInTheDocument()
  const provider = screen.getByLabelText("Smart check provider")
  const model = screen.getByLabelText("Provider model")
  const credential = screen.getByLabelText("TypeSafe API key")
  expect(provider).toBeVisible()
  expect(model).toHaveValue(saved.profiles.jev?.model)
  expect(model).toHaveAttribute("readonly")
  expect(
    provider.compareDocumentPosition(model) & Node.DOCUMENT_POSITION_FOLLOWING,
  ).toBeTruthy()
  expect(
    model.compareDocumentPosition(credential) & Node.DOCUMENT_POSITION_FOLLOWING,
  ).toBeTruthy()
  expect(screen.getByRole("button", { name: "Save connection" })).toBeDisabled()
  fireEvent.change(credential, { target: { value: "synthetic-secret" } })
  expect(screen.getByRole("button", { name: "Save connection" })).toBeEnabled()
})

it.each(["jev", "ollama", "cloudflare", "custom"] as const)(
  "tests and saves the exact %s draft without changing the active snapshot during testing",
  async (provider) => {
    await setup(provider)
    if (provider === "jev" || provider === "cloudflare")
      fireEvent.change(
        screen.getByLabelText(provider === "jev" ? "TypeSafe API key" : "Cloudflare API token"),
        { target: { value: "synthetic-secret" } },
      )
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }))
    await screen.findByText("Connection test passed.")
    expect(saved.activeId).toBe("jev")
    const test = invoke.mock.calls.find(
      ([command]) => command === "test_system_one_connection",
    )?.[1].draft
    expect(test.connection.provider).toBe(provider)
    expect(test.credential).toBe(
      provider === "jev" || provider === "cloudflare" ? "synthetic-secret" : null,
    )
    fireEvent.click(screen.getByRole("button", { name: "Save connection" }))
    await screen.findByText("Connection saved and selected for Smart Burn Checks.")
    expect(invoke).toHaveBeenCalledWith("save_system_one_connection", { draft: test })
    expect(saved.activeId).toBe(provider)
    expect(screen.queryByText("Connection test passed.")).not.toBeInTheDocument()
    expect(
      screen.getByLabelText(
        provider === "jev"
          ? "TypeSafe API key"
          : provider === "cloudflare"
            ? "Cloudflare API token"
            : "Provider credential (optional)",
      ),
    ).toHaveValue("")
  },
)

it("keeps failed drafts and active connection separate and never displays raw errors", async () => {
  await setup("custom")
  fireEvent.change(screen.getByLabelText("Response mode"), {
    target: { value: "cloudflare_envelope" },
  })
  fireEvent.change(screen.getByLabelText("Provider credential (optional)"), {
    target: { value: "private-key" },
  })
  invoke.mockRejectedValueOnce(new Error("private-key https://private.example user content"))
  fireEvent.click(screen.getByRole("button", { name: "Save connection" }))
  await screen.findByRole("alert")
  expect(screen.getByRole("alert")).toHaveTextContent("Could not save the connection")
  expect(screen.queryByText(/private-key https/)).not.toBeInTheDocument()
  expect(screen.getByLabelText("Provider credential (optional)")).toHaveValue("private-key")
  expect(screen.getByLabelText("Response mode")).toHaveValue("cloudflare_envelope")
  expect(saved.activeId).toBe("jev")
  fireEvent.change(screen.getByLabelText("Smart check provider"), {
    target: { value: "ollama" },
  })
  fireEvent.change(screen.getByLabelText("Smart check provider"), {
    target: { value: "custom" },
  })
  expect(screen.getByLabelText("Exact inference URL")).toHaveValue(
    "https://proxy.example/prefix/infer?mode=one",
  )
  expect(screen.getByLabelText("Provider credential (optional)")).toHaveValue("private-key")
})

it("invalidates test results and discovered limits on every draft edit", async () => {
  await setup("ollama")
  fireEvent.click(screen.getByRole("button", { name: "Test connection" }))
  await screen.findByText("Connection test passed.")
  fireEvent.change(screen.getByLabelText("Base URL"), {
    target: { value: "https://remote.example" },
  })
  expect(screen.queryByText("Connection test passed.")).not.toBeInTheDocument()
  invoke.mockResolvedValueOnce({
    model: "test-model",
    model_revision: "test-digest",
    total_input_tokens: { value: 8192, source: "provider_metadata" },
    state_and_longest_question_tokens: { value: null, source: "unknown" },
    runtime_context_tokens: { value: 4096, source: "runtime_metadata" },
    request_body_bytes: { value: 65536, source: "documented_default" },
    questions_per_request: { value: 64, source: "documented_default" },
    rendering_reserve_tokens: 1024,
    tokenizer: { kind: "conservative_estimator", name: "test" },
  })
  fireEvent.click(screen.getByRole("button", { name: "Model limits" }))
  fireEvent.click(screen.getByRole("button", { name: "Refresh model limits" }))
  await screen.findByText("Total input tokens: 8,192 · Provider metadata")
  expect(screen.getByText("Loaded context tokens: 4,096 · Loaded model metadata")).toBeVisible()
  expect(screen.getByText("Request body bytes: 65,536 · Documented default")).toBeVisible()
  fireEvent.change(screen.getByLabelText("Manual total input tokens"), {
    target: { value: "12345" },
  })
  expect(screen.queryByText(/Provider metadata/)).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Save connection" }))
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith(
      "save_system_one_connection",
      expect.objectContaining({
        draft: expect.objectContaining({
          connection: expect.objectContaining({
            contextOverride: {
              totalInputTokens: 12345,
              stateAndLongestQuestionTokens: null,
              runtimeContextTokens: null,
            },
          }),
        }),
      }),
    ),
  )
})

it("reconciles a provider change persisted before a failed save and retains the draft", async () => {
  await setup("custom")
  invoke.mockImplementationOnce(async (_command, args) => {
    saved = {
      activeId: "custom",
      profiles: { ...saved.profiles, custom: args.draft.connection },
    }
    throw new Error("Could not complete the connection update.")
  })
  fireEvent.click(screen.getByRole("button", { name: "Save connection" }))
  await screen.findByRole("alert")
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Use saved connection" }),
    ).not.toBeInTheDocument(),
  )
  expect(saved.activeId).toBe("custom")
  expect(screen.getByLabelText("Exact inference URL")).toHaveValue(
    "https://proxy.example/prefix/infer?mode=one",
  )
})

it("shows actionable Ollama protocol errors", async () => {
  await setup("ollama")
  invoke.mockRejectedValueOnce("Ollama must be version 0.35.0 or newer")
  fireEvent.click(screen.getByRole("button", { name: "Test connection" }))
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Ollama must be version 0.35.0 or newer",
  )
  expect(screen.getByLabelText("Provider model")).toHaveValue("test-model")
})

it("shows invalid manual bounds inline and blocks calls", async () => {
  await setup("custom")
  fireEvent.click(screen.getByRole("button", { name: "Model limits" }))
  fireEvent.change(screen.getByLabelText("Manual loaded context tokens"), {
    target: { value: "65537" },
  })
  expect(screen.getByRole("alert")).toHaveTextContent("Use whole token limits")
  expect(screen.getByRole("button", { name: "Save connection" })).toBeDisabled()
  expect(screen.getByRole("button", { name: "Test connection" })).toBeDisabled()
  fireEvent.change(screen.getByLabelText("Manual loaded context tokens"), {
    target: { value: "" },
  })
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
})

it("blocks duplicate operations and draft changes until a test completes", async () => {
  await setup("ollama")
  let finish: (() => void) | undefined
  invoke.mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve
      }),
  )
  fireEvent.click(screen.getByRole("button", { name: "Test connection" }))
  expect(screen.getByLabelText("Smart check provider")).toBeDisabled()
  expect(screen.getByLabelText("Provider model")).toBeDisabled()
  expect(screen.getByRole("button", { name: "Save connection" })).toBeDisabled()
  await act(async () => finish?.())
  await screen.findByText("Connection test passed.")
})

it("restores saved profiles without a write and activates only on request", async () => {
  saved.profiles.ollama = { ...defaultConnection("ollama"), model: "saved-model" }
  await setup("custom")
  fireEvent.change(screen.getByLabelText("Saved connections"), { target: { value: "ollama" } })
  expect(screen.getByLabelText("Provider model")).toHaveValue("saved-model")
  expect(screen.getByRole("option", { name: /Jev.*In use/ })).toBeInTheDocument()
  expect(invoke).not.toHaveBeenCalledWith("switch_system_one_connection", expect.anything())
  fireEvent.click(screen.getByRole("button", { name: "Use saved connection" }))
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("switch_system_one_connection", {
      connectionId: "ollama",
    }),
  )
})

it("removes the active credential and disables controls until the operation completes", async () => {
  saved.profiles.jev = {
    ...defaultConnection("jev"),
    credential: { kind: "connection", id: "jev" },
  }
  await setup("jev")
  fireEvent.click(screen.getByRole("button", { name: "Test connection" }))
  await screen.findByText("Connection test passed.")
  let finish: (() => void) | undefined
  invoke.mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        finish = () => {
          const connection = saved.profiles.jev
          if (!connection) throw new Error("Missing saved profile")
          saved = {
            ...saved,
            profiles: {
              ...saved.profiles,
              jev: { ...connection, credential: null, revision: connection.revision + 1 },
            },
          }
          resolve()
        }
      }),
  )
  fireEvent.click(screen.getByRole("button", { name: "Remove credential" }))
  expect(screen.queryByText("Connection test passed.")).not.toBeInTheDocument()
  expect(invoke).toHaveBeenCalledWith("remove_system_one_credential", { connectionId: "jev" })
  expect(screen.getByRole("button", { name: "Remove credential" })).toBeDisabled()
  expect(screen.getByRole("button", { name: "Test connection" })).toBeDisabled()
  expect(screen.getByRole("button", { name: "Save connection" })).toBeDisabled()
  expect(screen.getByLabelText("TypeSafe API key")).toBeDisabled()
  fireEvent.click(screen.getByRole("button", { name: "Remove credential" }))
  expect(
    invoke.mock.calls.filter(([command]) => command === "remove_system_one_credential"),
  ).toHaveLength(1)
  await act(async () => finish?.())
  await screen.findByText("Credential removed.")
  expect(screen.queryByRole("button", { name: "Remove credential" })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Save connection" })).toBeDisabled()
  expect(screen.getByLabelText("TypeSafe API key")).not.toHaveAttribute("placeholder")
  expect(saved.activeId).toBe("jev")
  expect(screen.getByLabelText("Provider model")).toHaveValue(saved.profiles.jev?.model)
  expect(invoke).not.toHaveBeenCalledWith("switch_system_one_connection", expect.anything())
})

it.each(["jev", "ollama", "cloudflare", "custom"] as const)(
  "tests and refreshes %s with a null draft credential and its explicit saved reference",
  async (provider) => {
    const connection = defaultConnection(provider)
    connection.credential = { kind: "connection", id: `saved-${provider}` }
    if (provider === "ollama" || provider === "custom") connection.model = "saved-model"
    if (connection.endpoint.kind !== "provider_default")
      connection.endpoint.value =
        provider === "cloudflare" ? "test-account" : "https://provider.example/infer"
    saved = { activeId: provider, profiles: { [provider]: connection } }
    render(<CheckProviderSettings />)
    const input = await screen.findByLabelText(
      provider === "jev"
        ? "TypeSafe API key"
        : provider === "cloudflare"
          ? "Cloudflare API token"
          : "Provider credential (optional)",
    )
    await waitFor(() => expect(input).toBeEnabled())
    expect(input).toHaveValue("")
    expect(screen.queryByText(/Re-enter saved credentials/)).not.toBeInTheDocument()
    expect(
      screen.getByText("Leave empty to use the saved credential, or enter a replacement."),
    ).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }))
    await screen.findByText("Connection test passed.")
    expect(invoke).toHaveBeenCalledWith("test_system_one_connection", {
      draft: { connection_id: provider, connection, credential: null },
    })
    fireEvent.click(screen.getByRole("button", { name: "Model limits" }))
    fireEvent.click(screen.getByRole("button", { name: "Refresh model limits" }))
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Refresh model limits" })).toBeEnabled(),
    )
    expect(invoke).toHaveBeenCalledWith("refresh_system_one_limits", {
      connection,
      credential: null,
    })
    fireEvent.change(input, { target: { value: "synthetic-replacement" } })
    fireEvent.click(screen.getByRole("button", { name: "Test connection" }))
    await screen.findByText("Connection test passed.")
    expect(invoke).toHaveBeenCalledWith("test_system_one_connection", {
      draft: { connection_id: provider, connection, credential: "synthetic-replacement" },
    })
    expect(invoke).not.toHaveBeenCalledWith("save_system_one_connection", expect.anything())
  },
)

it.each([
  [
    "save",
    "Could not restore the connection credential. Retry the connection update in Settings → Checks.",
  ],
  ["save", "Could not complete the connection update."],
  ["save", "Could not save connection settings."],
  [
    "remove",
    "Could not restore the connection credential. Retry credential removal in Settings → Checks.",
  ],
  ["remove", "Connection credential removal is incomplete. Retry it in Settings → Checks."],
  ["remove", "Could not remove the connection credential."],
  ["remove", "Could not save the credential removal state."],
  ["test", "Credential storage is unavailable."],
  ["refresh", "Connection credential removal is incomplete. Retry it in Settings → Checks."],
] as const)("shows the exact safe %s lifecycle error: %s", async (action, message) => {
  saved.profiles.jev = {
    ...defaultConnection("jev"),
    credential: { kind: "connection", id: "jev" },
  }
  await setup("jev")
  if (action === "refresh")
    fireEvent.click(screen.getByRole("button", { name: "Model limits" }))
  invoke.mockRejectedValueOnce(new Error(message))
  const button = screen.getByRole("button", {
    name:
      action === "save"
        ? "Save connection"
        : action === "remove"
          ? "Remove credential"
          : action === "refresh"
            ? "Refresh model limits"
            : "Test connection",
  })
  fireEvent.click(button)
  expect(await screen.findByRole("alert")).toHaveTextContent(message)
  await waitFor(() => expect(button).toBeEnabled())
  expect(saved.activeId).toBe("jev")
  if (action === "remove") {
    fireEvent.click(button)
    await screen.findByText("Credential removed.")
    expect(
      invoke.mock.calls.filter(([command]) => command === "remove_system_one_credential"),
    ).toHaveLength(2)
  }
})

it("keeps the active connection after a saved-profile switch fails with a safe retry error", async () => {
  saved.profiles.ollama = { ...defaultConnection("ollama"), model: "saved-model" }
  await setup("custom")
  fireEvent.change(screen.getByLabelText("Saved connections"), { target: { value: "ollama" } })
  const message = "Could not complete the connection update."
  invoke.mockRejectedValueOnce(message)
  fireEvent.click(screen.getByRole("button", { name: "Use saved connection" }))
  expect(await screen.findByRole("alert")).toHaveTextContent(message)
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Use saved connection" })).toBeEnabled(),
  )
  expect(saved.activeId).toBe("jev")
  expect(screen.getByLabelText("Provider model")).toHaveValue("saved-model")
})

it("tests a migrated Jev key using its saved legacy reference", async () => {
  const connection = {
    ...defaultConnection("jev"),
    credential: { kind: "legacy_type_safe" as const },
  }
  saved = { activeId: "jev", profiles: { jev: connection } }
  render(<CheckProviderSettings legacyKeySaved />)
  await waitFor(() => expect(screen.getByLabelText("TypeSafe API key")).toBeEnabled())
  fireEvent.click(screen.getByRole("button", { name: "Test connection" }))
  await screen.findByText("Connection test passed.")
  expect(invoke).toHaveBeenCalledWith("test_system_one_connection", {
    draft: { connection_id: "jev", connection, credential: null },
  })
  expect(screen.getByLabelText("TypeSafe API key")).toHaveValue("")
})

it.each([
  "Could not complete the connection update. private-key https://private.example",
  "Credential storage is unavailable. private-key",
  "Switch to another connection before removing this credential.",
])("uses a generic error for unapproved or obsolete text: %s", async (message) => {
  await setup("custom")
  invoke.mockRejectedValueOnce(message)
  fireEvent.click(screen.getByRole("button", { name: "Save connection" }))
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Could not save the connection. Check the settings and try again.",
  )
  expect(screen.getByRole("alert").textContent).not.toBe(message)
})

it.each(["smartCheckProvider", "smartCheckLimits", "typeSafeApiKey"])(
  "reveals and focuses searchable %s without changing values",
  async (control) => {
    const result = searchApp(
      control === "smartCheckProvider"
        ? "Use Ollama or another provider"
        : control === "smartCheckLimits"
          ? "Model limits"
          : "TypeSafe API key",
      "macos",
    ).find(
      (result) => result.target.kind === "stepSetting" && result.target.control === control,
    )
    expect(result).toBeDefined()
    if (!result || result.target.kind !== "stepSetting")
      throw new Error("Missing search destination")
    const request = resolveStepSettingsSearchTarget(result.target)
    const { container } = render(
      <CheckProviderSettings control={request.control} targetRevision={1} />,
    )
    if (!(container instanceof HTMLDivElement)) throw new Error("Missing Settings container")
    await waitFor(() => expect(screen.getByLabelText("TypeSafe API key")).toBeEnabled())
    HTMLElement.prototype.scrollIntoView = vi.fn()
    new SettingsTargetFocus().attach(container, "checks", control, 1)()
    const row = container.querySelector(`[data-settings-control="${control}"]`)
    expect(row?.contains(document.activeElement)).toBe(true)
    expect(screen.getByLabelText("TypeSafe API key")).toHaveValue("")
    expect(invoke.mock.calls.every(([command]) => command === "get_system_one_settings")).toBe(
      true,
    )
  },
)

it("keeps provider fields visible while model limits use a keyboard-focusable disclosure", async () => {
  await setup("custom")
  const disclosure = screen.getByRole("button", { name: "Model limits" })
  disclosure.focus()
  expect(disclosure).toHaveFocus()
  expect(disclosure).toHaveAttribute("aria-expanded", "false")
  expect(screen.queryByLabelText("Manual total input tokens")).not.toBeInTheDocument()
  fireEvent.click(disclosure)
  expect(disclosure).toHaveFocus()
  expect(disclosure).toHaveAttribute("aria-expanded", "true")
  expect(screen.getByLabelText("Manual total input tokens")).toBeVisible()
  fireEvent.click(disclosure)
  expect(disclosure).toHaveFocus()
  expect(screen.getByLabelText("Smart check provider")).toBeVisible()
  expect(screen.getByLabelText("Response mode").tagName).toBe("SELECT")
})

it("reopens model limits for a new search request after dismissal", async () => {
  const { rerender } = render(
    <CheckProviderSettings control="smartCheckLimits" targetRevision={1} />,
  )
  await waitFor(() => expect(screen.getByLabelText("TypeSafe API key")).toBeEnabled())
  const disclosure = screen.getByRole("button", { name: "Model limits" })
  expect(disclosure).toHaveAttribute("aria-expanded", "true")
  fireEvent.click(disclosure)
  expect(disclosure).toHaveAttribute("aria-expanded", "false")
  rerender(<CheckProviderSettings control="smartCheckLimits" targetRevision={2} />)
  expect(disclosure).toHaveAttribute("aria-expanded", "true")
  expect(screen.getByRole("button", { name: "Refresh model limits" })).toBeVisible()
  expect(invoke.mock.calls.every(([command]) => command === "get_system_one_settings")).toBe(
    true,
  )
})
