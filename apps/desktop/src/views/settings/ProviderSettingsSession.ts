import {
  defaultConnection,
  getProviderSettings,
  refreshProviderLimits,
  removeProviderCredential,
  saveProviderConnection,
  switchProviderConnection,
  testProviderConnection,
  type Connection,
  type ConnectionDraft,
  type ModelCapabilities,
  type ProviderSettings,
  type SmartCheckProvider,
} from "../../lib/smartCheckProviders"
import { removeTypeSafeApiKey } from "../../lib/checkAvailability"

type DraftState = {
  draft: ConnectionDraft
  tested: boolean
  capabilities: ModelCapabilities | null
}
type Snapshot = {
  saved: ProviderSettings | null
  selectedId: string
  drafts: Record<string, DraftState>
  busy: boolean
  error: string | null
  status: string | null
}

const safeErrors = new Set([
  "Enter a TypeSafe API key.",
  "Enter a Cloudflare API token.",
  "Credential storage is unavailable.",
  "Could not save the connection credential.",
  "Could not remove the connection credential.",
  "The connection credential reference is invalid.",
  "Saved Smart Burn Checks provider settings are invalid.",
  "Saved connection settings are invalid.",
  "The saved connection is missing.",
  "Could not save connection settings.",
  "Could not protect the connection update state.",
  "Could not complete the connection update.",
  "Could not protect the credential removal state.",
  "Could not disable checks.",
  "Could not save the credential removal state.",
  "Could not restore the connection credential. Retry the connection update in Settings → Checks.",
  "Could not restore the connection credential. Retry credential removal in Settings → Checks.",
  "Connection credential removal is incomplete. Retry it in Settings → Checks.",
  "Smart Burn Checks connection update is incomplete. Retry it in Settings → Checks.",
  "TypeSafe credential removal is incomplete. Retry it in Settings → Checks.",
  "Remove the TypeSafe API key in Settings → Checks.",
  "Could not remove the API key from credential storage.",
  "The selected provider rejected its credential. Update it in Settings → Checks.",
  "The selected provider needs a valid credential.",
  "The selected model provider needs a valid credential.",
  "The selected provider needs a credential. Add one and retry.",
  "Connection settings are invalid.",
  "Enter a valid connection credential.",
  "TypeSafe rejected the test request. Check the API key.",
  "Cloudflare rejected the test request. Check the account, model, and token.",
  "The custom endpoint rejected the test request. Check the URL and response mode.",
  "Could not reach Ollama. Check the server address and try again.",
  "Invalid Ollama base URL",
  "Ollama must be version 0.35.0 or newer",
  "The selected Ollama model is not installed",
  "The Ollama model does not support System One decisions",
  "The Ollama model is not loaded; retry after it loads",
  "The Ollama model rejected the request context",
  "Ollama text requests cannot exceed 64 KiB",
  "Ollama rejected the configured authentication",
  "Ollama is unavailable",
  "The Ollama request outcome is unknown",
  "The Ollama request is invalid",
  "Ollama returned an invalid System One response",
  "The Ollama response exceeded the local limit",
])

export function providerErrorMessage(error: unknown, fallback: string): string {
  const message = error instanceof Error ? error.message : error
  return typeof message === "string" && safeErrors.has(message) ? message : fallback
}

