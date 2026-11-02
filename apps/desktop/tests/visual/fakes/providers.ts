import {
  defaultConnection,
  type ConnectionDraft,
  type InstalledOllamaModel,
  type ProviderSettings,
} from "../../../src/lib/smartCheckProviders"
import type { CheckAvailability } from "../../../src/lib/checkAvailability"
import type { CheckDirectory } from "../../../src/lib/checkDirectories"
import { emitFixtureEvent } from "./event"
import { fixtureCheckAvailability } from "./checks"
import { providerSettingsFixture } from "../../../src/views/settings/tests/providerSettingsFixture"

declare global {
  interface Window {
    __ANTIBURN_VISUAL_PROVIDER_CALLS__?: string[]
    __ANTIBURN_VISUAL_RELEASE_PROVIDERS__?: () => void
    __ANTIBURN_VISUAL_RECOVER_PROVIDERS__?: () => void
    __ANTIBURN_VISUAL_RELEASE_DISCOVERY__?: () => void
    __ANTIBURN_VISUAL_DIRECTORY_OPENS__?: string[]
    __ANTIBURN_VISUAL_RELEASE_DIRECTORIES__?: () => void
    __ANTIBURN_VISUAL_RELEASE_OLLAMA_MODELS__?: () => void
    __ANTIBURN_VISUAL_RELEASE_PROVIDER_OPERATION__?: () => void
    __ANTIBURN_VISUAL_PROVIDER_REQUESTS__?: Array<{
      command: string
      args: Record<string, unknown> | undefined
    }>
  }
}

export function fixtureProviderSettings(): ProviderSettings {
  const jev = defaultConnection("jev")
  jev.credential = { kind: "connection", id: "jev" }
  const ollama = { ...defaultConnection("ollama"), model: "synthetic-local-model" }
  const cloudflare = defaultConnection("cloudflare")
  cloudflare.endpoint = { kind: "cloudflare_account", value: "synthetic-account" }
  const custom = {
    ...defaultConnection("custom"),
    model: "synthetic-custom-model",
    endpoint: { kind: "exact_url" as const, value: "https://example.invalid/systemone" },
  }
  const params = new URLSearchParams(window.location.search)
  if (params.get("credentials") === "saved")
    cloudflare.credential = { kind: "connection", id: "cloudflare-key" }
  if (params.get("connections") === "empty" || params.get("connections") === "first-setup")
    return providerSettingsFixture({ activeId: null, profiles: {} })
  if (params.get("connections") === "last")
    return providerSettingsFixture({ activeId: "jev", profiles: { jev } })
  if (params.get("connections") === "unselected")
    return providerSettingsFixture({ activeId: null, profiles: { jev, ollama } })
  if (params.get("connections") === "custom-unbounded") {
    return providerSettingsFixture({
      activeId: "jev",
      profiles: { jev, custom: { ...custom, contextOverride: null } },
    })
  }
  if (params.get("connections") === "long") {
    custom.model = `synthetic-model-${"long-name-".repeat(12)}`
    custom.endpoint.value = `https://example.invalid/${"nested-path/".repeat(24)}`
    return providerSettingsFixture({
      activeId: "jev",
      profiles: { jev, [`custom-${"long-id-".repeat(16)}`]: custom },
    })
  }
  if (params.get("connections") === "multiple-jev") {
    return providerSettingsFixture({
      activeId: "personal",
      profiles: {
        personal: { ...jev, credential: { kind: "connection", id: "personal" } },
        work: { ...jev, credential: { kind: "connection", id: "work" } },
      },
    })
  }
  return providerSettingsFixture({
    activeId: "jev",
    profiles: { jev, ollama, cloudflare, custom },
  })
}

let providerReadsRecovered = false
export function readFixtureProviderSettings(): ProviderSettings | Promise<ProviderSettings> {
  if (
    new URLSearchParams(window.location.search).get("providerRead") === "until-retry" &&
    !providerReadsRecovered
  ) {
    window.__ANTIBURN_VISUAL_RECOVER_PROVIDERS__ = () => {
      providerReadsRecovered = true
    }
    throw new Error("Synthetic provider read failure")
  }
  if (new URLSearchParams(window.location.search).get("providerRead") === "loading") {
    return new Promise((resolve) => {
      window.__ANTIBURN_VISUAL_RELEASE_PROVIDERS__ = () => resolve(currentProviderSettings())
    })
  }
  return currentProviderSettings()
}

