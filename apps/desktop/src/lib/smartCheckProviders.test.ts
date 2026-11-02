import { invoke } from "@tauri-apps/api/core"
import { expect, it, vi } from "vitest"
import {
  contextLimitError,
  defaultConnection,
  getInstalledOllamaModels,
  connectionLabel,
  newConnectionId,
  saveProviderConnection,
  removeProviderConnection,
  retryProviderOperation,
  type Connection,
} from "./smartCheckProviders"
import { providerSettingsFixture } from "../views/settings/tests/providerSettingsFixture"

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }))

it("uses fresh bounded identities and stable readable duplicate numbers", () => {
  const id = newConnectionId("cloudflare", ["cloudflare", "cloudflare-2-old"])
  expect(id).toMatch(/^cloudflare-3-[a-f0-9-]{36}$/)
  expect(id.length).toBeLessThanOrEqual(64)
  const connection = defaultConnection("cloudflare")
  expect(
    connectionLabel(
      id,
      connection,
      providerSettingsFixture({
        activeId: null,
        profiles: { [id]: connection },
        displayNumbers: { [id]: 3 },
      }),
    ),
  ).toBe("Cloudflare 3 · Clef")
  expect(
    connectionLabel(
      "old-internal-id",
      defaultConnection("jev"),
      providerSettingsFixture({
        activeId: null,
        profiles: { "old-internal-id": defaultConnection("jev") },
      }),
    ),
  ).toBe("Jev")
})

it("passes create/update intent and the exact removal revision to native commands", async () => {
  const draft = {
    connection_id: "local",
    connection: defaultConnection("ollama"),
    credential: null,
  }
  vi.mocked(invoke)
    .mockReset()
    .mockResolvedValue(providerSettingsFixture({ activeId: null, profiles: {} }))
  await saveProviderConnection(draft, "create", "create-request")
  await saveProviderConnection(draft, "update", "update-request")
  await removeProviderConnection("local", 7)
  await retryProviderOperation("operation-token")
  expect(vi.mocked(invoke).mock.calls).toEqual([
    ["save_system_one_connection", { draft, intent: "create", requestId: "create-request" }],
    ["save_system_one_connection", { draft, intent: "update", requestId: "update-request" }],
    ["remove_system_one_connection", { connectionId: "local", revision: 7 }],
    ["retry_system_one_connection_operation", { operationId: "operation-token" }],
  ])
})

it("keeps legacy labels readable and distinct beside numbered connection IDs", () => {
  const connection = defaultConnection("jev")
  const profiles = { work: connection, jev: connection, "jev-2-old": connection }
  const settings = providerSettingsFixture({
    activeId: null,
    profiles,
    displayNumbers: { work: 3, jev: 1, "jev-2-old": 2 },
  })
  expect(Object.keys(profiles).map((id) => connectionLabel(id, connection, settings))).toEqual([
    "Jev 3",
    "Jev",
    "Jev 2",
  ])
  expect(newConnectionId("jev", Object.keys(profiles))).toMatch(/^jev-4-/)
})

it("uses native display numbers for colliding UUID ordinals and keeps them after removal", () => {
  const connection = defaultConnection("jev")
  const first = "jev-2-first-uuid"
  const second = "jev-2-second-uuid"
  const settings = providerSettingsFixture({
    activeId: null,
    profiles: { [first]: connection, [second]: connection },
    displayNumbers: { [first]: 2, [second]: 3 },
  })
  expect(connectionLabel(first, connection, settings)).toBe("Jev 2")
  expect(connectionLabel(second, connection, settings)).toBe("Jev 3")
  delete settings.profiles[first]
  expect(connectionLabel(second, connection, settings)).toBe("Jev 3")
  delete settings.displayNumbers[second]
  expect(() => connectionLabel(second, connection, settings)).toThrow("display numbers")
})

