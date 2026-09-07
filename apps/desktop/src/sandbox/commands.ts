/**
 * The `invoke` switch for the UI sandbox.
 *
 * Each shell command answers from the active scenario. An unknown command
 * resolves to `null`, the same default the popover test uses, and logs once
 * so a missing fixture is easy to spot in the console.
 */

import type { AppSettings, SessionIdentityPayload } from "../lib/ipc"
import { sessionAnalysis, sessionHygiene } from "./fixtures"
import { pickScenario, type Scenario } from "./scenarios"
import { emit } from "./tauri-event"

let active: Scenario | null = null

function scenario(): Scenario {
  active ??= pickScenario()
  return active
}

const unknownCommands = new Set<string>()

export function runCommand(command: string, args: Record<string, unknown> = {}): unknown {
  const state = scenario()
  switch (command) {
    case "app_info":
      return state.appInfo
    case "engine_catalog_version":
      return state.appInfo.pricingCatalogVersion
    case "get_settings":
      return state.settings
    case "set_settings": {
      state.settings = args["settings"] as AppSettings
      emit("settings:changed", state.settings)
      return state.settings
    }
    case "list_recent_sessions":
      return state.entries
    case "get_session_analysis": {
      const sessionId = args["sessionId"] as string
      const entry = state.entries.find((each) => each.sessionId === sessionId)
      return state.analyses[sessionId] ?? (entry ? sessionAnalysis(entry) : null)
    }
    case "get_subagent_analysis":
      return state.subagentAnalyses[args["subagentId"] as string] ?? null
    case "get_session_hygiene": {
      const sessions = args["sessions"] as SessionIdentityPayload[]
      return sessions.map((each) => state.hygiene[each.sessionId] ?? sessionHygiene())
    }
    case "get_provider_usage":
      return state.providerUsage
    case "get_live_usage":
    case "refresh_live_usage":
      return state.liveUsage
    case "get_session_limit_allocations":
      return state.allocations
    case "get_checks_report":
      return state.checksReport
    case "get_scan_status":
    case "scan_now":
    case "cancel_scan":
      return state.scanStatus
    case "get_storage_health":
      return state.storageHealth
    case "list_scan_roots":
    case "default_scan_roots":
    case "list_repositories":
    case "refresh_repositories":
    case "set_repository_enabled":
      return []
    case "set_popover_height":
      return true
    case "show_popover_peek":
      return { generation: 1, target: args["target"] }
    default:
      if (!unknownCommands.has(command)) {
        unknownCommands.add(command)
        console.warn(`[sandbox] ${command} has no fixture; resolving null`)
      }
      return null
  }
}
