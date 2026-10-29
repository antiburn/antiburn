import { beforeEach, expect, it, vi } from "vitest"
import {
  connectionDraftChanged,
  defaultConnection,
  type PendingProviderOperation,
  type ProviderSettings,
} from "../../lib/smartCheckProviders"
import {
  ProviderSettingsSession,
  pendingOperationMessage,
  providerErrorMessage,
} from "./ProviderSettingsSession"
import { providerSettingsFixture } from "./tests/providerSettingsFixture"

const invoke = vi.hoisted(() => vi.fn())
const removeLegacyKey = vi.hoisted(() => vi.fn())
const confirmRemoval = vi.hoisted(() => vi.fn())
const unsupportedModelMessage =
  "This connection has no supported model profile or runner. Choose a supported provider and model."
vi.mock("@tauri-apps/api/core", () => ({ invoke }))
vi.mock("../../lib/checkAvailability", () => ({ removeTypeSafeApiKey: removeLegacyKey }))
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: confirmRemoval }))
let saved: ProviderSettings
beforeEach(() => {
  saved = providerSettingsFixture({
    activeId: "jev",
    profiles: {
      jev: defaultConnection("jev"),
      local: { ...defaultConnection("ollama"), model: "synthetic-local" },
    },
  })
  removeLegacyKey.mockReset().mockResolvedValue(undefined)
  confirmRemoval.mockReset().mockResolvedValue(true)
  invoke.mockReset().mockImplementation(async (command, args) => {
    if (command === "get_system_one_settings") {
      saved = providerSettingsFixture(saved)
      return saved
    }
    if (command === "test_system_one_connection") return null
    if (command === "save_system_one_connection") {
      const { draft, intent } = args
      const existing = saved.profiles[draft.connection_id]
      if (intent === "create" && existing)
        throw new Error("This connection identity was already used. Open Add connection again.")
      if (intent === "update" && !existing)
        throw new Error("This connection was removed. Close the editor and refresh Settings.")
      if (existing && existing.revision !== draft.connection.revision)
        throw new Error(
          "This connection changed in another window. Close the editor and refresh Settings.",
        )
      if (
        existing &&
        !draft.credential &&
        draft.connection.credential &&
        JSON.stringify(existing.credential) !== JSON.stringify(draft.connection.credential)
      )
        throw new Error(
          "The saved credential reference changed. Close the editor and refresh Settings.",
        )
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
              ? { kind: "connection", id: `${draft.connection_id}-key` }
              : draft.connection.credential,
          },
        },
      })
      return saved
    }
    if (command === "remove_system_one_credential") {
      const connection = saved.profiles[args.connectionId]!
      saved = {
        ...saved,
        profiles: {
          ...saved.profiles,
          [args.connectionId]: {
            ...connection,
            credential: null,
            revision: connection.revision + 1,
          },
        },
      }
      return null
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
    throw new Error(`Unexpected command: ${command}`)
  })
})

async function submitPendingSave(editor: "add" | "edit" = "edit") {
  saved.profiles.local = {
    ...saved.profiles.local!,
    credential: { kind: "connection", id: "previous-key" },
  }
  const session = new ProviderSettingsSession()
  await session.refresh()
  if (editor === "add") session.add("ollama")
  else session.openEdit("local")
  const id = session.getSnapshot().selectedId
  session.edit(
    { ...session.getSnapshot().drafts[id]!.draft.connection, model: "submitted" },
    "first-secret",
  )
  invoke.mockImplementationOnce(async (_command, { draft, requestId }) => {
    saved = {
      ...saved,
      profiles: {
        ...saved.profiles,
        [id]: {
          ...draft.connection,
          revision: 2,
          credential: { kind: "connection", id: "recovered-key" },
        },
      },
      pendingOperation: { id: "own-operation", kind: "save", connectionId: id, requestId },
    }
    throw new Error(
      "Could not complete the connection operation. Use Retry in Settings → Checks.",
    )
  })
  await session.run("save")
  return { session, id }
}

it.each([
  "The saved credential is missing. Use Edit → Change to add it again.",
  "The saved credential has an invalid format. Use Edit → Change to replace it.",
  "The system did not allow credential access. Check app access and retry.",
  "Credential access was cancelled. Use the connection again when ready.",
  "Credential storage matched more than one entry. Check the system credential store before retrying.",
  "Could not read the saved credential. Retry, or use Edit → Change to replace it.",
  unsupportedModelMessage,
  "The selected provider did not report usable model limits.",
  "Could not read model limits from the selected provider. Check the connection and try again.",
  "Could not cancel unfinished Burn Checks. Checks are paused. Retry the connection change.",
  "The connection revision limit was reached. Save a new connection.",
])("keeps the safe native error: %s", (message) =>
  expect(providerErrorMessage(new Error(message), "fallback")).toBe(message),
)

it("does not expose unapproved credential errors or diagnostic payloads", () => {
  for (const error of [
    "native error: credential-secret-sentinel account-sentinel -25308",
    { category: "platform", osStatus: -25308, secret: "credential-secret-sentinel" },
  ]) {
    expect(providerErrorMessage(error, "safe fallback")).toBe("safe fallback")
  }
})

it.each([
  { kind: "save", action: "save" },
  { kind: "switch", action: "selection" },
  { kind: "remove", action: "removal" },
] as const)(
  "explains an unfinished connection $action and how to finish it",
  ({ kind, action }) => {
    const operation: PendingProviderOperation = {
      id: "pending-operation",
      kind,
      connectionId: "jev",
      requestId: kind === "save" ? "save-request" : null,
    }
    for (const editorOpen of [false, true]) {
      expect(pendingOperationMessage(operation, editorOpen)).toBe(
        `The connection ${action} is not finished. Choose Retry to resolve it. This may restore the previous settings. Closing ${editorOpen ? "this dialog" : "Settings"} does not cancel it.`,
      )
    }
  },
)

