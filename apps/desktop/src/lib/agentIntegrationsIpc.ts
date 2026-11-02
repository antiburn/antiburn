import { invoke, isTauri } from "@tauri-apps/api/core"
import { listen } from "./tauriEvents"

export type IntegrationVendor = "claude" | "codex" | "cursor" | "open_code" | "pi"
export type IntegrationPhase =
  | "not_detected"
  | "unavailable"
  | "pending_native_gate"
  | "registered"
  | "awaiting_trust"
  | "reload_required"
  | "active"
  | "user_changed"
  | "disabled"

export interface IntegrationStatus {
  vendor: IntegrationVendor
  phase: IntegrationPhase
  reason: string
  hostVersion: string | null
  profile: string | null
  nodeVersion: string | null
  owned: boolean
}

export async function getAgentIntegrations(): Promise<IntegrationStatus[]> {
  if (!isTauri()) return []
  return invoke("get_agent_integrations")
}

export async function repairAgentIntegration(
  vendor: IntegrationVendor,
): Promise<IntegrationStatus[]> {
  return invoke("repair_agent_integration", { vendor })
}

export async function removeAgentIntegration(
  vendor: IntegrationVendor,
): Promise<IntegrationStatus[]> {
  return invoke("remove_agent_integration", { vendor })
}

export async function onAgentIntegrationsChanged(
  callback: (statuses: IntegrationStatus[]) => void,
): Promise<() => void> {
  if (!isTauri())
    return () => {
      // Browser previews have no native event subscription to release.
    }
  return listen<IntegrationStatus[]>("agent-integrations:changed", (event) =>
    callback(event.payload),
  )
}
