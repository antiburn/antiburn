import { invoke } from "@tauri-apps/api/core"

export const SMART_CHECK_PROVIDERS = {
  jev: { label: "Jev (recommended)", endpointLabel: null, model: "jev-latest" },
  ollama: { label: "Ollama", endpointLabel: "Base URL", model: "" },
  cloudflare: { label: "Cloudflare", endpointLabel: "Account ID", model: "clef" },
  custom: { label: "Custom", endpointLabel: "Exact inference URL", model: "" },
} as const
export type SmartCheckProvider = keyof typeof SMART_CHECK_PROVIDERS
export type Connection = {
  provider: SmartCheckProvider
  endpoint:
    | { kind: "provider_default" }
    | { kind: "base_url" | "exact_url" | "cloudflare_account"; value: string }
  model: string
  modelRevision: string | null
  responseMode: "direct" | "cloudflare_envelope"
  credential: { kind: "legacy_type_safe" } | { kind: "connection"; id: string } | null
  revision: number
  contextOverride: {
    totalInputTokens: number | null
    stateAndLongestQuestionTokens: number | null
    runtimeContextTokens: number | null
  } | null
}
export type PendingProviderOperation = {
  id: string
  kind: "save" | "switch" | "remove"
  connectionId: string
  requestId: string | null
}
export type ProviderSettings = {
  activeId: string | null
  profiles: Record<string, Connection>
  pendingOperation: PendingProviderOperation | null
  displayNumbers: Record<string, number>
}
export type ProviderOperationResult = {
  settings: ProviderSettings
  outcome: "completed" | "rolled_back"
}
export type ConnectionDraft = {
  connection_id: string
  connection: Connection
  credential: string | null
}
export type InstalledOllamaModel = { id: string; label: string }

export function connectionDraftChanged(draft: ConnectionDraft, saved?: Connection): boolean {
  return (
    Boolean(draft.credential?.trim()) ||
    JSON.stringify(draft.connection) !== JSON.stringify(saved)
  )
}

export function connectionLabel(
  id: string,
  connection: Connection,
  settings: ProviderSettings,
): string {
  const provider = connection.provider
  const number = settings.displayNumbers[id]
  if (!Number.isSafeInteger(number) || !number || number < 1)
    throw new Error("Saved connection display numbers are invalid.")
  const name = provider === "jev" ? "Jev" : SMART_CHECK_PROVIDERS[provider].label
  const numbered = number > 1 ? `${name} ${number}` : name
  return provider === "jev" ? numbered : `${numbered} · ${modelLabel(connection.model)}`
}

export function newConnectionId(provider: SmartCheckProvider, ids: string[]): string {
  const number = ids.length + 1
  return `${provider}-${number}-${crypto.randomUUID()}`
}

export function modelLabel(model: string): string {
  if (model === "clef") return "Clef"
  if (model === "clef-flash") return "Clef Flash"
  if (model.startsWith("jev-")) return "Jev"
  return model.replace(/[-_]/g, " ").replace(/^./, (letter) => letter.toUpperCase())
}

export function contextLimitError(connection: Connection): string | null {
  if (connection.provider !== "custom") return null
  if (connection.endpoint.kind !== "exact_url") return "Enter an exact inference URL."
  const endpoint = connection.endpoint.value
  if (
    connection.responseMode === "direct" &&
    endpoint === "https://api.typesafe.ai/v1/systemone" &&
    ["jev-1.13.0", "jev-1.14.0"].includes(connection.model)
  )
    return null
  if (
    connection.responseMode === "cloudflare_envelope" &&
    ["clef", "clef-flash"].includes(connection.model) &&
    new RegExp(
      `^https://api\\.cloudflare\\.com/client/v4/accounts/[A-Za-z0-9_-]{1,64}/ai/run/@cf/cloudflare/${connection.model}$`,
    ).test(endpoint)
  )
    return null
  return "This custom endpoint has no supported model profile. Choose Jev, Cloudflare, or an installed local Ollama model."
}

export function defaultConnection(provider: SmartCheckProvider): Connection {
  return {
    provider,
    endpoint:
      provider === "jev"
        ? { kind: "provider_default" }
        : {
            kind:
              provider === "ollama"
                ? "base_url"
                : provider === "cloudflare"
                  ? "cloudflare_account"
                  : "exact_url",
            value: provider === "ollama" ? "http://localhost:11434" : "",
          },
    model: SMART_CHECK_PROVIDERS[provider].model,
    modelRevision: null,
    responseMode: provider === "cloudflare" ? "cloudflare_envelope" : "direct",
    credential: null,
    revision: 1,
    contextOverride: null,
  }
}

export const getProviderSettings = () => invoke<ProviderSettings>("get_system_one_settings")
export const getInstalledOllamaModels = (
  draft: ConnectionDraft,
): Promise<InstalledOllamaModel[]> =>
  invoke<InstalledOllamaModel[]>("list_ollama_models", { draft })
export const testProviderConnection = (draft: ConnectionDraft) =>
  invoke<void>("test_system_one_connection", { draft })
export const saveProviderConnection = (
  draft: ConnectionDraft,
  intent: "create" | "update",
  requestId: string,
) => invoke<ProviderSettings>("save_system_one_connection", { draft, intent, requestId })
export const removeProviderConnection = (connectionId: string, revision: number) =>
  invoke<ProviderSettings>("remove_system_one_connection", { connectionId, revision })
export const retryProviderOperation = (operationId: string) =>
  invoke<ProviderOperationResult>("retry_system_one_connection_operation", { operationId })
export const switchProviderConnection = (connectionId: string) =>
  invoke<ProviderSettings>("switch_system_one_connection", { connectionId })
export const removeProviderCredential = (connectionId: string) =>
  invoke<void>("remove_system_one_credential", { connectionId })