it.each([unsupportedModelMessage, new Error(unsupportedModelMessage)])(
  "keeps the unsupported-model error when testing an Ollama connection: %s",
  async (error) => {
    const session = new ProviderSettingsSession()
    await session.refresh()
    session.openEdit("local")
    const draft = session.getSnapshot().drafts.local!.draft
    invoke.mockRejectedValueOnce(error)
    await session.run("test")
    expect(invoke).toHaveBeenLastCalledWith("test_system_one_connection", { draft })
    expect(session.getSnapshot()).toMatchObject({
      error: unsupportedModelMessage,
      errorLocation: "editor",
      busy: false,
      action: null,
      drafts: { local: { tested: false } },
    })
  },
)

it.each([
  "native error: credential-secret-sentinel account-sentinel -25308",
  new Error("native error: credential-secret-sentinel account-sentinel -25308"),
  { category: "platform", osStatus: -25308, secret: "credential-secret-sentinel" },
])("sanitizes unknown errors when testing an Ollama connection: %j", async (error) => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.openEdit("local")
  invoke.mockRejectedValueOnce(error)
  await session.run("test")
  expect(invoke.mock.calls.at(-1)?.[0]).toBe("test_system_one_connection")
  expect(session.getSnapshot().error).toBe(
    "Could not test the connection. Check the settings and try again.",
  )
})

it.each([
  providerSettingsFixture({ activeId: null, profiles: {} }),
  providerSettingsFixture({ activeId: null, profiles: { local: defaultConnection("ollama") } }),
] satisfies ProviderSettings[])("accepts a confirmed null selection: %j", async (settings) => {
  saved = settings
  const notify = vi.fn()
  const session = new ProviderSettingsSession(notify)
  await session.refresh()
  expect(session.getSnapshot().saved).toEqual(saved)
  expect(session.getSnapshot().activeConfirmed).toBe(true)
  expect(session.getSnapshot().error).toBeNull()
  expect(notify).toHaveBeenCalledWith(false)
})

it("rejects a broken non-null selection and leaves recovery read-only", async () => {
  saved.activeId = "missing"
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.add()
  expect(session.getSnapshot().saved).toBeNull()
  expect(session.getSnapshot().activeConfirmed).toBe(false)
  expect(session.getSnapshot().editor).toBeNull()
})

it.each([
  { activeId: "jev", profiles: { jev: defaultConnection("jev") }, displayNumbers: { jev: 1 } },
  { activeId: "jev", profiles: { jev: defaultConnection("jev") }, pendingOperation: null },
  {
    activeId: "jev",
    profiles: { jev: defaultConnection("jev") },
    pendingOperation: null,
    displayNumbers: {},
  },
])(
  "rejects missing native recovery or label metadata instead of inventing it: %j",
  async (settings) => {
    invoke.mockResolvedValueOnce(settings)
    const session = new ProviderSettingsSession()
    await session.refresh()
    expect(session.getSnapshot().saved).toBeNull()
    expect(session.getSnapshot().activeConfirmed).toBe(false)
  },
)

it("refreshes without losing dirty drafts or rebasing stale revisions", async () => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.edit(defaultConnection("jev"), "keep-secret")
  saved = {
    ...saved,
    activeId: "local",
    profiles: {
      ...saved.profiles,
      jev: { ...saved.profiles.jev!, revision: 2 },
      local: { ...saved.profiles.local!, model: "updated-model" },
    },
  }
  await session.refresh()
  expect(session.getSnapshot().drafts.jev!.draft.credential).toBe("keep-secret")
  expect(session.getSnapshot().drafts.jev!.draft.connection.revision).toBe(1)
  expect(session.getSnapshot().drafts.local!.draft.connection.model).toBe("updated-model")
  expect(session.getSnapshot().saved?.activeId).toBe("local")
  expect(invoke.mock.calls.map(([command]) => command)).toEqual([
    "get_system_one_settings",
    "get_system_one_settings",
  ])
})

it("does not send no-op switches and opens dirty connections before Use", async () => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  await session.run("switch")
  session.select("local")
  session.edit({ ...saved.profiles.local!, model: "draft" }, null)
  await session.useConnection("local")
  expect(session.getSnapshot().editor).toBe("edit")
  expect(session.getSnapshot().status).toBe(
    "Save or discard your changes before using this connection.",
  )
  expect(invoke.mock.calls.map(([command]) => command)).toEqual(["get_system_one_settings"])
})

it("saves an inactive edit without switching and preserves null selection when other profiles exist", async () => {
  saved.activeId = null
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.openEdit("local")
  session.edit({ ...saved.profiles.local!, model: "updated" }, null)
  await session.run("save")
  expect(saved.activeId).toBeNull()
  expect(session.getSnapshot().editor).toBeNull()
  expect(invoke).toHaveBeenCalledWith("save_system_one_connection", {
    draft: {
      connection_id: "local",
      connection: { ...defaultConnection("ollama"), model: "updated" },
      credential: null,
    },
    intent: "update",
    requestId: expect.any(String),
  })
  session.add()
  const id = session.getSnapshot().selectedId
  session.edit(defaultConnection("jev"), "synthetic-secret")
  await session.run("save")
  expect(saved.activeId).toBeNull()
  expect(saved.profiles[id]).toBeDefined()
})