it("uses the native Jev stable alias without a release pin or manual limits", () => {
  expect(defaultConnection("jev")).toMatchObject({
    model: "jev-latest",
    modelRevision: null,
    contextOverride: null,
  })
})

it.each(["jev", "ollama", "cloudflare"] as const)(
  "ignores legacy manual limits for the managed %s connection",
  (provider) => {
    const connection = defaultConnection(provider)
    expect(contextLimitError(connection)).toBeNull()
    expect(
      contextLimitError({
        ...connection,
        contextOverride: {
          totalInputTokens: 1,
          stateAndLongestQuestionTokens: 1,
          runtimeContextTokens: 1,
        },
      }),
    ).toBeNull()
  },
)

it.each([
  null,
  { totalInputTokens: 8192, stateAndLongestQuestionTokens: null, runtimeContextTokens: null },
])("blocks unknown Custom metadata even with a saved manual bound: %j", (contextOverride) => {
  const connection: Connection = {
    ...defaultConnection("custom"),
    endpoint: { kind: "exact_url", value: "https://gateway.example/infer?route=local" },
    model: "jev-latest",
    contextOverride,
  }
  expect(contextLimitError(connection)).toContain("no supported model profile")
  expect(contextLimitError(connection)).not.toContain("Model limits")
  expect(connection.endpoint).toEqual({
    kind: "exact_url",
    value: "https://gateway.example/infer?route=local",
  })
})

it.each([
  ["https://api.typesafe.ai/v1/systemone", "jev-1.13.0", "direct"],
  ["https://api.typesafe.ai/v1/systemone", "jev-1.14.0", "direct"],
  [
    "https://api.cloudflare.com/client/v4/accounts/account/ai/run/@cf/cloudflare/clef",
    "clef",
    "cloudflare_envelope",
  ],
  [
    "https://api.cloudflare.com/client/v4/accounts/account/ai/run/@cf/cloudflare/clef-flash",
    "clef-flash",
    "cloudflare_envelope",
  ],
] as const)("accepts the supported exact route %s", (value, model, responseMode) => {
  const connection: Connection = {
    ...defaultConnection("custom"),
    endpoint: { kind: "exact_url", value },
    model,
    responseMode,
  }
  expect(contextLimitError(connection)).toBeNull()
  for (const suffix of ["/", "?route=other"]) {
    expect(
      contextLimitError({
        ...connection,
        endpoint: { kind: "exact_url", value: value + suffix },
      }),
    ).not.toBeNull()
  }
  expect(
    contextLimitError({
      ...connection,
      responseMode: responseMode === "direct" ? "cloudflare_envelope" : "direct",
    }),
  ).not.toBeNull()
})

it("keeps the selected installed Ollama model explicit", () => {
  expect(defaultConnection("ollama").model).toBe("")
})

it("looks up installed Ollama models through the exact draft IPC contract without selecting one", async () => {
  const draft = {
    connection_id: "new-local",
    connection: defaultConnection("ollama"),
    credential: null,
  }
  const models = [{ id: "clef:9b", label: "clef:9b" }]
  vi.mocked(invoke).mockReset().mockResolvedValueOnce(models)
  expect(invoke).not.toHaveBeenCalled()
  await expect(getInstalledOllamaModels(draft)).resolves.toEqual(models)
  expect(invoke).toHaveBeenCalledExactlyOnceWith("list_ollama_models", { draft })
  expect(draft.connection.model).toBe("")
})

it("keeps model lookup failures separate from an empty installed list", async () => {
  const draft = {
    connection_id: "local",
    connection: defaultConnection("ollama"),
    credential: null,
  }
  vi.mocked(invoke)
    .mockReset()
    .mockRejectedValueOnce(new Error("Ollama rejected the credential"))
  await expect(getInstalledOllamaModels(draft)).rejects.toThrow(
    "Ollama rejected the credential",
  )
  expect(invoke).toHaveBeenCalledTimes(1)
  expect(draft.connection.model).toBe("")
})