let providerSettings: ProviderSettings | undefined
let operationBefore: ProviderSettings | undefined
let operationOutcome: "completed" | "rolled_back" = "completed"
const displayCounters = new Map<string, number>()
function currentProviderSettings() {
  if (!providerSettings) {
    const stored = sessionStorage.getItem("antiburn-visual-pending-provider")
    if (stored) {
      const journal: {
        settings: ProviderSettings
        before: ProviderSettings
        outcome: "completed" | "rolled_back"
      } = JSON.parse(stored)
      providerSettings = journal.settings
      operationBefore = journal.before
      operationOutcome = journal.outcome
    } else providerSettings = fixtureProviderSettings()
  }
  for (const snapshot of [providerSettings, operationBefore]) {
    if (!snapshot) continue
    for (const [id, profile] of Object.entries(snapshot.profiles))
      displayCounters.set(
        profile.provider,
        Math.max(displayCounters.get(profile.provider) ?? 0, snapshot.displayNumbers[id] ?? 0),
      )
  }
  return providerSettings
}

function failOperation(
  kind: "save" | "switch" | "remove",
  connectionId: string,
  before: ProviderSettings,
  fault: string | null,
  requestId: string | null = null,
) {
  if (!["cleanup", "finalize", "rollback", "loading"].includes(fault ?? "")) return
  operationBefore = before
  operationOutcome = fault === "rollback" ? "rolled_back" : "completed"
  providerSettings = {
    ...(fault === "rollback" ? before : currentProviderSettings()),
    pendingOperation: { id: crypto.randomUUID(), kind, connectionId, requestId },
  }
  sessionStorage.setItem(
    "antiburn-visual-pending-provider",
    JSON.stringify({ settings: providerSettings, before, outcome: operationOutcome }),
  )
  throw new Error(
    fault === "cleanup"
      ? "Could not finish credential cleanup. Use Retry in Settings → Checks."
      : "Could not complete the connection operation. Use Retry in Settings → Checks.",
  )
}