export class ProviderSettingsSession {
  private listeners = new Set<() => void>()
  private generation = 0
  private state: Snapshot = {
    saved: null,
    selectedId: "jev",
    drafts: {},
    busy: false,
    error: null,
    status: null,
  }
  getSnapshot = () => this.state
  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) {
      const generation = ++this.generation
      void getProviderSettings()
        .then((saved) => {
          if (generation !== this.generation) return
          if (!saved?.profiles[saved.activeId]) throw new Error("Missing active connection")
          this.load(saved)
        })
        .catch(() => {
          if (generation === this.generation)
            this.publish({
              error: "Could not load saved connections. Reopen Settings to try again.",
            })
        })
    }
    return () => {
      this.listeners.delete(listener)
      if (!this.listeners.size) this.generation++
    }
  }
  private publish(update: Partial<Snapshot>) {
    this.state = { ...this.state, ...update }
    this.listeners.forEach((listener) => listener())
  }
  private load(saved: ProviderSettings) {
    const drafts = Object.fromEntries(
      Object.entries(saved.profiles).map(([id, connection]) => [
        id,
        {
          draft: { connection_id: id, connection, credential: null },
          tested: false,
          capabilities: null,
        },
      ]),
    )
    this.publish({ saved, selectedId: saved.activeId, drafts, error: null })
  }
  select(id: string) {
    if (this.state.busy) return
    this.publish({ selectedId: id, error: null, status: null })
  }
  selectProvider(provider: SmartCheckProvider) {
    const id =
      Object.entries(this.state.drafts).find(
        ([, value]) => value.draft.connection.provider === provider,
      )?.[0] ?? provider
    if (!this.state.drafts[id]) {
      this.publish({
        drafts: {
          ...this.state.drafts,
          [id]: {
            draft: {
              connection_id: id,
              connection: defaultConnection(provider),
              credential: null,
            },
            tested: false,
            capabilities: null,
          },
        },
      })
    }
    this.select(id)
  }
  edit(connection: Connection, credential: string | null) {
    if (this.state.busy) return
    const id = this.state.selectedId
    this.publish({
      drafts: {
        ...this.state.drafts,
        [id]: {
          draft: { connection_id: id, connection, credential },
          tested: false,
          capabilities: null,
        },
      },
      error: null,
      status: null,
    })
  }
  private acceptSaved(saved: ProviderSettings, id: string, status: string) {
    const connection = saved.profiles[id]
    if (!connection) throw new Error("Missing saved connection")
    this.publish({
      saved,
      drafts: {
        ...this.state.drafts,
        [id]: {
          draft: { connection_id: id, connection, credential: null },
          tested: false,
          capabilities: null,
        },
      },
      status,
    })
  }
  async run(action: "test" | "save" | "refresh" | "remove" | "switch") {
    if (this.state.busy) return
    const id = this.state.selectedId
    const current = this.state.drafts[id]
    if (!current) return
    this.publish({ busy: true, error: null, status: null })
    try {
      if (action === "test") {
        await testProviderConnection(current.draft)
        this.publish({
          drafts: { ...this.state.drafts, [id]: { ...current, tested: true } },
          status: "This draft passed the connection test.",
        })
      } else if (action === "refresh") {
        const capabilities = await refreshProviderLimits(current.draft)
        this.publish({
          drafts: { ...this.state.drafts, [id]: { ...current, capabilities } },
          status: "Model limits refreshed for this draft.",
        })
      } else if (action === "save" || action === "switch") {
        const saved =
          action === "save"
            ? await saveProviderConnection(current.draft)
            : await switchProviderConnection(id)
        this.acceptSaved(saved, id, "Connection saved and selected for Smart Burn Checks.")
      } else {
        this.publish({
          drafts: {
            ...this.state.drafts,
            [id]: { ...current, tested: false, capabilities: null },
          },
        })
        if (current.draft.connection.credential?.kind === "legacy_type_safe")
          await removeTypeSafeApiKey()
        else await removeProviderCredential(id)
        const saved = await getProviderSettings()
        this.acceptSaved(saved, id, "Credential removed.")
      }
    } catch (error) {
      this.publish({
        error: providerErrorMessage(
          error,
          `Could not ${action === "refresh" ? "refresh model limits" : action === "remove" ? "remove the credential" : action === "switch" ? "select the saved connection" : `${action} the connection`}. Check the settings and try again.`,
        ),
      })
      if (action === "save" || action === "switch" || action === "remove") {
        try {
          const saved = await getProviderSettings()
          if (!saved?.profiles[saved.activeId])
            throw new Error("Missing active connection", { cause: error })
          this.publish({ saved })
        } catch {
          this.publish({
            status: "Could not confirm the active connection. Reopen Settings to refresh it.",
          })
        }
      }
    } finally {
      this.publish({ busy: false })
    }
  }
}