it("creates fresh identities, tests without activation, and selects only the first save", async () => {
  saved = providerSettingsFixture({ activeId: null, profiles: {} })
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.add()
  const firstId = session.getSnapshot().selectedId
  expect(firstId).toMatch(/^jev-1-[a-f0-9-]{36}$/)
  await session.closeEditor()
  session.add()
  const id = session.getSnapshot().selectedId
  expect(id).not.toBe(firstId)
  session.edit(defaultConnection("jev"), "synthetic-secret")
  await session.run("test")
  expect(session.getSnapshot().drafts[id]!.tested).toBe(true)
  expect(saved.activeId).toBeNull()
  await session.run("save")
  expect(saved.activeId).toBe(id)
  expect(session.getSnapshot().drafts[id]!.draft.credential).toBeNull()
  expect(invoke.mock.calls.map(([command]) => command)).toEqual([
    "get_system_one_settings",
    "test_system_one_connection",
    "save_system_one_connection",
  ])
})

it("does not ask to discard untouched Add or Edit and clears only unsaved drafts after a confirmed discard", async () => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.add()
  await session.closeEditor()
  session.openEdit()
  await session.closeEditor()
  expect(confirmRemoval).not.toHaveBeenCalled()
  session.add()
  const id = session.getSnapshot().selectedId
  session.edit(defaultConnection("jev"), "secret")
  confirmRemoval.mockResolvedValueOnce(false)
  await session.closeEditor()
  expect(session.getSnapshot().drafts[id]!.draft.credential).toBe("secret")
  await session.closeEditor()
  expect(session.getSnapshot().drafts[id]).toBeUndefined()
})

it("keeps the Add identity when a provider change arrives during reload", async () => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.add()
  const id = session.getSnapshot().selectedId
  session.edit(defaultConnection("jev"), "keep-secret")
  const draft = session.getSnapshot().drafts[id]
  let finish!: (settings: ProviderSettings) => void
  invoke.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const loading = session.refresh()
  await session.changeAddProvider("ollama")
  expect(session.getSnapshot()).toMatchObject({ selectedId: id, editor: "add" })
  expect(session.getSnapshot().drafts[id]).toBe(draft)
  expect(confirmRemoval).not.toHaveBeenCalled()
  finish(saved)
  await loading
  await session.run("save")
  expect(invoke).toHaveBeenLastCalledWith("save_system_one_connection", {
    draft: draft!.draft,
    intent: "create",
    requestId: expect.any(String),
  })
})

it.each(["reload", "another draft", "later edit", "another editor"] as const)(
  "ignores a stale provider discard after %s",
  async (change) => {
    const session = new ProviderSettingsSession()
    await session.refresh()
    session.add()
    const id = session.getSnapshot().selectedId
    session.edit(defaultConnection("jev"), "keep-secret")
    let confirm!: (discard: boolean) => void
    confirmRemoval.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          confirm = resolve
        }),
    )
    const changing = session.changeAddProvider("ollama")
    let finishRead!: (settings: ProviderSettings) => void
    let loading: Promise<void> | undefined
    if (change === "reload") {
      invoke.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finishRead = resolve
          }),
      )
      loading = session.refresh()
    } else if (change === "another draft") session.revealTarget("cloudflareModel", 1)
    else if (change === "later edit") session.edit(defaultConnection("jev"), "later-secret")
    else session.openEdit("local")
    const before = session.getSnapshot()
    confirm(true)
    await changing
    expect(session.getSnapshot()).toBe(before)
    expect(session.getSnapshot().drafts[id]).toBe(before.drafts[id])
    if (loading) {
      finishRead(saved)
      await loading
    }
  },
)

it.each(["saved selection", "saved Add identity"] as const)(
  "never sends an Add as an update after a %s",
  async (change) => {
    const session = new ProviderSettingsSession()
    await session.refresh()
    session.add()
    const id = session.getSnapshot().selectedId
    if (change === "saved selection") session.select("jev")
    else {
      saved = providerSettingsFixture({
        ...saved,
        profiles: { ...saved.profiles, [id]: defaultConnection("jev") },
      })
      await session.refresh()
    }
    const calls = invoke.mock.calls.length
    await session.run("save")
    expect(invoke).toHaveBeenCalledTimes(calls)
    expect(session.getSnapshot().editor).toBe("add")
    expect(session.getSnapshot().error).toContain("Open Add connection again")
  },
)

it.each(["local", "jev"])(
  "removes %s, prunes drafts and remembered search choices, and never chooses a replacement",
  async (id) => {
    const session = new ProviderSettingsSession()
    await session.refresh()
    session.select(id)
    await session.removeConnection(id)
    expect(session.getSnapshot().drafts[id]).toBeUndefined()
    expect(session.getSnapshot().saved?.activeId).toBe(id === "jev" ? null : "jev")
    expect(invoke).toHaveBeenCalledWith("remove_system_one_connection", {
      connectionId: id,
      revision: 1,
    })
    session.revealTarget(id === "jev" ? "typeSafeApiKey" : "ollamaModel", 1)
    expect(session.getSnapshot().selectedId).not.toBe(id)
  },
)

it("removes the last profile while retaining legacy credentials and reports an empty confirmed state", async () => {
  saved = providerSettingsFixture({
    activeId: "jev",
    profiles: {
      jev: { ...defaultConnection("jev"), credential: { kind: "legacy_type_safe" } },
    },
  })
  const session = new ProviderSettingsSession()
  await session.refresh()
  await session.removeConnection("jev")
  expect(session.getSnapshot().saved).toMatchObject({
    activeId: null,
    profiles: {},
    pendingOperation: null,
  })
  expect(session.getSnapshot().activeConfirmed).toBe(true)
  expect(removeLegacyKey).not.toHaveBeenCalled()
  session.add()
  expect(
    session.getSnapshot().drafts[session.getSnapshot().selectedId]!.draft.connection.credential,
  ).toBeNull()
  session.useLegacyKey()
  expect(
    session.getSnapshot().drafts[session.getSnapshot().selectedId]!.draft.connection.credential,
  ).toEqual({ kind: "legacy_type_safe" })
})

