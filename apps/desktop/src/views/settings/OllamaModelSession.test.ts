import { beforeEach, expect, it, vi } from "vitest"
import {
  defaultConnection,
  type ConnectionDraft,
  type InstalledOllamaModel,
} from "../../lib/smartCheckProviders"
import { OllamaModelSession } from "./OllamaModelSession"

const getModels = vi.hoisted(() => vi.fn())
vi.mock("@tauri-apps/api/core", () => ({ invoke: getModels }))
const models: InstalledOllamaModel[] = [
  { id: "local-one", label: "Local one" },
  { id: "local-two", label: "Local two" },
]
let draft: ConnectionDraft
beforeEach(() => {
  draft = {
    connection_id: "local",
    connection: {
      ...defaultConnection("ollama"),
      model: "saved-model",
      credential: { kind: "connection", id: "saved-key" },
    },
    credential: "replacement-secret",
  }
  getModels.mockReset().mockResolvedValue(models)
})

function setup() {
  const session = new OllamaModelSession(() => draft)
  session.bind()
  const stop = session.subscribe(() => undefined)
  return { session, stop }
}

it("reads only after an explicit find and leaves the exact draft unchanged", async () => {
  const { session } = setup()
  const original = structuredClone(draft)
  expect(getModels).not.toHaveBeenCalled()
  await session.find()
  expect(getModels).toHaveBeenCalledExactlyOnceWith("list_ollama_models", { draft })
  expect(session.getSnapshot()).toEqual({ models, loading: false, error: null })
  expect(draft).toEqual(original)
})

it("suppresses duplicate reads and keeps prior choices after a safe lookup failure", async () => {
  const { session } = setup()
  await session.find()
  let reject!: (error: Error) => void
  getModels.mockImplementationOnce(
    () =>
      new Promise((_, fail) => {
        reject = fail
      }),
  )
  const loading = session.find()
  await session.find()
  expect(getModels).toHaveBeenCalledTimes(2)
  expect(session.getSnapshot().models).toEqual(models)
  expect(session.getSnapshot().loading).toBe(true)
  reject(new Error("private provider response replacement-secret"))
  await loading
  expect(session.getSnapshot().models).toEqual(models)
  expect(session.getSnapshot().error).toBe(
    "Could not find installed models. Check the server URL and authentication, then try again.",
  )
  expect(draft.connection.model).toBe("saved-model")
})

it.each(["server", "credential", "saved-credential", "model"])(
  "ignores an old response after a %s edit and keeps the newer request",
  async (field) => {
    const { session } = setup()
    let resolveOld!: (models: InstalledOllamaModel[]) => void
    getModels.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolveOld = done
        }),
    )
    const oldRead = session.find()
    draft = {
      ...draft,
      credential: field === "credential" ? "new-secret" : draft.credential,
      connection: {
        ...draft.connection,
        ...(field === "server"
          ? { endpoint: { kind: "base_url", value: "http://localhost:22434" } }
          : field === "saved-credential"
            ? { credential: { kind: "connection", id: "new-saved-key" } }
            : field === "model"
              ? { model: "other-local" }
              : {}),
      },
    }
    session.bind()
    await session.find()
    expect(session.getSnapshot().models).toEqual(models)
    resolveOld([{ id: "stale", label: "Stale" }])
    await oldRead
    expect(session.getSnapshot().models).toEqual(models)
    expect(session.getSnapshot().loading).toBe(false)
  },
)

it("checks the current draft before accepting a response, even before the next render binds it", async () => {
  const { session } = setup()
  let resolve!: (models: InstalledOllamaModel[]) => void
  getModels.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done
      }),
  )
  const read = session.find()
  draft = { ...draft, credential: "new-secret" }
  resolve(models)
  await read
  expect(session.getSnapshot().models).toBeNull()
  session.bind()
  expect(session.getSnapshot().loading).toBe(false)
})

it("ignores an older lookup error after a newer read succeeds", async () => {
  const { session } = setup()
  let reject!: (error: Error) => void
  getModels.mockImplementationOnce(
    () =>
      new Promise((_, fail) => {
        reject = fail
      }),
  )
  const oldRead = session.find()
  draft = { ...draft, credential: "new-secret" }
  session.bind()
  await session.find()
  reject(new Error("Ollama is unavailable"))
  await oldRead
  expect(session.getSnapshot()).toEqual({ models, loading: false, error: null })
})

it("clears old server choices on connection edits but retains them on explicit model selection", async () => {
  const { session } = setup()
  await session.find()
  draft = { ...draft, connection: { ...draft.connection, model: "local-two" } }
  session.bind()
  expect(session.getSnapshot().models).toEqual(models)
  draft = {
    ...draft,
    connection: {
      ...draft.connection,
      endpoint: { kind: "base_url", value: "http://localhost:22434" },
    },
  }
  session.bind()
  expect(session.getSnapshot().models).toBeNull()
  expect(session.getSnapshot().error).toBeNull()
})

it("ignores results after unsubscribe and does not reuse them after reattachment", async () => {
  const { session, stop } = setup()
  let resolve!: (models: InstalledOllamaModel[]) => void
  getModels.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done
      }),
  )
  const read = session.find()
  stop()
  resolve(models)
  await read
  session.bind()
  const cleanup = session.subscribe(() => undefined)
  expect(session.getSnapshot()).toEqual({ models: null, loading: false, error: null })
  expect(getModels).toHaveBeenCalledOnce()
  cleanup()
})

it("treats an empty list as a successful read without clearing the selected model", async () => {
  getModels.mockResolvedValueOnce([])
  const { session } = setup()
  await session.find()
  expect(session.getSnapshot()).toEqual({ models: [], loading: false, error: null })
  expect(draft.connection.model).toBe("saved-model")
})
