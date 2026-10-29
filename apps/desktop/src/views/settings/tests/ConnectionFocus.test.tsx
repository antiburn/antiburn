import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import { defaultConnection, type ProviderSettings } from "../../../lib/smartCheckProviders"
import { CheckProviderSettings } from "../CheckProviderSettings"
import { ConnectionActions } from "../ConnectionActions"
import { ConnectionDialog } from "../ConnectionDialog"
import { SettingsTargetFocus } from "../SettingsTargetFocus"
import { providerSettingsFixture } from "./providerSettingsFixture"

const invoke = vi.hoisted(() => vi.fn())
const confirmDiscard = vi.hoisted(() => vi.fn())
vi.mock("@tauri-apps/api/core", () => ({ invoke }))
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: confirmDiscard }))

let saved: ProviderSettings

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: Error) => void
  const promise = new Promise<T>((complete, fail) => {
    resolve = complete
    reject = fail
  })
  return { promise, resolve, reject }
}

async function nextFrame() {
  await act(async () => {
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
  })
}

async function renderSettings() {
  render(
    <>
      <CheckProviderSettings />
      <button type="button">Check files</button>
    </>,
  )
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "More actions for Jev 2" })).toBeEnabled(),
  )
  return screen.getByRole("button", { name: "Check files" })
}

function searchSettings(focus: SettingsTargetFocus, control?: string, revision = 0) {
  return (
    <div ref={(node) => (node ? focus.attach(node, "checks", control, revision) : undefined)}>
      <CheckProviderSettings control={control} targetRevision={revision} />
      <div data-settings-control="checkDirectories" tabIndex={-1}>
        <button type="button">Check files</button>
      </div>
      <div data-settings-control="checkModelUsage" tabIndex={-1}>
        <button type="button">Model usage</button>
      </div>
    </div>
  )
}

async function editWorkConnection(value: string | null = "keep-secret") {
  const trigger = await screen.findByRole("button", { name: "More actions for Jev 2" })
  await waitFor(() => expect(trigger).toBeEnabled())
  trigger.focus()
  fireEvent.keyDown(trigger, { key: "Enter" })
  fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }))
  const dialog = await screen.findByRole("dialog", { name: "Edit Jev 2 connection" })
  const change = within(dialog).queryByRole("button", { name: "Change" })
  if (change) fireEvent.click(change)
  const credential = within(dialog).getByLabelText("Jev API key")
  if (value !== null) fireEvent.change(credential, { target: { value } })
  credential.focus()
  return { dialog, credential }
}

beforeEach(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn()
  HTMLDialogElement.prototype.showModal = function () {
    this.open = true
  }
  HTMLDialogElement.prototype.close = function () {
    this.open = false
  }
  const profile = {
    ...defaultConnection("jev"),
    credential: { kind: "connection" as const, id: "synthetic-key" },
  }
  saved = providerSettingsFixture({
    activeId: "personal",
    profiles: { personal: profile, work: profile },
  })
  confirmDiscard.mockReset().mockResolvedValue(true)
  invoke.mockReset().mockImplementation(async (command) => {
    if (command === "get_system_one_settings") return saved
    throw new Error(`Unexpected command: ${command}`)
  })
})

it("defers the latest outside search while a dirty modal keeps its secret", async () => {
  const focus = new SettingsTargetFocus()
  const view = render(searchSettings(focus))
  const { dialog, credential } = await editWorkConnection()
  view.rerender(searchSettings(focus, "checkDirectories", 1))
  expect(credential).toHaveFocus()
  view.rerender(searchSettings(focus, "checkModelUsage", 2))
  expect(dialog).toHaveAttribute("open")
  expect(credential).toHaveValue("keep-secret")
  expect(credential).toHaveFocus()
  expect(confirmDiscard).not.toHaveBeenCalled()
  confirmDiscard.mockResolvedValueOnce(false)
  fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }))
  await nextFrame()
  expect(dialog).toHaveAttribute("open")
  expect(credential).toHaveValue("keep-secret")
  invoke.mockImplementationOnce(async (command, { draft }) => {
    expect(command).toBe("save_system_one_connection")
    expect(draft.credential).toBe("keep-secret")
    saved = {
      ...saved,
      profiles: { ...saved.profiles, work: { ...draft.connection, revision: 2 } },
    }
    return saved
  })
  fireEvent.click(within(dialog).getByRole("button", { name: "Save" }))
  const usage = screen.getByRole("button", { name: "Model usage" })
  await waitFor(() => expect(usage).toHaveFocus())
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  const files = screen.getByRole("button", { name: "Check files" })
  files.focus()
  view.rerender(searchSettings(focus, "checkModelUsage", 2))
  await nextFrame()
  expect(files).toHaveFocus()
  expect(confirmDiscard).toHaveBeenCalledOnce()
  expect(invoke.mock.calls.map(([command]) => command)).toEqual([
    "get_system_one_settings",
    "save_system_one_connection",
  ])
})