it("does not resurrect a deleted stale editor and retains a concurrent edit conflict", async () => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.openEdit("local")
  session.edit({ ...saved.profiles.local!, model: "keep-draft" }, "keep-secret")
  saved = {
    ...saved,
    profiles: {
      ...saved.profiles,
      local: { ...saved.profiles.local!, model: "other-window", revision: 2 },
    },
  }
  await session.run("save")
  expect(session.getSnapshot().error).toBe(
    "This connection changed in another window. Close the editor and refresh Settings.",
  )
  expect(session.getSnapshot().drafts.local!.draft.connection.revision).toBe(1)
  expect(session.getSnapshot().drafts.local!.draft.credential).toBe("keep-secret")
  saved = providerSettingsFixture({ activeId: "jev", profiles: { jev: saved.profiles.jev! } })
  await session.run("save")
  expect(saved.profiles.local).toBeUndefined()
  expect(invoke.mock.calls.at(-2)?.[1].intent).toBe("update")
  session.revealTarget("ollamaModel", 1)
  expect(session.getSnapshot().editor).toBe("edit")
  await session.run("save")
  expect(invoke.mock.calls.at(-2)?.[1].intent).toBe("update")
  session.add("ollama")
  expect(session.getSnapshot().selectedId).not.toBe("local")
})

it.each([
  "Could not finish credential cleanup. Use Retry in Settings → Checks.",
  "Could not complete the connection operation. Use Retry in Settings → Checks.",
])("keeps revision and secret until an owned Save retry completes: %s", async (message) => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.openEdit("local")
  session.edit({ ...saved.profiles.local!, model: "updated" }, "keep-secret")
  invoke.mockImplementationOnce(async (_command, { draft, requestId }) => {
    saved = {
      ...saved,
      profiles: { ...saved.profiles, local: { ...draft.connection, revision: 2 } },
      pendingOperation: {
        id: "save-operation",
        kind: "save",
        connectionId: "local",
        requestId,
      },
    }
    throw new Error(message)
  })
  await session.run("save")
  expect(session.getSnapshot().drafts.local!.draft.credential).toBe("keep-secret")
  expect(session.getSnapshot().drafts.local!.draft.connection.revision).toBe(1)
  expect(session.getSnapshot().status).toBeNull()
  expect(session.getSnapshot().saved?.activeId).toBe("jev")
  expect(session.getSnapshot().error).toBe(message)
  const beforeRetry = invoke.mock.calls.length
  await session.run("save")
  await session.useConnection("local")
  await session.removeConnection("local")
  expect(invoke).toHaveBeenCalledTimes(beforeRetry)
  invoke.mockImplementationOnce(async (_command, args) => {
    expect(args).toEqual({ operationId: "save-operation" })
    saved = { ...saved, pendingOperation: null }
    return { settings: saved, outcome: "completed" }
  })
  await session.retryPendingOperation()
  expect(session.getSnapshot().drafts.local!.draft.credential).toBeNull()
  expect(session.getSnapshot().drafts.local!.draft.connection.revision).toBe(2)
  expect(session.getSnapshot().editor).toBeNull()
  expect(session.getSnapshot().status).toBe("Connection saved.")
})

it.each([false, true])(
  "keeps an Add draft after explicit rollback and requires a new Save, with later edits: %s",
  async (changed) => {
    const session = new ProviderSettingsSession()
    await session.refresh()
    session.add()
    const id = session.getSnapshot().selectedId
    session.edit(defaultConnection("jev"), "keep-secret")
    invoke.mockImplementationOnce(async (_command, { requestId }) => {
      saved = {
        ...saved,
        pendingOperation: { id: "create-operation", kind: "save", connectionId: id, requestId },
      }
      throw new Error(
        "Could not complete the connection operation. Use Retry in Settings → Checks.",
      )
    })
    await session.run("save")
    expect(session.getSnapshot().editor).toBe("add")
    expect(session.getSnapshot().drafts[id]!.draft.credential).toBe("keep-secret")
    if (changed)
      session.edit({ ...defaultConnection("jev"), modelRevision: "later" }, "later-secret")
    const retained = session.getSnapshot().drafts[id]!.draft
    invoke.mockImplementationOnce(async () => {
      saved = { ...saved, pendingOperation: null }
      return { settings: saved, outcome: "rolled_back" }
    })
    await session.retryPendingOperation()
    expect(session.getSnapshot().drafts[id]!.draft).toEqual(retained)
    expect(session.getSnapshot().editor).toBe("add")
    expect(session.getSnapshot().status).toContain("has not been saved")
    await session.run("save")
    expect(invoke.mock.calls.at(-1)?.[1].intent).toBe("create")
    expect(session.getSnapshot().editor).toBeNull()
    expect(saved.activeId).toBe("jev")
  },
)