export function fixtureProviderCommand(
  command: string,
  args: Record<string, unknown> | undefined,
) {
  const current = currentProviderSettings()
  if (command === "retry_system_one_connection_operation") {
    if (!current.pendingOperation || current.pendingOperation.id !== args?.operationId)
      throw new Error("This recovery operation is no longer pending. Refresh Settings.")
    const params = new URLSearchParams(window.location.search)
    const rolledBack = operationOutcome === "rolled_back" && Boolean(operationBefore)
    const finish = () => {
      providerSettings = {
        ...(rolledBack ? operationBefore! : current),
        pendingOperation: null,
      }
      sessionStorage.removeItem("antiburn-visual-pending-provider")
      return {
        settings: providerSettings,
        outcome: rolledBack ? ("rolled_back" as const) : ("completed" as const),
      }
    }
    if (params.get("providerRetry") === "loading")
      return new Promise((resolve) => {
        window.__ANTIBURN_VISUAL_RELEASE_PROVIDER_OPERATION__ = () => resolve(finish())
      })
    if (params.get("providerRetry") === "error")
      throw new Error(
        "Could not complete the connection operation. Use Retry in Settings → Checks.",
      )
    return finish()
  }
  if (current.pendingOperation && command !== "list_ollama_models")
    throw new Error("A connection operation is pending. Use Retry in Settings → Checks.")
  if (command === "list_ollama_models") {
    const draft = args?.draft as ConnectionDraft | undefined
    if (draft?.connection.provider !== "ollama")
      throw new Error("Connection settings are invalid.")
    const state = new URLSearchParams(window.location.search).get("ollamaModels")
    if (state === "error") throw new Error("Ollama is unavailable")
    const models: InstalledOllamaModel[] = [
      { id: "synthetic-local-model", label: "Local model" },
      { id: "synthetic-local-two", label: "Other local model" },
    ]
    if (state === "loading")
      return new Promise<InstalledOllamaModel[]>((resolve) => {
        window.__ANTIBURN_VISUAL_RELEASE_OLLAMA_MODELS__ = () => resolve(models)
      })
    return state === "empty" ? [] : models
  }
  if (command === "test_system_one_connection") return undefined
  if (command === "switch_system_one_connection") {
    if (new URLSearchParams(window.location.search).get("providerSwitch") === "error")
      throw new Error("The selected provider needs a valid credential.")
    if (typeof args?.connectionId !== "string" || !current.profiles[args.connectionId])
      throw new Error("The saved connection is missing.")
    providerSettings = { ...current, activeId: args.connectionId }
    failOperation(
      "switch",
      args.connectionId,
      current,
      new URLSearchParams(window.location.search).get("providerSwitch"),
    )
    return providerSettings
  }
  if (command === "save_system_one_connection") {
    const draft = args?.draft as ConnectionDraft | undefined
    if (!draft?.connection_id || !draft.connection)
      throw new Error("Connection settings are invalid.")
    if (typeof args?.requestId !== "string" || !args.requestId)
      throw new Error("Connection settings are invalid.")
    const requestId = args.requestId
    const existing = current.profiles[draft.connection_id]
    if (args?.intent !== "create" && args?.intent !== "update")
      throw new Error("Connection settings are invalid.")
    if (args.intent === "create" && existing)
      throw new Error("This connection identity was already used. Open Add connection again.")
    if (args.intent === "update" && !existing)
      throw new Error("This connection was removed. Close the editor and refresh Settings.")
    if (existing && existing.revision !== draft.connection.revision)
      throw new Error(
        "This connection changed in another window. Close the editor and refresh Settings.",
      )
    if (
      args.intent === "create" &&
      draft.connection.credential?.kind === "legacy_type_safe" &&
      draft.connection.provider !== "jev"
    )
      throw new Error("The saved TypeSafe key can be selected only for Jev.")
    if (
      ["jev", "cloudflare"].includes(draft.connection.provider) &&
      !draft.credential?.trim() &&
      !draft.connection.credential
    )
      throw new Error("The selected provider needs a credential. Add one and retry.")
    providerSettings = providerSettingsFixture({
      ...current,
      displayNumbers: existing
        ? current.displayNumbers
        : {
            ...current.displayNumbers,
            [draft.connection_id]: (displayCounters.get(draft.connection.provider) ?? 0) + 1,
          },
      activeId:
        Object.keys(current.profiles).length === 0 ? draft.connection_id : current.activeId,
      profiles: {
        ...current.profiles,
        [draft.connection_id]: {
          ...draft.connection,
          revision: draft.connection.revision + 1,
          credential: draft.credential
            ? { kind: "connection", id: crypto.randomUUID() }
            : draft.connection.credential,
        },
      },
    })
    if (new URLSearchParams(window.location.search).get("providerSave") === "loading")
      return new Promise((_resolve, reject) => {
        window.__ANTIBURN_VISUAL_RELEASE_PROVIDER_OPERATION__ = () => {
          try {
            failOperation("save", draft.connection_id, current, "finalize", requestId)
          } catch (error) {
            reject(error)
          }
        }
      })
    failOperation(
      "save",
      draft.connection_id,
      current,
      new URLSearchParams(window.location.search).get("providerSave"),
      requestId,
    )
    return providerSettings
  }
  if (command === "remove_system_one_connection") {
    if (new URLSearchParams(window.location.search).get("providerRemove") === "error")
      throw new Error("Could not remove the connection. Reload connections and try again.")
    if (typeof args?.connectionId !== "string" || !current.profiles[args.connectionId])
      throw new Error("The saved connection is missing.")
    if (current.profiles[args.connectionId]!.revision !== args.revision)
      throw new Error(
        "This connection changed in another window. Refresh Settings before removing it.",
      )
    const profiles = { ...current.profiles }
    delete profiles[args.connectionId]
    const selected = current.activeId === args.connectionId
    providerSettings = { ...current, activeId: selected ? null : current.activeId, profiles }
    if (selected) enabledOverride = false
    emitFixtureEvent("checks:availability-changed", {
      status: "updated",
      snapshot: readFixtureSettingsAvailability(fixtureCheckAvailability()),
    })
    failOperation(
      "remove",
      args.connectionId,
      current,
      new URLSearchParams(window.location.search).get("providerRemove"),
    )
    return providerSettings
  }
  if (command === "remove_typesafe_api_key") {
    legacyKeyForgotten = true
    providerSettings = {
      ...current,
      profiles: Object.fromEntries(
        Object.entries(current.profiles).map(([id, profile]) => [
          id,
          profile.credential?.kind === "legacy_type_safe"
            ? { ...profile, credential: null, revision: profile.revision + 1 }
            : profile,
        ]),
      ),
    }
    return undefined
  }
  if (command === "remove_system_one_credential") {
    if (typeof args?.connectionId !== "string" || !current.profiles[args.connectionId])
      throw new Error("The saved connection is missing.")
    providerSettings = {
      ...current,
      profiles: {
        ...current.profiles,
        [args.connectionId]: {
          ...current.profiles[args.connectionId]!,
          credential: null,
          revision: current.profiles[args.connectionId]!.revision + 1,
        },
      },
    }
    return undefined
  }
  throw new Error(`Unexpected synthetic provider command: ${command}`)
}