it("defers search through busy Save and keeps pending recovery and later secrets", async () => {
  const focus = new SettingsTargetFocus()
  const view = render(searchSettings(focus))
  const { dialog, credential } = await editWorkConnection()
  const operation = deferred<ProviderSettings>()
  let requestId: string | undefined
  invoke.mockImplementationOnce((command, args) => {
    expect(command).toBe("save_system_one_connection")
    requestId = args.requestId
    return operation.promise
  })
  fireEvent.click(within(dialog).getByRole("button", { name: "Save" }))
  view.rerender(searchSettings(focus, "checkDirectories", 1))
  view.rerender(searchSettings(focus, "checkModelUsage", 2))
  fireEvent(dialog, new Event("cancel", { cancelable: true }))
  expect(dialog).toHaveAttribute("open")
  expect(credential).toHaveValue("keep-secret")
  expect(credential).toBeDisabled()
  expect(confirmDiscard).not.toHaveBeenCalled()
  saved = {
    ...saved,
    pendingOperation: {
      id: "pending-save",
      kind: "save",
      connectionId: "work",
      requestId: requestId!,
    },
  }
  await act(async () =>
    operation.reject(
      new Error("Could not complete the connection operation. Use Retry in Settings → Checks."),
    ),
  )
  await waitFor(() => expect(credential).toBeEnabled())
  expect(dialog).toHaveAttribute("open")
  expect(credential).toHaveValue("keep-secret")
  expect(within(dialog).getByRole("button", { name: "Save" })).toBeDisabled()
  fireEvent.change(credential, { target: { value: "later-secret" } })
  fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }))
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  await waitFor(() => expect(screen.getByRole("button", { name: "Model usage" })).toHaveFocus())
  expect(screen.getByRole("button", { name: "Retry connection change" })).toBeEnabled()
  const reopened = await editWorkConnection(null)
  expect(reopened.credential).toHaveValue("later-secret")
  expect(saved.pendingOperation?.requestId).toBe(requestId)
  expect(confirmDiscard).not.toHaveBeenCalled()
  expect(invoke.mock.calls.map(([command]) => command)).toEqual([
    "get_system_one_settings",
    "save_system_one_connection",
    "get_system_one_settings",
  ])
})

it("keeps a row menu open through the full pointer click before Edit", async () => {
  await renderSettings()
  const trigger = screen.getByRole("button", { name: "More actions for Jev 2" })
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false })
  await screen.findByRole("menu")
  fireEvent.pointerUp(trigger, { button: 0, ctrlKey: false })
  fireEvent.click(trigger)
  await nextFrame()
  expect(trigger).toHaveAttribute("aria-expanded", "true")
  expect(screen.getByRole("menu")).toHaveAttribute("data-state", "open")
  fireEvent.click(screen.getByRole("menuitem", { name: "Edit" }))
  const dialog = await screen.findByRole("dialog", { name: "Edit Jev 2 connection" })
  expect(screen.queryByRole("menu")).not.toBeInTheDocument()
  await nextFrame()
  expect(within(dialog).getByRole("button", { name: "Change" })).toHaveFocus()
  fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }))
  await waitFor(() => expect(trigger).toHaveFocus())
})

it("keeps a new row menu focused when delayed dialog cleanup runs after its opener is enabled", async () => {
  const frames: FrameRequestCallback[] = []
  const requestFrame = vi
    .spyOn(window, "requestAnimationFrame")
    .mockImplementation((callback) => frames.push(callback))
  const onEdit = vi.fn()
  const onRemove = vi.fn()
  function panel(open: boolean, disabled = false) {
    return (
      <>
        <ConnectionActions
          id="connection-actions"
          label="Jev"
          disabled={disabled}
          pending={false}
          onEdit={onEdit}
          onRemove={onRemove}
        />
        {open && (
          <ConnectionDialog title="Edit Jev connection" onCancel={vi.fn()}>
            <button type="button">Cancel</button>
          </ConnectionDialog>
        )}
      </>
    )
  }
  try {
    const { rerender } = render(panel(false))
    const trigger = screen.getByRole("button", { name: "More actions for Jev" })
    trigger.focus()
    rerender(panel(true))
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus()
    rerender(panel(true, true))
    expect(trigger).toBeDisabled()
    rerender(panel(false, true))
    expect(trigger).toBeDisabled()
    expect(frames).toHaveLength(1)
    const restoreFocus = frames[0]!
    rerender(panel(false))
    trigger.focus()
    fireEvent.keyDown(trigger, { key: "Enter" })
    const edit = await screen.findByRole("menuitem", { name: "Edit" })
    act(() => edit.focus())
    expect(trigger).toHaveAttribute("aria-expanded", "true")
    expect(edit).toHaveFocus()
    act(() => restoreFocus(0))
    expect(edit).toHaveFocus()
    expect(trigger).toHaveAttribute("aria-expanded", "true")
    expect(screen.getByRole("menu")).toHaveAttribute("data-state", "open")
    expect(onEdit).not.toHaveBeenCalled()
    expect(onRemove).not.toHaveBeenCalled()
  } finally {
    requestFrame.mockRestore()
  }
})