it.each(["other-window", null])(
  "does not accept another window's Save with the same fields and request identity %s",
  async (requestIdentity) => {
    const session = new ProviderSettingsSession()
    await session.refresh()
    session.openEdit("local")
    session.edit({ ...saved.profiles.local!, model: "same-model" }, "keep-secret")
    invoke.mockImplementationOnce(async (_command, { draft, requestId }) => {
      saved = {
        ...saved,
        profiles: { ...saved.profiles, local: { ...draft.connection, revision: 2 } },
        pendingOperation: {
          id: "other-operation",
          kind: "save",
          connectionId: "local",
          requestId: requestIdentity ? `${requestId}-${requestIdentity}` : null,
        },
      }
      throw new Error(
        "Could not complete the connection operation. Use Retry in Settings → Checks.",
      )
    })
    await session.run("save")
    invoke.mockImplementationOnce(async () => {
      saved = { ...saved, pendingOperation: null }
      return { settings: saved, outcome: "completed" }
    })
    await session.retryPendingOperation()
    expect(session.getSnapshot().drafts.local!.draft.credential).toBe("keep-secret")
    expect(session.getSnapshot().drafts.local!.draft.connection.revision).toBe(1)
    expect(session.getSnapshot().editor).toBe("edit")
    expect(session.getSnapshot().status).not.toBe("Connection saved.")
    await session.run("save")
    expect(session.getSnapshot().error).toContain("changed in another window")
    expect(saved.profiles.local!.revision).toBe(2)
  },
)

it.each([
  { editor: "edit", credential: "second-secret" },
  { editor: "edit", credential: null },
  { editor: "add", credential: "second-secret" },
  { editor: "add", credential: null },
] as const)(
  "saves a changed $editor draft after owned recovery with replacement credential $credential",
  async ({ editor, credential }) => {
    const { session, id } = await submitPendingSave(editor)
    session.edit(
      { ...session.getSnapshot().drafts[id]!.draft.connection, model: "changed-again" },
      credential,
    )
    const callsBeforeRetry = invoke.mock.calls.length
    invoke.mockImplementationOnce(async (command, args) => {
      expect(command).toBe("retry_system_one_connection_operation")
      expect(args).toEqual({ operationId: "own-operation" })
      saved = { ...saved, pendingOperation: null }
      return { settings: saved, outcome: "completed" }
    })
    expect(await session.retryPendingOperation()).toBe(true)
    const current = session.getSnapshot().drafts[id]!
    expect(current.draft.credential).toBe(credential)
    expect.soft(current.draft.connection).toMatchObject({
      model: "changed-again",
      revision: 2,
      credential: { kind: "connection", id: "recovered-key" },
    })
    expect(connectionDraftChanged(current.draft, saved.profiles[id])).toBe(true)
    expect(current.tested).toBe(false)
    expect.soft(session.getSnapshot().editor).toBe("edit")
    expect(session.getSnapshot().status).not.toBe("Connection saved.")
    expect(saved.profiles[id]!.model).toBe("submitted")
    expect(saved.activeId).toBe("jev")
    expect(invoke).toHaveBeenCalledTimes(callsBeforeRetry + 1)
    await session.run("save")
    expect(session.getSnapshot().error).toBeNull()
    expect(invoke.mock.calls.at(-1)).toEqual([
      "save_system_one_connection",
      { draft: current.draft, intent: "update", requestId: expect.any(String) },
    ])
    expect(saved.profiles[id]!.model).toBe("changed-again")
    expect(saved.profiles[id]!.revision).toBe(3)
    expect(saved.activeId).toBe("jev")
    expect(session.getSnapshot().drafts[id]!.draft.credential).toBeNull()
    expect(session.getSnapshot().editor).toBeNull()
  },
)

it("closes an unchanged owned Add only after its Retry completes", async () => {
  const { session, id } = await submitPendingSave("add")
  invoke.mockImplementationOnce(async () => {
    saved = { ...saved, pendingOperation: null }
    return { settings: saved, outcome: "completed" }
  })
  await session.retryPendingOperation()
  expect(session.getSnapshot().editor).toBeNull()
  expect(session.getSnapshot().drafts[id]!.draft.credential).toBeNull()
  expect(
    connectionDraftChanged(session.getSnapshot().drafts[id]!.draft, saved.profiles[id]),
  ).toBe(false)
  expect(session.getSnapshot().status).toBe("Connection saved.")
})

it.each(["revision", "credential", "provider", "missing row"] as const)(
  "does not rebase an owned recovery after a changed %s",
  async (changed) => {
    const { session, id } = await submitPendingSave()
    const connection = session.getSnapshot().drafts[id]!.draft.connection
    session.edit(
      {
        ...connection,
        model: "later-model",
        ...(changed === "revision" ? { revision: 7 } : {}),
        ...(changed === "credential"
          ? { credential: { kind: "connection", id: "later-key" } as const }
          : {}),
        ...(changed === "provider" ? ({ provider: "custom" } as const) : {}),
      },
      "later-secret",
    )
    const retained = session.getSnapshot().drafts[id]!
    invoke.mockImplementationOnce(async () => {
      const profiles = { ...saved.profiles }
      if (changed === "missing row") delete profiles[id]
      saved = { ...saved, profiles, pendingOperation: null }
      return { settings: saved, outcome: "completed" }
    })
    expect(await session.retryPendingOperation()).toBe(true)
    expect(session.getSnapshot().drafts[id]).toEqual(retained)
    expect(session.getSnapshot().editor).toBe("edit")
    expect(session.getSnapshot().status).not.toBe("Connection saved.")
  },
)

it("does not rebase an owned Save from a stale operation's readback", async () => {
  const { session, id } = await submitPendingSave()
  session.edit(
    { ...session.getSnapshot().drafts[id]!.draft.connection, model: "later-model" },
    "later-secret",
  )
  const retained = session.getSnapshot().drafts[id]!
  invoke.mockImplementationOnce(async (command, args) => {
    expect(command).toBe("retry_system_one_connection_operation")
    expect(args).toEqual({ operationId: "own-operation" })
    saved = { ...saved, pendingOperation: null }
    throw new Error("This recovery operation is no longer pending. Refresh Settings.")
  })
  expect(await session.retryPendingOperation()).toBe(false)
  expect(session.getSnapshot().drafts[id]).toEqual(retained)
  expect(session.getSnapshot().error).toContain("no longer pending")
  expect(session.getSnapshot().status).toBeNull()
  await session.run("save")
  expect(session.getSnapshot().error).toContain("changed in another window")
})

