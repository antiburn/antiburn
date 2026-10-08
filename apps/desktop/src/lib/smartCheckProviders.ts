import { invoke } from "@tauri-apps/api/core"

export const SMART_CHECK_PROVIDERS = {
  jev: { label: "Jev (recommended)", endpointLabel: null, model: "jev-1.13.0" },
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
export type ProviderSettings = { activeId: string; profiles: Record<string, Connection> }
export type ConnectionDraft = {
  connection_id: string
  connection: Connection
  credential: string | null
}
export type CapabilitySource =
  "provider_metadata" | "runtime_metadata" | "documented_default" | "manual" | "unknown"
type CapabilityLimit = { value: number | null; source: CapabilitySource }
export type ModelCapabilities = {
  total_input_tokens: CapabilityLimit
  state_and_longest_question_tokens: CapabilityLimit
  runtime_context_tokens: CapabilityLimit
  request_body_bytes: CapabilityLimit
  questions_per_request: CapabilityLimit
  rendering_reserve_tokens: number
  tokenizer: { kind: "exact" | "conservative_estimator"; name: string } | null
  model: string
  model_revision: string | null
}

export function contextLimitError(connection: Connection): string | null {
  const limits = connection.contextOverride
  if (
    limits &&
    Object.values(limits).some(
      (value) => value !== null && (!Number.isInteger(value) || value < 1 || value > 65536),
    )
  )
    return "Use whole token limits from 1 to 65,536, or leave them empty."

  const reserve = connection.provider === "ollama" ? 1024 : 4096
  const inputBounds = [limits?.totalInputTokens, limits?.runtimeContextTokens].filter(
    (value): value is number => value != null,
  )
  if (connection.provider !== "custom")
    inputBounds.push(connection.provider === "ollama" ? 8192 : 65536)
  if (!inputBounds.length)
    return "Custom requires manual total input tokens or loaded context tokens above 4,096. Open Model limits to enter a bound."
  if (
    Math.min(...inputBounds) <= reserve ||
    (limits?.stateAndLongestQuestionTokens != null &&
      limits.stateAndLongestQuestionTokens <= reserve)
  )
    return `Each token limit must exceed the ${reserve.toLocaleString("en-US")}-token rendering reserve.`
  return null
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
export const testProviderConnection = (draft: ConnectionDraft) =>
  invoke<void>("test_system_one_connection", { draft })
export const saveProviderConnection = (draft: ConnectionDraft) =>
  invoke<ProviderSettings>("save_system_one_connection", { draft })
export const switchProviderConnection = (connectionId: string) =>
  invoke<ProviderSettings>("switch_system_one_connection", { connectionId })
export const refreshProviderLimits = (draft: ConnectionDraft) =>
  invoke<ModelCapabilities>("refresh_system_one_limits", {
    connection: draft.connection,
    credential: draft.credential,
  })
export const removeProviderCredential = (connectionId: string) =>
  invoke<void>("remove_system_one_credential", { connectionId })