it.each(["success", "pending failure"])(
  "preserves a later focus choice when Use ends with %s",
  async (outcome) => {
    const files = await renderSettings()
    const operation = deferred<ProviderSettings>()
    invoke.mockImplementationOnce((command) => {
      expect(command).toBe("switch_system_one_connection")
      return operation.promise
    })
    fireEvent.click(screen.getByRole("button", { name: "Use Jev 2" }))
    files.focus()
    await act(async () => {
      if (outcome === "success") {
        saved = { ...saved, activeId: "work" }
        operation.resolve(saved)
      } else {
        saved = {
          ...saved,
          pendingOperation: {
            id: "pending-switch",
            kind: "switch",
            connectionId: "work",
            requestId: null,
          },
        }
        operation.reject(
          new Error(
            "Could not complete the connection operation. Use Retry in Settings → Checks.",
          ),
        )
      }
    })
    if (outcome === "success")
      expect(screen.getByRole("listitem", { name: "Jev 2" })).toHaveAccessibleDescription(
        "Selected",
      )
    else expect(screen.getByRole("button", { name: "Retry connection change" })).toBeEnabled()
    await nextFrame()
    expect(files).toHaveFocus()
  },
)

it.each(["success", "failure", "pending failure"])(
  "preserves a later focus choice when Remove ends with %s",
  async (outcome) => {
    const files = await renderSettings()
    const operation = deferred<ProviderSettings>()
    invoke.mockImplementationOnce((command) => {
      expect(command).toBe("remove_system_one_connection")
      return operation.promise
    })
    fireEvent.keyDown(screen.getByRole("button", { name: "More actions for Jev 2" }), {
      key: "Enter",
    })
    fireEvent.click(await screen.findByRole("menuitem", { name: "Remove" }))
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("remove_system_one_connection", {
        connectionId: "work",
        revision: 1,
      }),
    )
    files.focus()
    await act(async () => {
      if (outcome === "success") {
        saved = { ...saved, profiles: { personal: saved.profiles.personal! } }
        operation.resolve(saved)
      } else {
        if (outcome === "pending failure")
          saved = {
            ...saved,
            pendingOperation: {
              id: "pending-remove",
              kind: "remove",
              connectionId: "work",
              requestId: null,
            },
          }
        operation.reject(
          new Error("Could not remove the connection. Reload connections and try again."),
        )
      }
    })
    if (outcome === "success")
      expect(screen.queryByRole("listitem", { name: "Jev 2" })).not.toBeInTheDocument()
    else if (outcome === "pending failure")
      expect(screen.getByRole("button", { name: "Retry connection change" })).toBeEnabled()
    else expect(screen.getByRole("alert")).toBeVisible()
    await nextFrame()
    expect(files).toHaveFocus()
  },
)

it.each(["completed", "failure"])(
  "preserves a later focus choice when Retry ends with %s",
  async (outcome) => {
    saved = {
      ...saved,
      pendingOperation: {
        id: "pending-switch",
        kind: "switch",
        connectionId: "work",
        requestId: null,
      },
    }
    const files = await renderSettings()
    const operation = deferred<{ settings: ProviderSettings; outcome: "completed" }>()
    invoke.mockImplementationOnce((command) => {
      expect(command).toBe("retry_system_one_connection_operation")
      return operation.promise
    })
    fireEvent.click(screen.getByRole("button", { name: "Retry connection change" }))
    files.focus()
    await act(async () => {
      if (outcome === "completed") {
        saved = { ...saved, pendingOperation: null }
        operation.resolve({ settings: saved, outcome: "completed" })
      } else
        operation.reject(
          new Error(
            "Could not complete the connection operation. Use Retry in Settings → Checks.",
          ),
        )
    })
    if (outcome === "completed")
      expect(
        screen.queryByRole("button", { name: "Retry connection change" }),
      ).not.toBeInTheDocument()
    else expect(screen.getByRole("button", { name: "Retry connection change" })).toBeEnabled()
    await nextFrame()
    expect(files).toHaveFocus()
  },
)