it.each(["closed", "another editor"] as const)(
  "keeps the later %s choice when an owned Add recovery completes",
  async (choice) => {
    const { session, id } = await submitPendingSave("add")
    session.edit(
      { ...session.getSnapshot().drafts[id]!.draft.connection, model: "later-model" },
      "later-secret",
    )
    if (choice === "closed") await session.closeEditor()
    else {
      session.openEdit("jev")
      session.edit({ ...saved.profiles.jev!, modelRevision: "later" }, "other-secret")
    }
    const selectedId = session.getSnapshot().selectedId
    const otherDraft = session.getSnapshot().drafts.jev!
    invoke.mockImplementationOnce(async () => {
      saved = { ...saved, activeId: "local", pendingOperation: null }
      return { settings: saved, outcome: "completed" }
    })
    await session.retryPendingOperation()
    expect(session.getSnapshot().selectedId).toBe(selectedId)
    expect(session.getSnapshot().editor).toBe(choice === "closed" ? null : "edit")
    expect(session.getSnapshot().drafts.jev).toEqual(otherDraft)
    expect(session.getSnapshot().drafts[id]!.draft).toMatchObject({
      credential: "later-secret",
      connection: { model: "later-model", revision: 2 },
    })
    expect(session.getSnapshot().saved?.activeId).toBe("local")
    expect(invoke.mock.calls.map(([command]) => command)).toEqual([
      "get_system_one_settings",
      "save_system_one_connection",
      "get_system_one_settings",
      "retry_system_one_connection_operation",
    ])
    session.openEdit(id)
    await session.run("save")
    expect(session.getSnapshot().error).toBeNull()
    expect(invoke.mock.calls.at(-1)?.[1].intent).toBe("update")
    expect(saved.activeId).toBe("local")
  },
)

it("keeps the removal token after the row disappears and across Settings remount", async () => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  invoke.mockImplementationOnce(async () => {
    saved = {
      ...saved,
      activeId: null,
      profiles: {},
      pendingOperation: {
        id: "remove-operation",
        kind: "remove",
        connectionId: "jev",
        requestId: null,
      },
    }
    throw new Error("Could not finish credential cleanup. Use Retry in Settings → Checks.")
  })
  await session.removeConnection("jev")
  expect(session.getSnapshot().saved?.profiles.jev).toBeUndefined()
  const remounted = new ProviderSettingsSession()
  await remounted.refresh()
  invoke.mockRejectedValueOnce(
    new Error("Could not complete the connection operation. Use Retry in Settings → Checks."),
  )
  await remounted.retryPendingOperation()
  expect(remounted.getSnapshot().saved?.pendingOperation?.id).toBe("remove-operation")
  invoke.mockImplementationOnce(async () => {
    saved = { ...saved, pendingOperation: null }
    return { settings: saved, outcome: "completed" }
  })
  await remounted.retryPendingOperation()
  expect(
    invoke.mock.calls
      .filter(([command]) => command === "retry_system_one_connection_operation")
      .map(([, args]) => args),
  ).toEqual([{ operationId: "remove-operation" }, { operationId: "remove-operation" }])
})

it("clears busy without claiming success after a stale token and a null-pending readback", async () => {
  saved.pendingOperation = {
    id: "stale-operation",
    kind: "switch",
    connectionId: "local",
    requestId: null,
  }
  const session = new ProviderSettingsSession()
  await session.refresh()
  invoke.mockImplementationOnce(async () => {
    saved = { ...saved, pendingOperation: null }
    throw new Error("This recovery operation is no longer pending. Refresh Settings.")
  })
  expect(await session.retryPendingOperation()).toBe(false)
  expect(session.getSnapshot().saved?.pendingOperation).toBeNull()
  expect(session.getSnapshot().busy).toBe(false)
  expect(session.getSnapshot().status).toBeNull()
  expect(session.getSnapshot().error).toContain("no longer pending")
})

it("offers explicit recovery for Use even after selection metadata changed", async () => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  invoke.mockImplementationOnce(async () => {
    saved = {
      ...saved,
      activeId: "local",
      pendingOperation: {
        id: "switch-operation",
        kind: "switch",
        connectionId: "local",
        requestId: null,
      },
    }
    throw new Error(
      "Could not complete the connection operation. Use Retry in Settings → Checks.",
    )
  })
  await session.useConnection("local")
  expect(session.getSnapshot().saved?.activeId).toBe("local")
  invoke.mockImplementationOnce(async () => {
    saved = { ...saved, pendingOperation: null }
    return { settings: saved, outcome: "completed" }
  })
  expect(await session.retryPendingOperation()).toBe(true)
  expect(invoke.mock.calls.at(-1)).toEqual([
    "retry_system_one_connection_operation",
    { operationId: "switch-operation" },
  ])
})

it("preserves an unsaved create draft on failed readback and does not claim confirmed selection", async () => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.add()
  const id = session.getSnapshot().selectedId
  session.edit(defaultConnection("jev"), "keep-secret")
  invoke
    .mockRejectedValueOnce(new Error("write failed"))
    .mockRejectedValueOnce(new Error("read failed"))
  await session.run("save")
  expect(session.getSnapshot().activeConfirmed).toBe(false)
  expect(session.getSnapshot().drafts[id]!.draft.credential).toBe("keep-secret")
  expect(session.getSnapshot().status).toContain("Could not confirm")
  expect(session.getSnapshot().busy).toBe(false)
})