export function readFixtureCheckDirectories(): CheckDirectory[] | Promise<CheckDirectory[]> {
  const state = new URLSearchParams(window.location.search).get("directories")
  if (state === "unavailable") throw new Error("Synthetic directory read failure")
  const rows: CheckDirectory[] = [
    {
      reference: "fixture-global-checks",
      scope: "global",
      label: "This device",
      path: "/Users/fixture/.antiburn/checks",
      state: "ready",
      fileCount: 6,
      message: null,
    },
    {
      reference: "fixture-project-checks",
      scope: "project",
      label: "antiburn",
      path: `/Users/fixture/projects/${state === "long" ? "long-project-path/".repeat(12) : "antiburn/"}.antiburn/checks`,
      state: "ready",
      fileCount: 2,
      message: null,
    },
  ]
  if (state === "mixed")
    rows.push(
      {
        reference: "fixture-empty",
        scope: "project",
        label: "Empty project",
        path: "/Users/fixture/projects/empty/.antiburn/checks",
        state: "ready",
        fileCount: 0,
        message: null,
      },
      {
        reference: null,
        scope: "project",
        label: "Uncounted project",
        path: "/Users/fixture/projects/uncounted/.antiburn/checks",
        state: "ready",
        fileCount: null,
        message: null,
      },
      {
        reference: null,
        scope: "project",
        label: "Paused with files",
        path: "/Users/fixture/projects/paused-with-files/.antiburn/checks",
        state: "paused",
        fileCount: 3,
        message: null,
      },
      {
        reference: null,
        scope: "project",
        label: "New project",
        path: "/Users/fixture/projects/new/.antiburn/checks",
        state: "missing",
        fileCount: null,
        message: null,
      },
      {
        reference: null,
        scope: "project",
        label: "Paused project",
        path: "/Users/fixture/projects/paused/.antiburn/checks",
        state: "paused",
        fileCount: null,
        message: null,
      },
      {
        reference: null,
        scope: "project",
        label: "Unavailable project",
        path: "/Users/fixture/projects/unavailable/.antiburn/checks",
        state: "unavailable",
        fileCount: null,
        message: "Folder access is unavailable.",
      },
    )
  if (state === "loading")
    return new Promise((resolve) => {
      window.__ANTIBURN_VISUAL_RELEASE_DIRECTORIES__ = () => resolve(rows)
    })
  return state === "empty" ? [] : rows
}

export function openFixtureCheckDirectory(args: Record<string, unknown> | undefined) {
  if (typeof args?.directoryReference !== "string")
    throw new Error("Missing synthetic directory reference")
  window.__ANTIBURN_VISUAL_DIRECTORY_OPENS__ ??= []
  window.__ANTIBURN_VISUAL_DIRECTORY_OPENS__.push(args.directoryReference)
  if (new URLSearchParams(window.location.search).get("directoryOpen") === "error")
    throw new Error("Synthetic directory open failure")
}

let enabledOverride: boolean | undefined
let legacyKeyForgotten = false
export function setFixtureModelChecksEnabled(
  value: CheckAvailability,
  args: Record<string, unknown> | undefined,
) {
  if (typeof args?.enabled !== "boolean") throw new Error("Missing synthetic enabled state")
  if (
    args.enabled &&
    (!currentProviderSettings().activeId || currentProviderSettings().pendingOperation)
  )
    throw new Error("Select a connection before enabling checks.")
  enabledOverride = args.enabled
  return { ...value, yamlEnabled: enabledOverride }
}

export function readFixtureSettingsAvailability(
  value: CheckAvailability,
): CheckAvailability | Promise<CheckAvailability> {
  if (enabledOverride !== undefined) value = { ...value, yamlEnabled: enabledOverride }
  value = {
    ...value,
    savedKey:
      !legacyKeyForgotten &&
      new URLSearchParams(window.location.search).get("legacyKey") === "saved",
  }
  const usage = new URLSearchParams(window.location.search).get("checkUsage")
  if (usage && usage !== "empty")
    value = {
      ...value,
      usage: {
        ...value.usage,
        confirmedCalls: usage === "unknown-only" ? 0 : 12,
        inputTokens: usage === "unknown-only" ? 0 : 12000,
        outputTokens: usage === "unknown-only" ? 0 : 1800,
        unknownOutcomes: usage === "unknown" || usage === "unknown-only" ? 2 : 0,
        estimatedUsd: usage === "unpriced" || usage === "unknown-only" ? null : "$0.000017",
      },
    }
  const state = new URLSearchParams(window.location.search).get("discovery")
  if (state === "unavailable") throw new Error("Synthetic check status read failure")
  if (state === "loading")
    return new Promise((resolve) => {
      window.__ANTIBURN_VISUAL_RELEASE_DISCOVERY__ = () => resolve(value)
    })
  if (state === "0" || state === "1" || state === "6") {
    return {
      ...value,
      installedChecks: Array.from({ length: Number(state) }, (_, index) => ({
        effectiveId: `installed:synthetic-${index}`,
        name: `Synthetic ${index}`,
        aliases: [],
        availability: { status: index % 2 ? "paused" : "invalid" },
      })),
    }
  }
  return value
}