it.each(["ollama", "custom"] as const)(
  "forgets %s credentials without dropping field edits or other drafts",
  async (provider) => {
    saved.profiles.local = {
      ...defaultConnection(provider),
      model: "synthetic-model",
      credential: { kind: "connection", id: "local-key" },
      revision: 7,
    }
    const session = new ProviderSettingsSession()
    await session.refresh()
    session.edit(defaultConnection("jev"), "keep-jev-secret")
    session.select("local")
    session.edit({ ...saved.profiles.local, model: "draft-model" }, "replacement")
    await session.run("remove")
    const state = session.getSnapshot()
    expect(state.saved?.activeId).toBe("jev")
    expect(state.drafts.local!.draft).toMatchObject({
      credential: "replacement",
      connection: { model: "draft-model", revision: 8, credential: null },
    })
    expect(state.drafts.jev!.draft.credential).toBe("keep-jev-secret")
    expect(connectionDraftChanged(state.drafts.local!.draft, saved.profiles.local)).toBe(true)
  },
)

it("keeps shared-key removal behind confirmation and makes retained-key forgetting reachable with no profiles", async () => {
  saved = providerSettingsFixture({ activeId: null, profiles: {} })
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.add()
  confirmRemoval.mockResolvedValueOnce(false)
  await session.forgetLegacyKey()
  expect(removeLegacyKey).not.toHaveBeenCalled()
  await session.forgetLegacyKey()
  expect(removeLegacyKey).toHaveBeenCalledOnce()
  expect(session.getSnapshot().legacyKeyForgotten).toBe(true)
})

it.each(
  (["add", "edit"] as const).flatMap((editor) =>
    [0, 1, 2].map((failedReads) => ({ editor, failedReads })),
  ),
)(
  "keeps each dirty draft savable after shared-key removal from $editor with $failedReads failed reads",
  async ({ editor, failedReads }) => {
    const legacy = {
      ...defaultConnection("jev"),
      credential: { kind: "legacy_type_safe" as const },
    }
    saved = providerSettingsFixture({
      ...saved,
      profiles: { ...saved.profiles, jev: legacy, work: legacy },
    })
    removeLegacyKey.mockImplementation(async () => {
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
    })
    const session = new ProviderSettingsSession()
    await session.refresh()
    for (const id of ["jev", "work"]) {
      session.openEdit(id)
      session.edit({ ...legacy, modelRevision: `draft-${id}` }, `secret-${id}`)
    }
    session.openEdit("local")
    session.edit({ ...saved.profiles.local!, model: "keep-local-model" }, "keep-local-secret")
    const local = session.getSnapshot().drafts.local
    if (editor === "add") {
      session.add()
      session.edit(defaultConnection("jev"), "new-connection-secret")
    } else session.openEdit("work")
    const id = session.getSnapshot().selectedId
    const current = session.getSnapshot().drafts[id]!
    for (let index = 0; index < failedReads; index++)
      invoke.mockRejectedValueOnce(new Error("settings read failed"))
    if (editor === "add") await session.forgetLegacyKey()
    else await session.run("remove")
    if (failedReads === 2) {
      expect(session.getSnapshot().activeConfirmed).toBe(false)
      expect(session.getSnapshot().drafts.jev!.draft.connection.revision).toBe(1)
      await session.refresh()
    }
    for (const legacyId of ["jev", "work"]) {
      expect(session.getSnapshot().drafts[legacyId]).toMatchObject({
        draft: {
          credential: `secret-${legacyId}`,
          connection: { ...legacy, revision: 2, modelRevision: `draft-${legacyId}` },
        },
        tested: false,
      })
    }
    expect(session.getSnapshot().drafts.local).toBe(local)
    if (editor === "add") {
      expect(session.getSnapshot().drafts[id]).toBe(current)
      await session.run("save")
      expect(invoke.mock.calls.at(-1)?.[1].intent).toBe("create")
    }
    session.openEdit("jev")
    await session.run("save")
    expect(session.getSnapshot().error).toBeNull()
    expect(invoke.mock.calls.at(-1)?.[1]).toMatchObject({
      intent: "update",
      draft: { connection_id: "jev", connection: { revision: 2 }, credential: "secret-jev" },
    })
  },
)

it.each(["add", "edit"] as const)(
  "does not rebase an external change during shared-key removal from %s",
  async (editor) => {
    const legacy = {
      ...defaultConnection("jev"),
      credential: { kind: "legacy_type_safe" as const },
    }
    saved = providerSettingsFixture({
      ...saved,
      profiles: { ...saved.profiles, jev: legacy, work: legacy },
    })
    const session = new ProviderSettingsSession()
    await session.refresh()
    for (const id of ["jev", "work"]) {
      session.openEdit(id)
      session.edit(legacy, `secret-${id}`)
    }
    if (editor === "add") session.add()
    else session.openEdit("jev")
    removeLegacyKey.mockImplementation(async () => {
      saved = {
        ...saved,
        profiles: {
          ...saved.profiles,
          jev: { ...legacy, revision: 3, modelRevision: "external-change" },
          work: { ...legacy, revision: 2 },
        },
      }
    })
    if (editor === "add") await session.forgetLegacyKey()
    else await session.run("remove")
    expect(session.getSnapshot().drafts.jev!.draft).toMatchObject({
      credential: "secret-jev",
      connection: { revision: 1, modelRevision: null },
    })
    expect(session.getSnapshot().drafts.work!.draft.connection.revision).toBe(2)
    session.openEdit("jev")
    await session.run("save")
    expect(session.getSnapshot().error).toContain("changed in another window")
  },
)

it("retries the read instead of removing a shared key again after successful removal", async () => {
  const legacy = {
    ...defaultConnection("jev"),
    credential: { kind: "legacy_type_safe" as const },
  }
  saved = providerSettingsFixture({
    ...saved,
    activeId: "local",
    profiles: { ...saved.profiles, jev: legacy },
  })
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.openEdit("jev")
  session.edit(legacy, "keep-secret")
  removeLegacyKey.mockImplementation(async () => {
    saved = {
      ...saved,
      profiles: {
        ...saved.profiles,
        jev: { ...saved.profiles.jev!, revision: saved.profiles.jev!.revision + 1 },
      },
    }
  })
  invoke
    .mockRejectedValueOnce(new Error("read failed"))
    .mockRejectedValueOnce(new Error("read failed"))
  await session.run("remove")
  expect(session.getSnapshot().activeConfirmed).toBe(false)
  await session.run("remove")
  await session.forgetLegacyKey()
  expect(removeLegacyKey).toHaveBeenCalledOnce()
  await session.refresh()
  expect(session.getSnapshot().drafts.jev!.draft).toMatchObject({
    credential: "keep-secret",
    connection: { revision: 2 },
  })
})

it.each(["earlier external revision", "changed credential", "failed removal"] as const)(
  "keeps a dirty revision after shared-key removal with %s",
  async (change) => {
    const legacy = {
      ...defaultConnection("jev"),
      credential: { kind: "legacy_type_safe" as const },
    }
    saved = providerSettingsFixture({ ...saved, profiles: { ...saved.profiles, jev: legacy } })
    const session = new ProviderSettingsSession()
    await session.refresh()
    session.openEdit("jev")
    session.edit(
      change === "changed credential"
        ? { ...legacy, credential: { kind: "connection", id: "other-key" } }
        : legacy,
      "keep-secret",
    )
    if (change === "earlier external revision") {
      saved = { ...saved, profiles: { ...saved.profiles, jev: { ...legacy, revision: 2 } } }
      await session.refresh()
    }
    session.add()
    removeLegacyKey.mockImplementation(async () => {
      if (change === "failed removal") throw new Error("credential removal failed")
      saved = {
        ...saved,
        profiles: {
          ...saved.profiles,
          jev: { ...saved.profiles.jev!, revision: saved.profiles.jev!.revision + 1 },
        },
      }
    })
    await session.forgetLegacyKey()
    expect(session.getSnapshot().drafts.jev!.draft.connection.revision).toBe(1)
    expect(session.getSnapshot().drafts.jev!.draft.credential).toBe("keep-secret")
  },
)

it("defers a non-first search field while busy without extra native calls", async () => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.openEdit("jev")
  let finish!: () => void
  invoke.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = () => resolve(saved)
      }),
  )
  const saving = session.run("save")
  session.revealTarget("ollamaModel", 1)
  expect(session.getSnapshot().selectedId).toBe("jev")
  finish()
  await saving
  expect(session.getSnapshot().selectedId).toBe("local")
  expect(invoke.mock.calls.map(([command]) => command)).toEqual([
    "get_system_one_settings",
    "save_system_one_connection",
  ])
})

it("keeps a dirty editor and its secret when a non-editor search arrives", async () => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.openEdit("jev")
  session.edit({ ...saved.profiles.jev!, modelRevision: "draft" }, "keep-secret")
  const before = session.getSnapshot()
  session.revealTarget("checkDirectories", 1)
  session.revealTarget("checkModelUsage", 2)
  expect(session.getSnapshot()).toEqual(before)
  expect(confirmRemoval).not.toHaveBeenCalled()
  expect(invoke.mock.calls.map(([command]) => command)).toEqual(["get_system_one_settings"])
})

it("lets the latest non-editor search cancel a deferred provider target during Save", async () => {
  const session = new ProviderSettingsSession()
  await session.refresh()
  session.openEdit("jev")
  session.edit(saved.profiles.jev!, "keep-secret")
  let finish!: () => void
  invoke.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = () => resolve(saved)
      }),
  )
  const saving = session.run("save")
  session.revealTarget("ollamaModel", 1)
  session.revealTarget("checkDirectories", 2)
  expect(session.getSnapshot().editor).toBe("edit")
  expect(session.getSnapshot().drafts.jev!.draft.credential).toBe("keep-secret")
  finish()
  await saving
  expect(session.getSnapshot().editor).toBeNull()
  expect(session.getSnapshot().selectedId).toBe("jev")
  expect(confirmRemoval).not.toHaveBeenCalled()
  expect(invoke.mock.calls.map(([command]) => command)).toEqual([
    "get_system_one_settings",
    "save_system_one_connection",
  ])
})

it("does not discard pending Save recovery or later secrets for a non-editor search", async () => {
  const { session, id } = await submitPendingSave()
  session.edit(
    { ...session.getSnapshot().drafts[id]!.draft.connection, model: "later-model" },
    "later-secret",
  )
  const before = session.getSnapshot()
  session.revealTarget("checkDirectories", 1)
  expect(session.getSnapshot()).toEqual(before)
  expect(confirmRemoval).not.toHaveBeenCalled()
  await session.closeEditor()
  expect(session.getSnapshot().saved?.pendingOperation?.id).toBe("own-operation")
  expect(session.getSnapshot().drafts[id]!.draft.credential).toBe("later-secret")
})

it("ignores a late saved-state read after a newer read", async () => {
  const session = new ProviderSettingsSession()
  let finish!: (settings: ProviderSettings) => void
  invoke.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const first = session.refresh()
  invoke.mockResolvedValueOnce({ ...saved, activeId: "local" })
  await session.refresh()
  finish(saved)
  await first
  expect(session.getSnapshot().saved?.activeId).toBe("local")
})
